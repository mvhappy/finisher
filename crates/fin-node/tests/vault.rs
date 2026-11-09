//! The vault (5B Task 2, Decisions 4–6): the file store's privacy and wrapping, the keychain
//! choice that never prompts, the identity key and its pin. The crate is a napi `cdylib`, so
//! this includes the napi-free modules by path. Keychain round trips run only with
//! `FIN_TEST_KEYCHAIN=1` (they touch the real OS keychain).

#![allow(dead_code)]
// A temporary server data's `secrets/` folder, never a library path.
#![allow(clippy::disallowed_methods)]

#[path = "../src/vault/file.rs"]
mod file;
#[path = "../src/identity.rs"]
mod identity;
#[path = "../src/vault/keychain.rs"]
mod keychain;

use std::sync::atomic::{AtomicUsize, Ordering};

use file::FileStore;
use keychain::{Choice, KcError, Keychain, Mode, choose};

const ENV: &str = "01envenvenvenvenvenvenvenv";

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn code(e: fin_core::CoreError) -> String {
    e.code.to_string()
}

#[cfg(unix)]
#[test]
fn file_store_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let t = tmp();
    let store = FileStore::open(t.path(), ENV, None).unwrap();
    store.set("identity", b"value").unwrap();
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&t.path().join("secrets")), 0o700);
    assert_eq!(mode(&t.path().join("secrets/identity")), 0o600);
    // A loose mode is tightened at the next open and read.
    std::fs::set_permissions(
        t.path().join("secrets"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::set_permissions(
        t.path().join("secrets/identity"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let store = FileStore::open(t.path(), ENV, None).unwrap();
    assert_eq!(
        store.get("identity").unwrap().as_deref(),
        Some(&b"value"[..])
    );
    assert_eq!(mode(&t.path().join("secrets")), 0o700);
    assert_eq!(mode(&t.path().join("secrets/identity")), 0o600);
    // A link in its place is refused, and no temp file is left behind.
    let other = tmp();
    let names: Vec<_> = std::fs::read_dir(t.path().join("secrets"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("identity")]);
    std::fs::remove_dir_all(t.path().join("secrets")).unwrap();
    std::os::unix::fs::symlink(other.path(), t.path().join("secrets")).unwrap();
    assert_eq!(
        code(FileStore::open(t.path(), ENV, None).err().unwrap()),
        "SECRET_STORE_UNSAFE"
    );
}

#[test]
fn names_are_closed() {
    let t = tmp();
    let store = FileStore::open(t.path(), ENV, None).unwrap();
    for bad in ["", "../x", "Identity", "a/b", &"x".repeat(65)] {
        assert_eq!(
            code(store.set(bad, b"v").unwrap_err()),
            "INVALID_REQUEST",
            "{bad}"
        );
    }
    assert_eq!(store.get("absent").unwrap(), None);
    assert!(!store.delete("absent").unwrap());
    store.set("secrets-dek", b"k").unwrap();
    assert!(store.delete("secrets-dek").unwrap());
    assert_eq!(store.get("secrets-dek").unwrap(), None);
}

#[test]
fn wrapped_file_store_round_trips() {
    let t = tmp();
    let key = [7u8; 32];
    let store = FileStore::open(t.path(), ENV, Some(key)).unwrap();
    assert!(store.wrapped());
    store.set("identity", b"a private key").unwrap();
    // The bytes on disk are not the value.
    let raw = std::fs::read(t.path().join("secrets/identity")).unwrap();
    assert!(!raw.windows(13).any(|w| w == b"a private key"));
    assert_eq!(
        FileStore::open(t.path(), ENV, Some(key))
            .unwrap()
            .get("identity")
            .unwrap()
            .as_deref(),
        Some(&b"a private key"[..])
    );
    // Two writes of one value differ (a fresh nonce each time).
    store.set("identity", b"a private key").unwrap();
    assert_ne!(
        std::fs::read(t.path().join("secrets/identity")).unwrap(),
        raw
    );
}

#[test]
fn wrapped_file_with_wrong_key_is_secrets_key_invalid() {
    let t = tmp();
    FileStore::open(t.path(), ENV, Some([1u8; 32]))
        .unwrap()
        .set("identity", b"v")
        .unwrap();
    let wrong = FileStore::open(t.path(), ENV, Some([2u8; 32])).unwrap();
    assert_eq!(
        code(wrong.get("identity").unwrap_err()),
        "SECRETS_KEY_INVALID"
    );
    // Without a key at all, too.
    let none = FileStore::open(t.path(), ENV, None).unwrap();
    assert_eq!(
        code(none.get("identity").unwrap_err()),
        "SECRETS_KEY_INVALID"
    );
}

#[test]
fn associated_data_binds_name_and_env() {
    let t = tmp();
    let key = [3u8; 32];
    FileStore::open(t.path(), ENV, Some(key))
        .unwrap()
        .set("identity", b"v")
        .unwrap();
    // Renamed: it does not open under another name.
    std::fs::copy(
        t.path().join("secrets/identity"),
        t.path().join("secrets/secrets-dek"),
    )
    .unwrap();
    let store = FileStore::open(t.path(), ENV, Some(key)).unwrap();
    assert_eq!(
        code(store.get("secrets-dek").unwrap_err()),
        "SECRETS_KEY_INVALID"
    );
    // Moved to another environment's server data with the same key: it does not open either.
    let other = FileStore::open(t.path(), "01otherotherotherotherothe", Some(key)).unwrap();
    assert_eq!(
        code(other.get("identity").unwrap_err()),
        "SECRETS_KEY_INVALID"
    );
    assert_eq!(store.get("identity").unwrap().as_deref(), Some(&b"v"[..]));
}

#[test]
fn plain_file_is_wrapped_once_a_key_is_set() {
    let t = tmp();
    FileStore::open(t.path(), ENV, None)
        .unwrap()
        .set("identity", b"plain value")
        .unwrap();
    let store = FileStore::open(t.path(), ENV, Some([4u8; 32])).unwrap();
    assert_eq!(
        store.get("identity").unwrap().as_deref(),
        Some(&b"plain value"[..])
    );
    let raw = std::fs::read(t.path().join("secrets/identity")).unwrap();
    assert!(!raw.windows(11).any(|w| w == b"plain value"));
    assert_eq!(
        store.get("identity").unwrap().as_deref(),
        Some(&b"plain value"[..])
    );
}

#[test]
fn key_file_is_32_bytes_of_base64url() {
    use base64::Engine as _;
    let t = tmp();
    let p = t.path().join("key");
    std::fs::write(
        &p,
        format!(
            "{}\n",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([9u8; 32])
        ),
    )
    .unwrap();
    assert_eq!(file::read_key_file(&p).unwrap(), [9u8; 32]);
    std::fs::write(
        &p,
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([9u8; 16]),
    )
    .unwrap();
    assert_eq!(
        code(file::read_key_file(&p).unwrap_err()),
        "SECRETS_KEY_INVALID"
    );
    std::fs::write(&p, "not base64 at all!").unwrap();
    assert_eq!(
        code(file::read_key_file(&p).unwrap_err()),
        "SECRETS_KEY_INVALID"
    );
    assert_eq!(
        code(file::read_key_file(&t.path().join("missing")).unwrap_err()),
        "SECRETS_KEY_INVALID"
    );
}

/// A keychain that answers `probe` with `answer` and counts every other call (none may happen
/// when it is not chosen: that is where a prompt would come from).
struct Fake {
    answer: Result<(), KcError>,
    calls: AtomicUsize,
}

impl Keychain for Fake {
    fn probe(&self) -> Result<(), KcError> {
        self.answer.clone()
    }
    fn get(&self, _: &str) -> Result<Option<Vec<u8>>, KcError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
    fn set(&self, _: &str, _: &[u8]) -> Result<(), KcError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn delete(&self, _: &str) -> Result<bool, KcError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(false)
    }
}

fn fake(answer: Result<(), KcError>) -> Fake {
    Fake {
        answer,
        calls: AtomicUsize::new(0),
    }
}

#[test]
fn locked_secret_service_falls_back_without_prompt() {
    let kc = fake(Err(KcError::Locked));
    assert_eq!(
        choose(Mode::Auto, &kc).unwrap(),
        Choice::File {
            reason: Some("keychainLocked")
        }
    );
    assert_eq!(kc.calls.load(Ordering::SeqCst), 0);
    let absent = fake(Err(KcError::Absent("no session bus".into())));
    assert_eq!(
        choose(Mode::Auto, &absent).unwrap(),
        Choice::File {
            reason: Some("keychainAbsent")
        }
    );
}

#[test]
fn keychain_error_falls_back() {
    let kc = fake(Err(KcError::Other("backend said no".into())));
    assert_eq!(
        choose(Mode::Auto, &kc).unwrap(),
        Choice::File {
            reason: Some("keychainError")
        }
    );
    // The backend's own words never reach the reason.
    assert_eq!(
        KcError::Other("secret detail".into()).reason(),
        "keychainError"
    );
    // `keychain` mode insists and fails instead; `file` never asks.
    assert_eq!(
        code(choose(Mode::Keychain, &kc).unwrap_err()),
        "SECRET_STORE_UNAVAILABLE"
    );
    assert_eq!(
        choose(Mode::File, &fake(Ok(()))).unwrap(),
        Choice::File { reason: None }
    );
    assert_eq!(choose(Mode::Auto, &fake(Ok(()))).unwrap(), Choice::Keychain);
    assert_eq!(code(Mode::parse("vault").unwrap_err()), "INVALID_REQUEST");
    assert_eq!(Mode::parse("").unwrap(), Mode::Auto);
}

#[test]
fn identity_key_persists() {
    let t = tmp();
    let store = FileStore::open(t.path(), ENV, None).unwrap();
    assert!(
        identity::load(|| store.get("identity"), None)
            .unwrap()
            .is_none()
    );
    let first = identity::generate().unwrap();
    assert!(first.created);
    assert_eq!(first.spki.len(), 43);
    assert!(first.pem.starts_with("-----BEGIN PRIVATE KEY-----\n"));
    store.set("identity", first.pem.as_bytes()).unwrap();
    let again = identity::load(|| store.get("identity"), Some(&first.spki))
        .unwrap()
        .unwrap();
    assert!(!again.created);
    assert_eq!(again.spki, first.spki);
    assert_eq!(again.pem, first.pem);
    assert_eq!(identity::spki_pin(&first.pem).unwrap(), first.spki);
    // Another key, another pin.
    assert_ne!(identity::generate().unwrap().spki, first.spki);
}

#[test]
fn identity_spki_mismatch_refuses_start() {
    let t = tmp();
    let store = FileStore::open(t.path(), ENV, None).unwrap();
    store
        .set("identity", identity::generate().unwrap().pem.as_bytes())
        .unwrap();
    let e = identity::load(|| store.get("identity"), Some(&"A".repeat(43))).unwrap_err();
    assert_eq!(e.code, "IDENTITY_MISMATCH");
    // The error never carries the key.
    let text = serde_json::to_string(&e).unwrap();
    assert!(!text.contains("PRIVATE KEY"));
    // A damaged key is its own error, not a mismatch.
    store.set("identity", b"not a key").unwrap();
    assert_eq!(
        identity::load(|| store.get("identity"), None)
            .unwrap_err()
            .code,
        "SECRET_CORRUPT"
    );
}

#[test]
fn wrapped_header_is_bound() {
    let t = tmp();
    let key = [5u8; 32];
    let store = FileStore::open(t.path(), ENV, Some(key)).unwrap();
    store.set("identity", b"v").unwrap();
    // Flipping the mode byte to plain makes the file damaged, never a plain value.
    let p = t.path().join("secrets/identity");
    let mut raw = std::fs::read(&p).unwrap();
    raw[9] = 0;
    std::fs::write(&p, &raw).unwrap();
    let got = store.get("identity").unwrap();
    assert_ne!(got.as_deref(), Some(&b"v"[..]));
}

#[test]
fn spki_is_the_p256_subject_public_key_info() {
    let pem = identity::generate_pem().unwrap();
    let spki = identity::spki_der(&pem).unwrap();
    // SEQUENCE (0x59 bytes): AlgorithmIdentifier {id-ecPublicKey, prime256v1}, BIT STRING of an uncompressed point.
    assert_eq!(spki.len(), 91);
    assert_eq!(&spki[..2], &[0x30, 0x59]);
    assert_eq!(spki[26], 0x04);
    assert_eq!(
        identity::pin_of_spki(&spki),
        identity::spki_pin(&pem).unwrap()
    );
    assert_eq!(
        identity::spki_pin("not a pem").unwrap_err().code,
        "SECRET_CORRUPT"
    );
}

#[test]
fn no_dbus_feature() {
    // `keyring`'s libdbus backends (AFL-2.1 OR GPL-2.0+) are never built, on any target.
    let out = std::process::Command::new(env!("CARGO"))
        .args([
            "tree",
            "--offline",
            "-p",
            "fin-node",
            "-e",
            "features",
            "--target",
            "all",
            "--prefix",
            "none",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tree = String::from_utf8(out.stdout).unwrap();
    assert!(tree.contains("keyring"), "keyring is in the tree");
    for banned in ["dbus-secret-service", "libdbus-sys", "dbus v"] {
        assert!(!tree.contains(banned), "{banned} is in fin-node's tree");
    }
}

#[test]
#[ignore = "touches the OS keychain: run with FIN_TEST_KEYCHAIN=1 (5B Task 15)"]
fn keychain_round_trips() {
    if std::env::var("FIN_TEST_KEYCHAIN").as_deref() != Ok("1") {
        return;
    }
    let kc = keychain::OsKeychain;
    kc.probe().expect("the keychain answers without a prompt");
    let account = keychain::account("01testtesttesttesttesttest", "round-trip");
    kc.set(&account, b"a value").unwrap();
    assert_eq!(kc.get(&account).unwrap().as_deref(), Some(&b"a value"[..]));
    assert!(kc.delete(&account).unwrap());
    assert_eq!(kc.get(&account).unwrap(), None);
}
