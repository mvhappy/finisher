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

fn current_gen(c: &TrustedRepo) -> u64 {
    std::fs::read_to_string(c.store().dir().join("current"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn generations(c: &TrustedRepo) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(c.store().dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("gen-"))
        .collect();
    v.sort();
    v
}

/// What a crash leaves after `gen-<k+1>` was renamed into place and before `current` named it.
fn crashed_generation(c: &TrustedRepo) -> std::path::PathBuf {
    let dir = c.store().dir();
    let k = current_gen(c);
    let stray = dir.join(format!("gen-{}", k + 1));
    for (rel, bytes) in Reg::files(&dir.join(format!("gen-{k}"))) {
        let p = stray.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
    std::fs::write(stray.join("timestamp.json"), b"half written").unwrap();
    stray
}

#[test]
fn crash_between_generation_and_current() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    let mut c = r.client();
    r.refresh(&mut c).unwrap();
    let k = current_gen(&c);
    // The same client refreshes past the stray generation (it used to fail renaming onto it).
    let stray = crashed_generation(&c);
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    let rep = r.refresh(&mut c).unwrap();
    assert!(rep.changed);
    assert!(c.target("acme/pack/1.0.0.finpkg").is_some());
    assert_eq!(current_gen(&c), k + 2);
    assert!(!stray.exists());
    assert_eq!(generations(&c), [format!("gen-{}", k + 2)]);
    // A client opened after the crash deletes the generation `current` does not name.
    let stray = crashed_generation(&c);
    let sha = c.store().root_sha256().to_string();
    drop(c);
    let mut c = reopen(&r, &sha);
    assert!(!stray.exists(), "open kept {}", stray.display());
    assert!(c.target("acme/pack/1.0.0.finpkg").is_some());
    r.b.publish(&r.pkg("@acme/pack", "1.1.0"), &acme).unwrap();
    r.refresh(&mut c).unwrap();
    assert!(c.target("acme/pack/1.1.0.finpkg").is_some());
    assert_eq!(generations(&c).len(), 1);
}

#[test]
fn concurrent_refreshes_share_one_store() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    let sha = r.client().store().root_sha256().to_string();
    for round in 0..4 {
        r.b.publish(&r.pkg("@acme/pack", &format!("1.{round}.0")), &acme)
            .unwrap();
        let clients: Vec<TrustedRepo> = (0..6).map(|_| reopen(&r, &sha)).collect();
        let reports: Vec<_> = std::thread::scope(|s| {
            let handles: Vec<_> = clients
                .into_iter()
                .map(|mut c| {
                    let r = &r;
                    s.spawn(move || {
                        let rep = r.refresh(&mut c);
                        (rep, c)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (rep, c) in &reports {
            let rep = rep
                .as_ref()
                .unwrap_or_else(|e| panic!("round {round}: {e}"));
            assert_eq!(
                rep.targets_version,
                reports[0].0.as_ref().unwrap().targets_version
            );
            assert!(c.target(&format!("acme/pack/1.{round}.0.finpkg")).is_some());
        }
        let c = &reports[0].1;
        assert_eq!(generations(c), [format!("gen-{}", current_gen(c))]);
        assert!(!c.store().dir().join("tmp").exists(), "scratch left behind");
    }
}
