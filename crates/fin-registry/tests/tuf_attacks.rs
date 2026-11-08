//! The TUF attack matrix (8A Task 4b, spec §6.2): every attack is refused and leaves the
//! trusted set as it was.

mod support;

use std::sync::Arc;

use fin_registry::editor::{Role, SigningKey, sign_metadata};
use fin_registry::transport::{FsTransport, TransportError};
use fin_registry::tuf::{Policy, TufError};
use support::tuf::{Faulty, Reg};
use tough::TargetName;
use tough::schema::{RoleType, Root, Signed, Target, Timestamp};

/// A registry with `@acme/pack` 1.0.0 published and a client that trusts it.
fn setup() -> (Reg, SigningKey, fin_registry::tuf::TrustedRepo) {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    let mut c = r.client();
    r.refresh(&mut c).unwrap();
    (r, acme, c)
}

/// Refreshes and expects `code` (and `role`, when given), with the trust store unchanged.
fn refused(
    r: &Reg,
    c: &mut fin_registry::tuf::TrustedRepo,
    code: &str,
    role: Option<&str>,
) -> TufError {
    let before = Reg::files(c.store().dir());
    let versions = c.versions();
    let e = r.refresh(c).unwrap_err();
    assert_eq!(e.code, code, "{e}");
    if let Some(role) = role {
        assert_eq!(e.role(), Some(role), "{e}");
    }
    assert_eq!(
        Reg::files(c.store().dir()),
        before,
        "{code} changed the trust store"
    );
    assert_eq!(c.versions(), versions);
    e
}

#[test]
fn tampered_target_refused() {
    let (r, _, c) = setup();
    let info = c.target("acme/pack/1.0.0.finpkg").unwrap();
    let file = r.repo.join(info.file_path());
    let mut b = std::fs::read(&file).unwrap();
    b[10] ^= 0xff;
    std::fs::write(&file, &b).unwrap();
    let out = r.tmp.path().join("out.finpkg");
    let e = c
        .fetch_target(&FsTransport::new(&r.repo), &info.name, &out)
        .unwrap_err();
    assert_eq!((e.code, e.reason()), ("TUF_TARGET_MISMATCH", Some("hash")));
    assert!(!out.exists(), "a refused target was written");
}

#[test]
fn length_cap_enforced() {
    let (r, _, mut c) = setup();
    // Every metadata fetch carries its cap (Decision 15).
    let t = Arc::new(Faulty::new(&r.repo));
    r.refresh_with(&mut c, t.clone(), Policy::default(), r.clock.as_ref())
        .unwrap();
    let mib = 1024 * 1024;
    for (path, max) in t.fetched() {
        let want = if path.ends_with(".root.json") || path.ends_with(".targets.json") {
            4 * mib
        } else if path.ends_with("timestamp.json") {
            16 * 1024
        } else if path.ends_with(".snapshot.json") {
            mib
        } else {
            4 * mib
        };
        assert_eq!(max, want, "{path}");
    }
    // A target is read only up to its signed length.
    let info = c.target("acme/pack/1.0.0.finpkg").unwrap();
    let file = r.repo.join(info.file_path());
    let good = std::fs::read(&file).unwrap();
    std::fs::write(&file, [good.as_slice(), &vec![0u8; 1 << 20]].concat()).unwrap();
    let t = Faulty::new(&r.repo);
    let e = c.fetch_target_bytes(&t, &info.name).unwrap_err();
    assert_eq!(
        (e.code, e.reason()),
        ("TUF_TARGET_MISMATCH", Some("length"))
    );
    assert_eq!(t.fetched(), [(info.file_path(), info.length)]);
}

#[test]
fn timestamp_rollback_refused() {
    let (mut r, _, mut c) = setup();
    r.b.set_next_version(&Role::Timestamp, 9);
    r.b.resign_timestamp(30).unwrap();
    r.refresh(&mut c).unwrap();
    r.b.set_next_version(&Role::Timestamp, 8);
    r.b.resign_timestamp(30).unwrap();
    let e = refused(&r, &mut c, "TUF_ROLLBACK", Some("timestamp"));
    assert_eq!(
        (e.details["trusted"].as_u64(), e.details["got"].as_u64()),
        (Some(9), Some(8))
    );
}

