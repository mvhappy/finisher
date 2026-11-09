//! The vault's file store (5B Decision 4): `<server-data>/secrets/<name>`, each file 0600 in a
//! 0700 directory (Windows: the directory's protected owner-only DACL, which `finisherd` sets
//! and checks before it opens the vault), written temp-then-rename with fsyncs.
//!
//! Every file starts with a header: `FINVAULT`, a format byte (1) and a mode byte, `0` plain or
//! `1` wrapped. With a key file (`FIN_SECRETS_KEY_FILE`: 32 random bytes in base64url, a Docker
//! secret) a value is AES-256-GCM sealed (`ring` `aead`) with a random 96-bit nonce, and the
//! associated data binds it to its name and its environment, so a file renamed, or copied into
//! another server data, does not open. A wrapped file read without its key, or with another
//! key, is `SECRETS_KEY_INVALID`; a plain file read while a key is set is wrapped at once.
//! Napi-free, so `tests/vault.rs` includes it by path.

// The server data's own `secrets/` folder and the key file the owner names, never a library path.
#![allow(clippy::disallowed_methods)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use fin_core::CoreError;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::json;

const MAGIC: &[u8; 8] = b"FINVAULT";
const FORMAT: u8 = 1;
const PLAIN: u8 = 0;
const WRAPPED: u8 = 1;
const HEADER: usize = 10;
/// The largest value the vault keeps (keys and PEMs are far smaller).
pub const MAX_VALUE: usize = 64 * 1024;

/// A vault entry's name: lower-case letters, digits and dashes, 1–64 of them.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub fn invalid_name(name: &str) -> CoreError {
    CoreError::new(
        "INVALID_REQUEST",
        "a vault entry's name is 1–64 lower-case letters, digits and dashes",
    )
    .with_details(json!({ "field": "name", "length": name.len() }))
}

pub fn key_invalid(reason: &str) -> CoreError {
    CoreError::new("SECRETS_KEY_INVALID", "the server's secret files cannot be opened with the key it was given")
        .with_details(json!({ "reason": reason }))
        .with_fix(
            "The secrets key file does not match the files in this server data. Give the server the key these files were written with, \
             or move the secrets folder aside to start with new secrets (paired devices then pair again).",
        )
}

fn unsafe_store(what: &str) -> CoreError {
    CoreError::new(
        "SECRET_STORE_UNSAFE",
        "the server's secrets folder is not private",
    )
    .with_details(json!({ "what": what }))
    .with_fix("Make the server-data folder private to the account the server runs as.")
}

fn io(what: &str, e: &std::io::Error) -> CoreError {
    CoreError::new(
        "SECRET_STORE_UNAVAILABLE",
        "the server's secrets folder cannot be used",
    )
    .with_details(json!({ "what": what, "kind": format!("{:?}", e.kind()) }))
}

/// Reads a key file: 32 bytes in base64url (padding and surrounding whitespace allowed).
pub fn read_key_file(path: &Path) -> Result<[u8; 32], CoreError> {
    let text = fs::read_to_string(path).map_err(|_| key_invalid("keyFileUnreadable"))?;
    let t = text.trim().trim_end_matches('=');
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(t)
        .map_err(|_| key_invalid("keyFileNotBase64url"))?;
    bytes
        .try_into()
        .map_err(|_| key_invalid("keyFileNot32Bytes"))
}

pub struct FileStore {
    dir: PathBuf,
    env_id: String,
    key: Option<LessSafeKey>,
}

