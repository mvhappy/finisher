//! ed25519 keys as PEM files (spec §5.6): private keys are PKCS#8 (`PRIVATE KEY`, what
//! `openssl genpkey -algorithm ed25519` writes), public keys SubjectPublicKeyInfo
//! (`PUBLIC KEY`). Key bytes never reach `Debug` or `Display`.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use tough::schema::key::{Ed25519Key, Ed25519Scheme, Key};

use serde_json::json;

use super::EditorError;
use crate::atomic;
use crate::private::{self, Kind};

/// The DER prefix of an ed25519 SubjectPublicKeyInfo (RFC 8410), before the 32 key bytes.
const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// An ed25519 private key.
pub struct SigningKey {
    pkcs8: Vec<u8>,
    pair: Ed25519KeyPair,
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SigningKey({})", self.public().key_id())
    }
}

impl Clone for SigningKey {
    fn clone(&self) -> SigningKey {
        SigningKey::from_pkcs8(&self.pkcs8).expect("a key that parsed once parses again")
    }
}

impl SigningKey {
    /// A new random key.
    pub fn generate() -> SigningKey {
        let doc = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("key generation");
        SigningKey::from_pkcs8(doc.as_ref()).expect("a generated key parses")
    }

    pub fn from_pkcs8(der: &[u8]) -> Result<SigningKey, EditorError> {
        let pair = Ed25519KeyPair::from_pkcs8(der)
            .map_err(|_| EditorError::key("not an ed25519 PKCS#8 private key"))?;
        Ok(SigningKey {
            pkcs8: der.to_vec(),
            pair,
        })
    }

    /// Parses a `PRIVATE KEY` PEM.
    pub fn from_pem(text: &str) -> Result<SigningKey, EditorError> {
        let p = pem::parse(text).map_err(|_| EditorError::key("not a PEM file"))?;
        if p.tag() != "PRIVATE KEY" {
            return Err(EditorError::key(format!(
                "a {} PEM is not an ed25519 private key",
                p.tag()
            )));
        }
        SigningKey::from_pkcs8(p.contents())
    }

    /// Reads a private key file, refused (`KEY_UNPROTECTED`) when anyone but its owner can
    /// reach it, as `ssh` refuses an unprotected key ([`crate::private`]).
    pub fn read(path: &Path) -> Result<SigningKey, EditorError> {
        let text = fs::read_to_string(path).map_err(|e| EditorError::io("read", path, e))?;
        refuse_unprotected(path, Kind::File)?;
        SigningKey::from_pem(&text)
    }

    pub fn to_pem(&self) -> String {
        pem::encode(&pem::Pem::new("PRIVATE KEY", self.pkcs8.clone()))
    }

    /// Writes the key as PEM, temp then rename, the temp file created owner-only (0600 on
    /// Unix, a protected owner-only DACL on Windows), so it is never readable by others, even
    /// for a moment, whatever the folder allows.
    pub fn write(&self, path: &Path) -> Result<(), EditorError> {
        atomic::write_private(path, self.to_pem().as_bytes())
            .map_err(|e| EditorError::io("write", path, e))
    }

    pub fn public(&self) -> PublicKey {
        let mut raw = [0u8; 32];
        raw.copy_from_slice(self.pair.public_key().as_ref());
        PublicKey(raw)
    }

    pub(crate) fn sign(&self, msg: &[u8]) -> Vec<u8> {
        self.pair.sign(msg).as_ref().to_vec()
    }
}

/// An ed25519 public key.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey([u8; 32]);

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.key_id())
    }
}

impl PublicKey {
    /// Parses a `PUBLIC KEY` PEM, or derives the public key of a `PRIVATE KEY` PEM.
    pub fn from_pem(text: &str) -> Result<PublicKey, EditorError> {
        let p = pem::parse(text).map_err(|_| EditorError::key("not a PEM file"))?;
        match p.tag() {
            "PRIVATE KEY" => Ok(SigningKey::from_pkcs8(p.contents())?.public()),
            "PUBLIC KEY" => {
                let der = p.contents();
                if der.len() != 44 || der[..12] != SPKI_PREFIX {
                    return Err(EditorError::key("not an ed25519 public key"));
                }
                let mut raw = [0u8; 32];
                raw.copy_from_slice(&der[12..]);
                Ok(PublicKey(raw))
            }
            other => Err(EditorError::key(format!("a {other} PEM is not a key"))),
        }
    }