#[test]
fn snapshot_rollback_refused() {
    let (mut r, acme, mut c) = setup();
    let sv = c.versions().2.unwrap();
    r.b.set_next_version(&Role::Snapshot, sv - 1);
    r.b.publish(&r.pkg("@acme/pack", "1.1.0"), &acme).unwrap();
    refused(&r, &mut c, "TUF_ROLLBACK", Some("snapshot"));
}

#[test]
fn targets_rollback_refused() {
    let (mut r, _, mut c) = setup();
    let tv = c.versions().3.unwrap();
    r.b.set_next_version(&Role::Targets, tv - 1);
    let targets_key = r.role_keys(RoleType::Targets).remove(0);
    r.b.advise(advisory("A-1", "@acme/pack"), &targets_key)
        .unwrap();
    refused(&r, &mut c, "TUF_ROLLBACK", Some("targets"));
}

#[test]
fn delegated_role_rollback_refused() {
    let (mut r, acme, mut c) = setup();
    r.b.publish(&r.pkg("@acme/pack", "1.1.0"), &acme).unwrap();
    r.refresh(&mut c).unwrap();
    let v = r.b_role_version("acme");
    r.b.set_next_version(&Role::Publisher("acme".into()), v - 1);
    r.b.publish(&r.pkg("@acme/pack", "1.2.0"), &acme).unwrap();
    refused(&r, &mut c, "TUF_ROLLBACK", Some("acme"));
}

#[test]
fn mix_and_match_refused() {
    let (mut r, acme, mut c) = setup();
    let old =
        std::fs::read(r.meta_path(&format!("{}.acme.json", r.b_role_version("acme")))).unwrap();
    r.b.publish(&r.pkg("@acme/pack", "1.1.0"), &acme).unwrap();
    // The new snapshot names acme v2; serve the older, validly signed v1 under that name.
    let v = r.b_role_version("acme");
    std::fs::write(r.meta_path(&format!("{v}.acme.json")), old).unwrap();
    let e = refused(&r, &mut c, "TUF_TARGET_MISMATCH", Some("acme"));
    assert_eq!(e.reason(), Some("hash"));
}

#[test]
fn timestamp_signed_by_revoked_key_refused() {
    let (mut r, _, mut c) = setup();
    let old_key = r.role_keys(RoleType::Timestamp).remove(0);
    r.b.rotate(Role::Timestamp, &[SigningKey::generate()], 1)
        .unwrap();
    r.refresh(&mut c).unwrap();
    // The revoked key signs a newer timestamp naming the current snapshot.
    let mut ts: Signed<Timestamp> = r.meta("timestamp.json");
    ts.signed.version = ts.signed.version.checked_add(5).unwrap();
    std::fs::write(
        r.meta_path("timestamp.json"),
        sign_metadata(ts.signed, &[old_key]),
    )
    .unwrap();
    refused(&r, &mut c, "TUF_SIGNATURE", Some("timestamp"));
}

#[test]
fn root_rotation_needs_the_old_threshold() {
    let (r, _, mut c) = setup();
    let new_key = SigningKey::generate();
    let mut root: Root = r.newest_root().signed;
    root.version = root.version.checked_add(1).unwrap();
    let id: tough::schema::decoded::Decoded<tough::schema::decoded::Hex> =
        hex::decode(new_key.public().key_id()).unwrap().into();
    root.keys.insert(id.clone(), new_key.public().tuf_key());
    root.roles.get_mut(&RoleType::Root).unwrap().keyids = vec![id];
    let v = root.version;
    // Signed by the new root key only: the old threshold is not met.
    std::fs::write(
        r.meta_path(&format!("{v}.root.json")),
        sign_metadata(root, &[new_key]),
    )
    .unwrap();
    refused(&r, &mut c, "TUF_SIGNATURE", Some("root"));
    assert_eq!(c.root_version(), 1);
}