impl FileStore {
    /// Opens (creating) `<server_data>/secrets`. On Unix the directory must be ours and not a
    /// link; a loose mode is tightened to 0700.
    pub fn open(
        server_data: &Path,
        env_id: &str,
        key: Option<[u8; 32]>,
    ) -> Result<Self, CoreError> {
        let dir = server_data.join("secrets");
        match fs::symlink_metadata(&dir) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                create_private_dir(&dir).map_err(|e| io("dir", &e))?
            }
            Err(e) => return Err(io("dir", &e)),
        }
        let md = fs::symlink_metadata(&dir).map_err(|e| io("dir", &e))?;
        if !md.is_dir() || md.file_type().is_symlink() {
            return Err(unsafe_store("dir"));
        }
        private_unix(&dir, &md, 0o700)?;
        let key = key.map(|k| {
            LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &k).expect("a 32-byte AES-256 key"))
        });
        Ok(Self {
            dir,
            env_id: env_id.to_owned(),
            key,
        })
    }

    pub fn wrapped(&self) -> bool {
        self.key.is_some()
    }

    fn aad(&self, name: &str) -> Vec<u8> {
        // The header (format, wrapped) is bound too: a file's mode byte cannot be flipped.
        let mut a = b"finisher/vault/v1\0".to_vec();
        a.extend_from_slice(MAGIC);
        a.extend_from_slice(&[FORMAT, WRAPPED, 0]);
        a.extend_from_slice(self.env_id.as_bytes());
        a.push(0);
        a.extend_from_slice(name.as_bytes());
        a
    }

    pub fn get(&self, name: &str) -> Result<Option<Vec<u8>>, CoreError> {
        if !valid_name(name) {
            return Err(invalid_name(name));
        }
        let path = self.dir.join(name);
        let md = match fs::symlink_metadata(&path) {
            Ok(md) => md,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io("file", &e)),
        };
        if !md.is_file() {
            return Err(unsafe_store("file"));
        }
        private_unix(&path, &md, 0o600)?;
        let bytes = fs::read(&path).map_err(|e| io("file", &e))?;
        if bytes.len() < HEADER || &bytes[..8] != MAGIC || bytes[8] != FORMAT {
            return Err(
                CoreError::new("SECRET_CORRUPT", "a secret file of the server is damaged")
                    .with_details(json!({ "name": name })),
            );
        }
        match (bytes[9], &self.key) {
            (PLAIN, None) => Ok(Some(bytes[HEADER..].to_vec())),
            (PLAIN, Some(_)) => {
                // A key was set since: wrap it now, so no plain copy stays behind.
                let value = bytes[HEADER..].to_vec();
                self.set(name, &value)?;
                Ok(Some(value))
            }
            (WRAPPED, None) => Err(key_invalid("noKey")),
            (WRAPPED, Some(key)) => {
                let body = &bytes[HEADER..];
                if body.len() < NONCE_LEN + AES_256_GCM.tag_len() {
                    return Err(key_invalid("short"));
                }
                let (nonce, sealed) = body.split_at(NONCE_LEN);
                let nonce =
                    Nonce::try_assume_unique_for_key(nonce).map_err(|_| key_invalid("nonce"))?;
                let mut buf = sealed.to_vec();
                let plain = key
                    .open_in_place(nonce, Aad::from(self.aad(name)), &mut buf)
                    .map_err(|_| key_invalid("doesNotOpen"))?;
                Ok(Some(plain.to_vec()))
            }
            _ => Err(
                CoreError::new("SECRET_CORRUPT", "a secret file of the server is damaged")
                    .with_details(json!({ "name": name })),
            ),
        }
    }

    pub fn set(&self, name: &str, value: &[u8]) -> Result<(), CoreError> {
        if !valid_name(name) {
            return Err(invalid_name(name));
        }
        if value.len() > MAX_VALUE {
            return Err(
                CoreError::new("INVALID_REQUEST", "a vault value is at most 64 KiB")
                    .with_details(json!({ "field": "value" })),
            );
        }
        let mut out = Vec::with_capacity(HEADER + value.len() + 32);
        out.extend_from_slice(MAGIC);
        out.push(FORMAT);
        match &self.key {
            None => {
                out.push(PLAIN);
                out.extend_from_slice(value);
            }
            Some(key) => {
                out.push(WRAPPED);
                let mut nonce = [0u8; NONCE_LEN];
                SystemRandom::new()
                    .fill(&mut nonce)
                    .map_err(|_| CoreError::new("INTERNAL", "no random bytes"))?;
                out.extend_from_slice(&nonce);
                let mut buf = value.to_vec();
                key.seal_in_place_append_tag(
                    Nonce::assume_unique_for_key(nonce),
                    Aad::from(self.aad(name)),
                    &mut buf,
                )
                .map_err(|_| CoreError::new("INTERNAL", "sealing failed"))?;
                out.extend_from_slice(&buf);
            }
        }
        write_atomic(&self.dir, name, &out).map_err(|e| io("write", &e))
    }

    pub fn delete(&self, name: &str) -> Result<bool, CoreError> {
        if !valid_name(name) {
            return Err(invalid_name(name));
        }
        match fs::remove_file(self.dir.join(name)) {
            Ok(()) => {
                sync_dir(&self.dir).map_err(|e| io("dir", &e))?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(io("delete", &e)),
        }
    }
}

fn random_suffix() -> String {
    let mut b = [0u8; 6];
    let _ = SystemRandom::new().fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Temp-then-rename: the temp file is created exclusively with mode 0600 before any byte is
/// written, fsynced, renamed over the name, and the directory fsynced (not on Windows, where a
/// directory cannot be opened for flushing and NTFS journals the rename).
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = dir.join(format!(".{name}.{}.tmp", random_suffix()));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, dir.join(name))?;
        sync_dir(dir)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir(dir)
}

/// Unix: owned by the effective user and without group or other bits (a loose mode is
/// tightened to `mode`). Elsewhere `finisherd` checks the DACL before opening the vault.
#[cfg(unix)]
fn private_unix(path: &Path, md: &fs::Metadata, mode: u32) -> Result<(), CoreError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    if md.uid() != uid {
        return Err(unsafe_store("owner"));
    }
    if md.mode() & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| io("chmod", &e))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_unix(_path: &Path, _md: &fs::Metadata, _mode: u32) -> Result<(), CoreError> {
    Ok(())
}
