//! The vault's keychain backend (5B Decision 4): the OS keychain through `keyring` (macOS
//! Keychain, Windows Credential Manager, the Secret Service on Linux), service `finisher`,
//! account `<envId>:<name>`, and the choice between it and the file store.
//!
//! **Never a prompt, never a blocked start:** in `auto` mode the keychain is probed first
//! (`Keychain::probe`). On Linux that asks the Secret Service whether its default collection is
//! locked without unlocking it, because `keyring` unlocks a locked collection and unlocking
//! prompts; a locked or absent Secret Service, or any error, picks the file store, and
//! `choose` reports why. `keychain` mode insists on the keychain and fails instead. Every later
//! call checks the lock again and runs under a 2 s deadline (`bounded`). Napi-free,
//! so `tests/vault.rs` includes it by path and drives `choose` with a fake.

use fin_core::CoreError;
use serde_json::json;

pub const SERVICE: &str = "finisher";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KcError {
    /// The keychain is there but locked: using it would prompt. Only the Secret Service is
    /// asked (Linux); elsewhere a lock surfaces as the backend's own error.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Locked,
    /// No keychain answers (no Secret Service on a headless host, no session bus).
    Absent(String),
    /// Anything else the backend reported.
    Other(String),
}

impl KcError {
    /// The closed reason word for `server.status` (never the backend's own text).
    pub fn reason(&self) -> &'static str {
        match self {
            KcError::Locked => "keychainLocked",
            KcError::Absent(_) => "keychainAbsent",
            KcError::Other(_) => "keychainError",
        }
    }
}

pub trait Keychain: Send + Sync {
    /// Whether the keychain answers now without asking the person anything.
    fn probe(&self) -> Result<(), KcError>;
    fn get(&self, account: &str) -> Result<Option<Vec<u8>>, KcError>;
    fn set(&self, account: &str, value: &[u8]) -> Result<(), KcError>;
    fn delete(&self, account: &str) -> Result<bool, KcError>;
}

/// `FIN_SECRETS`: `auto` (the default), `keychain` or `file`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Keychain,
    File,
}

impl Mode {
    pub fn parse(s: &str) -> Result<Mode, CoreError> {
        match s {
            "" | "auto" => Ok(Mode::Auto),
            "keychain" => Ok(Mode::Keychain),
            "file" => Ok(Mode::File),
            _ => Err(
                CoreError::new("INVALID_REQUEST", "FIN_SECRETS is auto, keychain or file")
                    .with_details(json!({ "field": "FIN_SECRETS" })),
            ),
        }
    }
}

/// What `choose` picked: the keychain, or the file store and why (`None`: it was asked for).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Keychain,
    File { reason: Option<&'static str> },
}

/// Picks the backend for `mode`, probing `kc` only when the keychain could be used.
pub fn choose(mode: Mode, kc: &dyn Keychain) -> Result<Choice, CoreError> {
    match mode {
        Mode::File => Ok(Choice::File { reason: None }),
        Mode::Auto => Ok(match kc.probe() {
            Ok(()) => Choice::Keychain,
            Err(e) => Choice::File {
                reason: Some(e.reason()),
            },
        }),
        Mode::Keychain => match kc.probe() {
            Ok(()) => Ok(Choice::Keychain),
            Err(e) => Err(unavailable(&e)),
        },
    }
}

/// `SECRET_STORE_UNAVAILABLE` for a keychain that `keychain` mode insisted on, or that failed later.
pub fn unavailable(e: &KcError) -> CoreError {
    CoreError::new("SECRET_STORE_UNAVAILABLE", "the system keychain cannot be used now")
        .with_details(json!({ "reason": e.reason() }))
        .with_fix("Unlock the system keychain, or let the server keep its secrets in files in its private data folder.")
}

/// The keychain account of `name` in `env_id`.
pub fn account(env_id: &str, name: &str) -> String {
    format!("{env_id}:{name}")
}

/// The OS keychain through `keyring`.
pub struct OsKeychain;

fn kc_err(e: keyring::Error) -> KcError {
    match e {
        keyring::Error::NoStorageAccess(inner) => KcError::Absent(inner.to_string()),
        keyring::Error::PlatformFailure(inner) => KcError::Other(inner.to_string()),
        other => KcError::Other(other.to_string()),
    }
}

/// How long one keychain call may take before it counts as unavailable (a hung D-Bus
/// activation, a daemon waiting for a confirmation): the server never blocks on it.
pub const DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// Runs `f` on its own thread and waits at most `DEADLINE` for it; past it the call is
/// `Absent("timeout")` (the thread is left to finish on its own).
fn bounded<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, KcError> + Send + 'static,
) -> Result<T, KcError> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("fin-keychain".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .map_err(|e| KcError::Other(e.to_string()))?;
    rx.recv_timeout(DEADLINE)
        .unwrap_or_else(|_| Err(KcError::Absent("timeout".into())))
}

/// Before every keychain call: on Linux the default collection must be unlocked now (it may
/// have locked since the vault opened), because `keyring` would unlock it, and that prompts.
fn unlocked_now() -> Result<(), KcError> {
    #[cfg(target_os = "linux")]
    linux_unlocked()?;
    Ok(())
}

impl Keychain for OsKeychain {
    fn probe(&self) -> Result<(), KcError> {
        bounded(|| {
            unlocked_now()?;
            // A read of an account no one writes: `NoEntry` is the healthy answer. On Linux it
            // runs only once the default collection is known to be unlocked, so it cannot prompt.
            match keyring::Entry::new(SERVICE, "probe:finisher").and_then(|e| e.get_secret()) {
                Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(kc_err(e)),
            }
        })
    }

    fn get(&self, account: &str) -> Result<Option<Vec<u8>>, KcError> {
        let account = account.to_owned();
        bounded(move || {
            unlocked_now()?;
            match keyring::Entry::new(SERVICE, &account).and_then(|e| e.get_secret()) {
                Ok(v) => Ok(Some(v)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(kc_err(e)),
            }
        })
    }

    fn set(&self, account: &str, value: &[u8]) -> Result<(), KcError> {
        let (account, value) = (account.to_owned(), value.to_vec());
        bounded(move || {
            unlocked_now()?;
            keyring::Entry::new(SERVICE, &account)
                .and_then(|e| e.set_secret(&value))
                .map_err(kc_err)
        })
    }

    fn delete(&self, account: &str) -> Result<bool, KcError> {
        let account = account.to_owned();
        bounded(move || {
            unlocked_now()?;
            match keyring::Entry::new(SERVICE, &account).and_then(|e| e.delete_credential()) {
                Ok(()) => Ok(true),
                Err(keyring::Error::NoEntry) => Ok(false),
                Err(e) => Err(kc_err(e)),
            }
        })
    }
}

/// The Secret Service's default collection, asked whether it is locked: never unlocked here.
#[cfg(target_os = "linux")]
fn linux_unlocked() -> Result<(), KcError> {
    use secret_service::EncryptionType;
    use secret_service::blocking::SecretService;
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none()
        && std::env::var_os("XDG_RUNTIME_DIR").is_none()
    {
        return Err(KcError::Absent("no session bus".into()));
    }
    let ss =
        SecretService::connect(EncryptionType::Dh).map_err(|e| KcError::Absent(e.to_string()))?;
    let collection = ss
        .get_default_collection()
        .map_err(|e| KcError::Absent(e.to_string()))?;
    match collection.is_locked() {
        Ok(false) => Ok(()),
        Ok(true) => Err(KcError::Locked),
        Err(e) => Err(KcError::Other(e.to_string())),
    }
}