    pub fn read(path: &Path) -> Result<PublicKey, EditorError> {
        let text = fs::read_to_string(path).map_err(|e| EditorError::io("read", path, e))?;
        PublicKey::from_pem(&text)
    }

    pub fn to_pem(&self) -> String {
        let mut der = SPKI_PREFIX.to_vec();
        der.extend_from_slice(&self.0);
        pem::encode(&pem::Pem::new("PUBLIC KEY", der))
    }

    /// The key as TUF metadata lists it.
    pub fn tuf_key(&self) -> Key {
        Key::Ed25519 {
            keyval: Ed25519Key {
                public: self.0.to_vec().into(),
                _extra: HashMap::new(),
            },
            scheme: Ed25519Scheme::Ed25519,
            _extra: HashMap::new(),
        }
    }

    /// The TUF key id: lowercase hex SHA-256 of the key's canonical JSON.
    pub fn key_id(&self) -> String {
        let id = self.tuf_key().key_id().expect("a key serialises");
        hex::encode(id.as_ref())
    }
}

/// The editor's private keys: one PEM per key, named by key id, in a directory that is never
/// part of the published repository (the publisher's secret folder).
#[derive(Debug, Clone)]
pub struct KeyRing {
    dir: PathBuf,
}

impl KeyRing {
    pub fn new(dir: impl Into<PathBuf>) -> KeyRing {
        KeyRing { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, key_id: &str) -> PathBuf {
        self.dir.join(format!("{key_id}.pem"))
    }

    /// Creates the ring folder, and any missing folder above it, owner-only from creation
    /// (0700 on Unix, a protected owner-only DACL on Windows). Every write into the ring
    /// creates it through here; an existing ring folder is refused when it is not private.
    pub(crate) fn create(&self) -> Result<(), EditorError> {
        let mut missing = Vec::new();
        let mut cur = self.dir.as_path();
        loop {
            match fs::metadata(cur) {
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(cur);
                    match cur.parent() {
                        Some(p) if !p.as_os_str().is_empty() => cur = p,
                        _ => break,
                    }
                }
                Err(e) => return Err(EditorError::io("create", cur, e)),
            }
        }
        for d in missing.into_iter().rev() {
            match private::create_dir(d) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && d.is_dir() => {}
                Err(e) => return Err(EditorError::io("create", d, e)),
            }
        }
        self.check()
    }

    /// Refuses (`KEY_UNPROTECTED`) a ring folder anyone but its owner can reach; a ring that
    /// does not exist yet passes.
    pub(crate) fn check(&self) -> Result<(), EditorError> {
        if fs::metadata(&self.dir).is_err() {
            return Ok(());
        }
        refuse_unprotected(&self.dir, Kind::Folder)
    }

    /// Saves `key` in the ring; a new ring folder is created owner-only.
    pub fn add(&self, key: &SigningKey) -> Result<(), EditorError> {
        self.create()?;
        key.write(&self.path(&key.public().key_id()))
    }

    /// The private key whose id is `key_id`, if this ring holds it. The ring folder and the
    /// key file must be private ([`SigningKey::read`]).
    pub fn get(&self, key_id: &str) -> Result<Option<SigningKey>, EditorError> {
        let p = self.path(key_id);
        if !p.exists() {
            return Ok(None);
        }
        self.check()?;
        SigningKey::read(&p).map(Some)
    }

    /// The keys of `ids` this ring holds.
    pub fn signers(&self, ids: &[String]) -> Result<Vec<SigningKey>, EditorError> {
        let mut out = Vec::new();
        for id in ids {
            if let Some(k) = self.get(id)? {
                out.push(k);
            }
        }
        Ok(out)
    }
}

/// `KEY_UNPROTECTED {path, problems}`, with the command that fixes it, when someone but the
/// owner (and the OS) can reach the key folder or key file `path`.
fn refuse_unprotected(path: &Path, kind: Kind) -> Result<(), EditorError> {
    let u = match private::check(path, kind) {
        Ok(None) => return Ok(()),
        Ok(Some(u)) => u,
        Err(e) => return Err(EditorError::io("check the permissions of", path, e)),
    };
    let what = match kind {
        Kind::Folder => "key folder",
        Kind::File => "private key",
    };
    let mut e = EditorError::new(
        "KEY_UNPROTECTED",
        format!(
            "the {what} {} is not private ({}): others could read or replace the signing \
             keys; fix it, and rotate keys others may have read",
            path.display(),
            u.problems.join(", ")
        ),
        json!({ "path": path.display().to_string(), "problems": u.problems }),
    );
    e.fix = Some(u.fix);
    Err(e)
}