#[test]
fn delegation_escape_refused() {
    let (mut r, acme, mut c) = setup();
    let other = r.publisher("other");
    r.b.publish(&r.pkg("@other/xx", "1.0.0"), &other).unwrap();
    r.refresh(&mut c).unwrap();
    // @acme's role lists a target under other/*, signed by acme's own key.
    let mut role = r.b.role("acme").unwrap().clone();
    let theirs =
        r.b.role("other")
            .unwrap()
            .targets
            .values()
            .next()
            .unwrap()
            .clone();
    role.targets.insert(
        TargetName::new("other/xx/9.0.0.finpkg").unwrap(),
        theirs.clone(),
    );
    r.b.commit_unchecked_role("acme", role, &[acme]).unwrap();
    let e = refused(&r, &mut c, "TUF_DELEGATION", None);
    assert!(e.message.contains("other/xx"), "{e}");
    let _: Target = theirs;
}

#[test]
fn oversized_timestamp_refused() {
    let (r, _, mut c) = setup();
    let p = r.meta_path("timestamp.json");
    let mut b = std::fs::read(&p).unwrap();
    b.extend(std::iter::repeat_n(b' ', 16 * 1024));
    std::fs::write(&p, b).unwrap();
    let e = refused(&r, &mut c, "REGISTRY_INVALID", None);
    assert_eq!(e.reason(), Some("tooLarge"));
}

#[test]
fn at_most_32_new_roots_per_refresh() {
    let (mut r, _, mut c) = setup();
    for _ in 0..32 {
        r.b.rotate(Role::Root, &[SigningKey::generate()], 1)
            .unwrap();
    }
    let rep = r.refresh(&mut c).unwrap();
    assert_eq!((rep.root_version, rep.roots_added), (33, 32));
    for _ in 0..33 {
        r.b.rotate(Role::Root, &[SigningKey::generate()], 1)
            .unwrap();
    }
    let gen_before = c.versions();
    let e = r.refresh(&mut c).unwrap_err();
    assert_eq!(
        (e.code, e.reason()),
        ("REGISTRY_INVALID", Some("tooManyRoots")),
        "{e}"
    );
    assert_eq!(c.versions(), gen_before);
    // The roots verified so far are kept; the next refresh finishes the walk.
    let rep = r.refresh(&mut c).unwrap();
    assert_eq!(rep.root_version, 66);
}

#[test]
fn root_walk_errors_fail_the_refresh() {
    let (mut r, _, mut c) = setup();
    r.b.rotate(Role::Root, &[SigningKey::generate()], 1)
        .unwrap();
    // `tough` alone would read a failure on 3.root.json as "no newer root".
    let mut t = Faulty::new(&r.repo);
    t.fail_at = Some((
        "metadata/3.root.json".into(),
        TransportError::Unreachable {
            path: "metadata/3.root.json".into(),
            reason: "share went away".into(),
        },
    ));
    let e = r
        .refresh_with(&mut c, Arc::new(t), Policy::default(), r.clock.as_ref())
        .unwrap_err();
    assert_eq!(e.code, "REGISTRY_UNREACHABLE", "{e}");
    // Root 2 was verified before the failure and stays trusted; nothing else changed.
    assert!(c.store().dir().join("roots/2.root.json").exists());
    let mut t = Faulty::new(&r.repo);
    t.fail_at = Some((
        "metadata/3.root.json".into(),
        TransportError::TooLarge {
            path: "metadata/3.root.json".into(),
            max: 4 << 20,
        },
    ));
    let e = r
        .refresh_with(&mut c, Arc::new(t), Policy::default(), r.clock.as_ref())
        .unwrap_err();
    assert_eq!((e.code, e.reason()), ("REGISTRY_INVALID", Some("tooLarge")));
    assert_eq!(r.refresh(&mut c).unwrap().root_version, 2);
}

fn advisory(id: &str, plugin: &str) -> fin_registry::advisory::Advisory {
    fin_registry::advisory::Advisory {
        id: id.into(),
        plugin: plugin.into(),
        versions: "*".into(),
        severity: fin_registry::advisory::Severity::High,
        action: fin_registry::advisory::AdvisoryAction::Warn,
        summary: "test".into(),
        url: None,
    }
}

trait RoleVersion {
    fn b_role_version(&self, publisher: &str) -> u64;
}

impl RoleVersion for Reg {
    fn b_role_version(&self, publisher: &str) -> u64 {
        self.b.role(publisher).unwrap().version.get()
    }
}
