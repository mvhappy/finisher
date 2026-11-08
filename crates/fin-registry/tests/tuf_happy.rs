//! The TUF client workflow over registries the editor builds (8A Task 4a).

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fin_plugin_api::{Manifest, Permission};
use fin_registry::package::unpack;
use fin_registry::target::{TargetCustom, custom_for};
use fin_registry::transport::FsTransport;
use fin_registry::tuf::{
    FixedClock, Policy, TrustStore, TrustedRepo, permissions_digest, sha256_hex,
};
use serde_json::{Value, json};
use support::tuf::Reg;

#[test]
fn refresh_from_root_to_targets() {
    let r = Reg::new(None);
    let mut c = r.client();
    assert_eq!(c.versions(), (1, None, None, None));
    let rep = r.refresh(&mut c).unwrap();
    assert_eq!(
        (
            rep.root_version,
            rep.timestamp_version,
            rep.snapshot_version,
            rep.targets_version
        ),
        (1, 1, 1, 1)
    );
    assert!(rep.changed);
    assert_eq!(c.targets().count(), 0);
    // Nothing new: unchanged, and the store says so.
    let again = r.refresh(&mut c).unwrap();
    assert!(!again.changed);
    // Reopening the store gives the same trusted versions.
    let store = TrustStore::new(&r.trust, c.store().root_sha256()).unwrap();
    assert_eq!(
        TrustedRepo::open(store).unwrap().versions(),
        (1, Some(1), Some(1), Some(1))
    );
}

#[test]
fn delegated_targets_resolve() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    let other = r.publisher("other");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    r.b.publish(&r.pkg("@acme/pack", "1.1.0"), &acme).unwrap();
    r.b.publish(&r.pkg("@other/thing", "0.1.0"), &other)
        .unwrap();
    let mut c = r.client();
    r.refresh(&mut c).unwrap();
    let names: Vec<(String, String)> = c.targets().map(|t| (t.role, t.name)).collect();
    assert_eq!(
        names,
        [
            ("acme".into(), "acme/pack/1.0.0.finpkg".into()),
            ("acme".into(), "acme/pack/1.1.0.finpkg".into()),
            ("other".into(), "other/thing/0.1.0.finpkg".into()),
        ]
    );
    let t = c.target("acme/pack/1.1.0.finpkg").unwrap();
    let p = t.package().unwrap();
    assert_eq!((p.id.as_str(), p.version.as_str()), ("@acme/pack", "1.1.0"));
    // A publisher's key cannot publish another publisher's package.
    let e =
        r.b.publish(&r.pkg("@other/thing", "0.2.0"), &acme)
            .unwrap_err();
    assert_eq!(e.code, "PUBLISHER_NOT_DELEGATED");
}

#[test]
fn custom_metadata_round_trips() {
    let mut r = Reg::new(Some(30));
    let acme = r.publisher("acme");
    let pkg = r.pkg("@acme/pack", "2.0.0");
    let published = r.b.publish(&pkg, &acme).unwrap();
    let mut c = r.client();
    r.refresh(&mut c).unwrap();
    assert_eq!(c.max_stale_days(), 30);
    let t = c.target(&published.name).unwrap();
    assert_eq!(t, published);
    let dir = r.tmp.path().join("unpacked");
    let u = unpack(&pkg, &dir, |m| m.executables(), &Default::default()).unwrap();
    let want = TargetCustom {
        id: "@acme/pack".into(),
        version: "2.0.0".into(),
        kind: fin_plugin_api::PluginKind::Content,
        host_api: "^1.0".into(),
        platforms: vec![],
        license: "MIT".into(),
        permissions_digest: permissions_digest(&[]),
        manifest_sha256: sha256_hex(&u.manifest),
        yanked: false,
        bytes_unpacked: u.files.values().map(|f| f.size).sum(),
    };
    assert_eq!(t.package().unwrap(), want);
    assert_eq!(
        Value::Object(t.custom.clone()),
        json!({
            "id": "@acme/pack", "version": "2.0.0", "kind": "content", "hostApi": "^1.0",
            "license": "MIT", "permissionsDigest": want.permissions_digest,
            "manifestSha256": want.manifest_sha256, "bytesUnpacked": want.bytes_unpacked,
        })
    );
    // What the editor signs is what the install plan checks: `plan::checks` reads the signed
    // custom metadata as `TargetCustom` and compares it with `custom_for` of the package.
    let manifest: Manifest = serde_json::from_slice(&u.manifest).unwrap();
    let signed: TargetCustom = serde_json::from_value(Value::Object(t.custom.clone())).unwrap();
    assert_eq!(
        signed,
        custom_for(&manifest, &u.manifest, want.bytes_unpacked)
    );
    // The root's maxStaleDays defaults to 7.
    let d = Reg::new(None);
    let mut c = d.client();
    d.refresh(&mut c).unwrap();
    assert_eq!(c.max_stale_days(), 7);
}

