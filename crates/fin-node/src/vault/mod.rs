//! The vault (5B Decision 4) as napi: `Vault.open(serverData, envId, mode, keyFile?)` picks the
//! OS keychain or the file store (`keychain::choose`, never a prompt, never a blocked start),
//! and keeps small named secrets: the identity key (`identityKey`, Decision 5) and the secret
//! box's data key (`secrets-dek`, Decision 7). Values cross into JS as Buffers; nothing here
//! logs a value or puts one in an error.
//!
//! A keychain that errors after it was chosen fails the call (`SECRET_STORE_UNAVAILABLE`): the
//! fallback is decided once, at open, so one environment never splits its secrets between two
//! stores.

pub mod file;
pub mod keychain;

use std::path::Path;

use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::identity;
use crate::{guarded, reject};
use file::FileStore;
use keychain::{Choice, Keychain, Mode, OsKeychain};

enum Backend {
    Keychain {
        kc: Box<dyn Keychain>,
        env_id: String,
    },
    File(Box<FileStore>),
}

/// Which store the vault uses: `keychain`, `file` or `fileWrapped`; `reason` says why a file
/// store stands in for the keychain (`keychainLocked`, `keychainAbsent`, `keychainError`).
#[napi(object)]
pub struct VaultBackend {
    pub kind: String,
    pub reason: Option<String>,
}

#[napi]
pub struct Vault {
    backend: Backend,
    reason: Option<&'static str>,
}

impl Vault {
    fn get_inner(&self, name: &str) -> std::result::Result<Option<Vec<u8>>, fin_core::CoreError> {
        match &self.backend {
            Backend::File(f) => f.get(name),
            Backend::Keychain { kc, env_id } => {
                if !file::valid_name(name) {
                    return Err(file::invalid_name(name));
                }
                kc.get(&keychain::account(env_id, name))
                    .map_err(|e| keychain::unavailable(&e))
            }
        }
    }

    fn set_inner(&self, name: &str, value: &[u8]) -> std::result::Result<(), fin_core::CoreError> {
        match &self.backend {
            Backend::File(f) => f.set(name, value),
            Backend::Keychain { kc, env_id } => {
                if !file::valid_name(name) {
                    return Err(file::invalid_name(name));
                }
                kc.set(&keychain::account(env_id, name), value)
                    .map_err(|e| keychain::unavailable(&e))
            }
        }
    }
}

#[napi]
impl Vault {
    /// Opens the vault of `env_id`. `mode`: `auto` | `keychain` | `file` (`FIN_SECRETS`);
    /// `key_file` (`FIN_SECRETS_KEY_FILE`) wraps the file store's values.
    #[napi(factory, catch_unwind)]
    pub fn open(
        server_data: String,
        env_id: String,
        mode: String,
        key_file: Option<String>,
    ) -> Result<Vault> {
        guarded(
            || {
                let mode = Mode::parse(&mode).map_err(reject)?;
                let key = match key_file.as_deref() {
                    None | Some("") => None,
                    Some(p) => Some(file::read_key_file(Path::new(p)).map_err(reject)?),
                };
                // A key file asks for the file store: it exists for hosts without a keychain.
                let choice = match (key.is_some(), mode) {
                    (true, Mode::Keychain) => {
                        return Err(reject(
                            fin_core::CoreError::new("INVALID_REQUEST", "a secrets key file wraps the file store, which FIN_SECRETS=keychain does not use")
                                .with_details(serde_json::json!({ "field": "FIN_SECRETS_KEY_FILE" }))
                                .with_fix("Use the key file with the file store, or leave it out to keep secrets in the system keychain."),
                        ));
                    }
                    (true, _) => Choice::File { reason: None },
                    (false, _) => keychain::choose(mode, &OsKeychain).map_err(reject)?,
                };
                Ok(match choice {
                    Choice::Keychain => Vault {
                        backend: Backend::Keychain {
                            kc: Box::new(OsKeychain),
                            env_id,
                        },
                        reason: None,
                    },
                    Choice::File { reason } => Vault {
                        backend: Backend::File(Box::new(
                            FileStore::open(Path::new(&server_data), &env_id, key)
                                .map_err(reject)?,
                        )),
                        reason,
                    },
                })
            },
            || {},
        )
    }

    #[napi]
    pub fn backend(&self) -> VaultBackend {
        let kind = match &self.backend {
            Backend::Keychain { .. } => "keychain",
            Backend::File(f) if f.wrapped() => "fileWrapped",
            Backend::File(_) => "file",
        };
        VaultBackend {
            kind: kind.to_owned(),
            reason: self.reason.map(str::to_owned),
        }
    }

    #[napi(catch_unwind)]
    pub fn get(&self, name: String) -> Result<Option<Buffer>> {
        guarded(
            || Ok(self.get_inner(&name).map_err(reject)?.map(Buffer::from)),
            || {},
        )
    }

    #[napi(catch_unwind)]
    pub fn set(&self, name: String, value: Buffer) -> Result<()> {
        guarded(|| self.set_inner(&name, &value).map_err(reject), || {})
    }

    #[napi(catch_unwind)]
    pub fn delete(&self, name: String) -> Result<bool> {
        guarded(
            || match &self.backend {
                Backend::File(f) => f.delete(&name).map_err(reject),
                Backend::Keychain { kc, env_id } => {
                    if !file::valid_name(&name) {
                        return Err(reject(file::invalid_name(&name)));
                    }
                    kc.delete(&keychain::account(env_id, &name))
                        .map_err(|e| reject(keychain::unavailable(&e)))
                }
            },
            || {},
        )
    }
}

/// The identity key as JS sees it: the PEM (for the TLS context only), its pin, made now or not.
#[napi(object)]
pub struct IdentityKey {
    pub pem: String,
    pub spki: String,
    pub created: bool,
}

const IDENTITY: &str = "identity";

/// The vault's identity key (Decision 5), or `None` when there is none; a key whose pin is
/// not `expected_spki` (what `control.db` recorded) is `IDENTITY_MISMATCH`. It never makes one:
/// `generateIdentityKey` does, and the caller stores it once `control.db` records its pin.
#[napi(catch_unwind)]
pub fn identity_key(vault: &Vault, expected_spki: Option<String>) -> Result<Option<IdentityKey>> {
    guarded(
        || {
            let id = identity::load(|| vault.get_inner(IDENTITY), expected_spki.as_deref())
                .map_err(reject)?;
            Ok(id.map(|id| IdentityKey {
                pem: id.pem,
                spki: id.spki,
                created: id.created,
            }))
        },
        || {},
    )
}

/// A new P-256 identity key, stored nowhere yet (`created: true`).
#[napi(catch_unwind)]
pub fn generate_identity_key() -> Result<IdentityKey> {
    guarded(
        || {
            let id = identity::generate().map_err(reject)?;
            Ok(IdentityKey {
                pem: id.pem,
                spki: id.spki,
                created: id.created,
            })
        },
        || {},
    )
}

/// The SPKI pin of a P-256 PKCS#8 PEM (Decision 6).
#[napi(catch_unwind)]
pub fn spki_pin(pem: String) -> Result<String> {
    guarded(|| identity::spki_pin(&pem).map_err(reject), || {})
}
