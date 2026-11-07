//! Root chains, key rotation and fast-forward recovery (8A Task 4b, spec §6.2, §6.5).

mod support;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use fin_registry::editor::{Role, SigningKey};
use fin_registry::tuf::{Policy, TrustStore, TrustedRepo};
use support::tuf::{Faulty, Reg};

/// Refreshes through a transport that panics at `path` (a crash at that point).
fn killed_at(r: &Reg, c: TrustedRepo, path: &str) {
    let mut t = Faulty::new(&r.repo);
    t.panic_at = Some(path.into());
    let t = Arc::new(t);
    let mut c = c;
    let hit = catch_unwind(AssertUnwindSafe(|| {
        let _ = c.refresh(t.clone(), &Policy::default(), r.clock.as_ref());
    }));
    assert!(hit.is_err(), "the transport was never asked for {path}");
    drop(c);
}

fn reopen(r: &Reg, root_sha: &str) -> TrustedRepo {
    TrustedRepo::open(TrustStore::new(&r.trust, root_sha).unwrap()).unwrap()
}

#[test]
fn each_root_saved_atomically() {
    let mut r = Reg::new(None);
    let c = r.client();
    let sha = c.store().root_sha256().to_string();
    // 1 → 2 (two keys, threshold 2) → 3 (one key, threshold 1).
    r.b.rotate(
        Role::Root,
        &[SigningKey::generate(), SigningKey::generate()],
        2,
    )
    .unwrap();
    r.b.rotate(Role::Root, &[SigningKey::generate()], 1)
        .unwrap();
    killed_at(&r, c, "metadata/3.root.json");
    // Root 2 was on disk, complete and as served, before root 3 was fetched.
    let roots = Reg::files(&r.trust.join(&sha).join("roots"));
    assert_eq!(
        roots.keys().collect::<Vec<_>>(),
        ["1.root.json", "2.root.json"]
    );
    assert_eq!(
        roots["2.root.json"],
        std::fs::read(r.meta_path("2.root.json")).unwrap()
    );
    // Resumed after the crash: from root 2 to root 3.
    let mut c = reopen(&r, &sha);
    assert_eq!(c.root_version(), 2);
    let rep = r.refresh(&mut c).unwrap();
    assert_eq!((rep.root_version, rep.roots_added), (3, 1));
    assert!(
        !r.trust.join(&sha).join("tmp").exists(),
        "scratch left behind"
    );
}

#[test]
fn timestamp_key_rotation_with_fast_forward_recovery() {
    let mut r = Reg::new(None);
    let mut c = r.client();
    r.b.set_next_version(&Role::Timestamp, 10);
    r.b.resign_timestamp(30).unwrap();
    r.refresh(&mut c).unwrap();
    assert_eq!(c.versions().1, Some(10));
    // Without a key rotation a lower timestamp is a rollback.
    r.b.set_next_version(&Role::Timestamp, 3);
    r.b.resign_timestamp(30).unwrap();
    assert_eq!(r.refresh(&mut c).unwrap_err().code, "TUF_ROLLBACK");
    // A rotation of the timestamp key (root 2) recovers: v3 by the new key is accepted.
    r.b.set_next_version(&Role::Timestamp, 3);
    r.b.rotate(Role::Timestamp, &[SigningKey::generate()], 1)
        .unwrap();
    let rep = r.refresh(&mut c).unwrap();
    assert_eq!((rep.root_version, rep.timestamp_version), (2, 3));

    // The same after a crash between saving the rotating root and the new timestamp: the
    // trusted timestamp and snapshot were dropped with the root, so v2 by the next key is
    // accepted on resume.
    let sha = c.store().root_sha256().to_string();
    r.b.set_next_version(&Role::Timestamp, 2);
    r.b.rotate(Role::Timestamp, &[SigningKey::generate()], 1)
        .unwrap();
    killed_at(&r, c, "metadata/timestamp.json");
    let files = Reg::files(&r.trust.join(&sha));
    assert!(files.contains_key("roots/3.root.json"));
    let trusted_ts = files
        .keys()
        .any(|k| k.starts_with("gen-") && k.ends_with("/timestamp.json"));
    assert!(!trusted_ts, "{:?}", files.keys());
    let mut c = reopen(&r, &sha);
    let rep = r.refresh(&mut c).unwrap();
    assert_eq!((rep.root_version, rep.timestamp_version), (3, 2));
}

#[test]
fn publisher_key_rotation_by_redelegation() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    let mut c = r.client();
    r.refresh(&mut c).unwrap();
    let acme2 = SigningKey::generate();
    r.b.rotate(
        Role::Publisher("acme".into()),
        std::slice::from_ref(&acme2),
        1,
    )
    .unwrap();
    r.refresh(&mut c).unwrap();
    assert!(
        c.target("acme/pack/1.0.0.finpkg").is_some(),
        "rotation dropped a version"
    );
    r.b.publish(&r.pkg("@acme/pack", "1.1.0"), &acme2).unwrap();
    r.refresh(&mut c).unwrap();
    assert!(c.target("acme/pack/1.1.0.finpkg").is_some());
    // A key that does not sign for the publisher is refused by the editor.
    let e =
        r.b.publish(&r.pkg("@acme/pack", "1.2.0"), &acme)
            .unwrap_err();
    assert_eq!(e.code, "PUBLISHER_NOT_DELEGATED");
}

#[test]
fn revoked_publisher_key_cannot_publish() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    let acme2 = SigningKey::generate();
    r.b.rotate(
        Role::Publisher("acme".into()),
        std::slice::from_ref(&acme2),
        1,
    )
    .unwrap();
    let mut c = r.client();
    r.refresh(&mut c).unwrap();
    // The revoked key signs a role with a new version; the registry lists it.
    let mut role = r.b.role("acme").unwrap().clone();
    let t = role.targets.values().next().unwrap().clone();
    role.targets
        .insert(tough::TargetName::new("acme/pack/6.6.6.finpkg").unwrap(), t);
    r.b.commit_unchecked_role("acme", role, &[acme]).unwrap();
    let before = Reg::files(c.store().dir());
    let e = r.refresh(&mut c).unwrap_err();
    assert_eq!((e.code, e.role()), ("TUF_SIGNATURE", Some("acme")), "{e}");
    assert_eq!(Reg::files(c.store().dir()), before);
    assert!(c.target("acme/pack/6.6.6.finpkg").is_none());
}