#[test]
fn fetch_target_checks_length_and_hashes() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    let pkg = r.pkg("@acme/pack", "1.0.0");
    let info = r.b.publish(&pkg, &acme).unwrap();
    let mut c = r.client();
    r.refresh(&mut c).unwrap();
    let fs = FsTransport::new(&r.repo);
    let out = r.tmp.path().join("fetched.finpkg");
    c.fetch_target(&fs, &info.name, &out).unwrap();
    let good = std::fs::read(&pkg).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), good);

    let file = r.repo.join(info.file_path());
    let mut flipped = good.clone();
    let mid = flipped.len() / 2;
    flipped[mid] ^= 1;
    for (bytes, reason) in [
        (flipped, "hash"),
        ([good.as_slice(), b"tail"].concat(), "length"),
        (good[..good.len() - 1].to_vec(), "length"),
    ] {
        std::fs::write(&file, &bytes).unwrap();
        let e = c.fetch_target(&fs, &info.name, &out).unwrap_err();
        assert_eq!((e.code, e.reason()), ("TUF_TARGET_MISMATCH", Some(reason)));
    }
    let e = c
        .fetch_target(&fs, "acme/pack/9.9.9.finpkg", &out)
        .unwrap_err();
    assert_eq!(e.code, "PLUGIN_NOT_FOUND");
}

fn break_file(path: &Path) {
    let mut b = std::fs::read(path).unwrap();
    let i = b.iter().rposition(|c| c.is_ascii_hexdigit()).unwrap();
    b[i] = if b[i] == b'0' { b'1' } else { b'0' };
    std::fs::write(path, b).unwrap();
}

#[test]
fn trusted_files_written_only_after_full_chain() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    let mut c = r.client();
    r.refresh(&mut c).unwrap();
    let trusted = Reg::files(c.store().dir());
    let versions = c.versions();

    // A new timestamp whose snapshot is broken: the timestamp verifies, the snapshot fails.
    r.b.publish(&r.pkg("@acme/pack", "1.1.0"), &acme).unwrap();
    let meta = r.repo.join("metadata");
    let (_, _, sv, tv) = {
        let mut probe = r.client_in(&r.tmp.path().join("probe"));
        r.refresh(&mut probe).unwrap();
        probe.versions()
    };
    let snapshot = meta.join(format!("{}.snapshot.json", sv.unwrap()));
    let good = std::fs::read(&snapshot).unwrap();
    break_file(&snapshot);
    assert!(r.refresh(&mut c).is_err());
    assert_eq!(
        Reg::files(c.store().dir()),
        trusted,
        "a failed snapshot changed trust"
    );
    assert_eq!(c.versions(), versions);
    std::fs::write(&snapshot, good).unwrap();

    // Same with a delegated role broken at the end of the chain.
    let acme_role = std::fs::read_dir(&meta)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".acme.json"))
        .max_by_key(|n| n.split('.').next().unwrap().parse::<u64>().unwrap())
        .unwrap();
    let role = meta.join(acme_role);
    let good = std::fs::read(&role).unwrap();
    break_file(&role);
    let e = r.refresh(&mut c).unwrap_err();
    assert_eq!(e.role(), Some("acme"), "{e}");
    assert_eq!(Reg::files(c.store().dir()), trusted);
    std::fs::write(&role, good).unwrap();

    // Repaired: the whole chain is promoted at once.
    r.refresh(&mut c).unwrap();
    assert_eq!(c.versions().2, sv);
    assert_eq!(c.versions().3, tv);
    assert!(c.target("acme/pack/1.1.0.finpkg").is_some());
    let store = TrustStore::new(&r.trust, c.store().root_sha256()).unwrap();
    assert_eq!(TrustedRepo::open(store).unwrap().versions(), c.versions());
}

#[test]
fn fixture_vectors_verify_identically() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tuf/vectors");
    if std::env::var_os("FIN_REGEN_TUF_VECTORS").is_some() {
        regenerate(&base);
    }
    for name in ["basic", "rotated"] {
        let dir = base.join(name);
        let expected: Value =
            serde_json::from_slice(&std::fs::read(dir.join("expected.json")).unwrap()).unwrap();
        let root = std::fs::read(dir.join("metadata/1.root.json")).unwrap();
        let sha = expected["rootSha256"].as_str().unwrap();
        assert_eq!(sha256_hex(&root), sha, "{name}: root bytes changed");
        let trust = tempfile::tempdir().unwrap();
        let store = TrustStore::new(trust.path(), sha).unwrap();
        let mut c = TrustedRepo::init(store, &root, sha).unwrap();
        let at = UNIX_EPOCH + Duration::from_secs(expected["createdAt"].as_u64().unwrap());
        c.refresh(
            Arc::new(FsTransport::new(&dir)),
            &Policy::default(),
            &FixedClock::new(at),
        )
        .unwrap();
        assert_eq!(listing(&c), expected["trusted"], "{name}");
        // The vectors' packages have no permissions: their signed digest is today's definition
        // (a vector signed with an older one would fail `plan::checks`; regenerate instead).
        for t in c.targets() {
            assert_eq!(
                t.package().unwrap().permissions_digest,
                permissions_digest(&[]),
                "{name}: {} was signed with another permissionsDigest; regenerate the vectors",
                t.name
            );
        }
        // Every trusted root is kept byte for byte as the repository serves it.
        for (rel, bytes) in Reg::files(c.store().dir()) {
            if let Some(file) = rel.strip_prefix("roots/") {
                assert_eq!(
                    bytes,
                    std::fs::read(dir.join("metadata").join(file)).unwrap()
                );
            }
        }
    }
}

