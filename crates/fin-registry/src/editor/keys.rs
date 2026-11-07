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

use super::EditorError;
use crate::atomic;

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

    pub fn read(path: &Path) -> Result<SigningKey, EditorError> {
        let text = fs::read_to_string(path).map_err(|e| EditorError::io("read", path, e))?;
        SigningKey::from_pem(&text)
    }

    pub fn to_pem(&self) -> String {
        pem::encode(&pem::Pem::new("PRIVATE KEY", self.pkcs8.clone()))
    }

    /// Writes the key as PEM, temp then rename, the temp file created with mode 0600 on Unix
    /// (never readable by others, even for a moment). On Windows the file inherits its
    /// folder's ACL: keep key folders in the owner's profile, where only the owner can read.
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

    /// Saves `key` in the ring; a new ring folder is created owner-only (0700 on Unix).
    pub fn add(&self, key: &SigningKey) -> Result<(), EditorError> {
        let mut b = fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        b.create(&self.dir)
            .map_err(|e| EditorError::io("create", &self.dir, e))?;
        key.write(&self.path(&key.public().key_id()))
    }

    /// The private key whose id is `key_id`, if this ring holds it.
    pub fn get(&self, key_id: &str) -> Result<Option<SigningKey>, EditorError> {
        let p = self.path(key_id);
        if !p.exists() {
            return Ok(None);
        }
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