/// The vectors' packages were packed on another OS; packing the same trees here must give
/// the same bytes (8A Decision 7), so their signed hashes hold on every OS.
#[test]
fn fixture_packages_repack_to_the_same_bytes() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tuf/vectors");
    let out = tempfile::tempdir().unwrap();
    for (vector, version) in [
        ("basic", "1.0.0"),
        ("rotated", "1.0.0"),
        ("rotated", "1.1.0"),
    ] {
        let pkg = support::tuf::content_pkg(out.path(), "@test-only/pack", version);
        let bytes = std::fs::read(&pkg).unwrap();
        let sha = sha256_hex(&bytes);
        let committed = base.join(format!(
            "{vector}/targets/test-only/pack/{sha}.{version}.finpkg"
        ));
        assert!(
            committed.is_file(),
            "{vector} {version}: packed to {sha}, which is not a committed target"
        );
        assert_eq!(
            std::fs::read(&committed).unwrap(),
            bytes,
            "{vector} {version}"
        );
    }
}

/// `permissionsDigest` is BLAKE3 (hex) over the permissions' canonical JSON (sorted keys),
/// the list sorted and deduplicated. Changing it changes every signed digest: regenerate the
/// fixed vectors (`FIN_REGEN_TUF_VECTORS=1`) in the same change.
#[test]
fn permissions_digest_is_pinned() {
    let b3 = |s: &[u8]| blake3::hash(s).to_hex().to_string();
    assert_eq!(permissions_digest(&[]), b3(b"[]"));
    let perms: Vec<Permission> = serde_json::from_value(json!([
        { "perm": "gpu" },
        { "perm": "fs", "scope": "pluginData", "access": "read" },
        { "perm": "gpu" },
    ]))
    .unwrap();
    let want = b3(br#"[{"access":"read","perm":"fs","scope":"pluginData"},{"perm":"gpu"}]"#);
    assert_eq!(permissions_digest(&perms), want);
    let reversed: Vec<Permission> = perms.iter().rev().cloned().collect();
    assert_eq!(permissions_digest(&reversed), want);
}

fn listing(c: &TrustedRepo) -> Value {
    let (root, ts, snap, targets) = c.versions();
    let t: Vec<Value> = c
        .targets()
        .map(|t| json!({ "name": t.name, "role": t.role, "length": t.length, "sha256": t.sha256 }))
        .collect();
    json!({ "versions": [root, ts, snap, targets], "maxStaleDays": c.max_stale_days(), "targets": t })
}

/// Writes the two `test-only` vectors (keys are generated and thrown away).
fn regenerate(base: &Path) {
    use fin_registry::editor::{RepoBuilder, Role, SigningKey};
    let _ = std::fs::remove_dir_all(base);
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let clock = Arc::new(FixedClock::new(UNIX_EPOCH + Duration::from_secs(created)));
    let keys = tempfile::tempdir().unwrap();
    let pkgs = tempfile::tempdir().unwrap();
    for (name, stale, rotate) in [("basic", None, false), ("rotated", Some(14), true)] {
        let dir = base.join(name);
        let kd = keys.path().join(name);
        let roots = [SigningKey::generate(), SigningKey::generate()];
        let mut b = RepoBuilder::init_with(&dir, &kd, &roots, 2, stale, clock.clone()).unwrap();
        let p = SigningKey::generate();
        b.delegate("test-only", &p.public()).unwrap();
        let pkg = support::tuf::content_pkg(pkgs.path(), "@test-only/pack", "1.0.0");
        b.publish(&pkg, &p).unwrap();
        if rotate {
            b.rotate(Role::Root, &[SigningKey::generate()], 1).unwrap();
            b.rotate(Role::Timestamp, &[SigningKey::generate()], 1)
                .unwrap();
            let p2 = SigningKey::generate();
            b.rotate(
                Role::Publisher("test-only".into()),
                std::slice::from_ref(&p2),
                1,
            )
            .unwrap();
            let pkg = support::tuf::content_pkg(pkgs.path(), "@test-only/pack", "1.1.0");
            b.publish(&pkg, &p2).unwrap();
        }
        let root = b.first_root_bytes().unwrap();
        let sha = sha256_hex(&root);
        let trust = tempfile::tempdir().unwrap();
        let mut c =
            TrustedRepo::init(TrustStore::new(trust.path(), &sha).unwrap(), &root, &sha).unwrap();
        c.refresh(
            Arc::new(FsTransport::new(&dir)),
            &Policy::default(),
            clock.as_ref(),
        )
        .unwrap();
        let expected = json!({ "rootSha256": sha, "createdAt": created, "trusted": listing(&c) });
        let mut bytes = serde_json::to_vec_pretty(&expected).unwrap();
        bytes.push(b'\n');
        std::fs::write(dir.join("expected.json"), bytes).unwrap();
    }
}
