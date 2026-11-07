//! The repository editor (8A Task 4a): publishing, immutability, rotation and keys.

mod support;

use fin_registry::editor::{PublicKey, RepoBuilder, Role, SigningKey};
use support::tuf::Reg;

#[test]
fn editor_publish_refuses_existing_version() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    let pkg = r.pkg("@acme/pack", "1.0.0");
    r.b.publish(&pkg, &acme).unwrap();
    let before = Reg::files(&r.repo);
    let e = r.b.publish(&pkg, &acme).unwrap_err();
    assert_eq!(e.code, "PLUGIN_VERSION_EXISTS");
    assert_eq!(e.details["version"], "1.0.0");
    // Different bytes under the same version are refused too, and nothing was written.
    let other = r.tmp.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let changed = support::tuf::content_pkg(&other, "@acme/pack", "1.0.0");
    let e = r.b.publish(&changed, &acme).unwrap_err();
    assert_eq!(e.code, "PLUGIN_VERSION_EXISTS");
    assert_eq!(Reg::files(&r.repo), before);
    // A reopened editor knows the published versions.
    let mut reopened = RepoBuilder::open(&r.repo, &r.keys).unwrap();
    assert_eq!(
        reopened.publish(&pkg, &acme).unwrap_err().code,
        "PLUGIN_VERSION_EXISTS"
    );
    // Re-delegating an existing publisher is a rotation.
    assert_eq!(
        reopened
            .delegate("acme", &SigningKey::generate().public())
            .unwrap_err()
            .code,
        "PUBLISHER_EXISTS"
    );
    // Reserved and malformed publishers are refused.
    assert!(reopened.delegate("targets", &acme.public()).is_err());
    assert!(reopened.delegate("Bad_Name", &acme.public()).is_err());
}

#[test]
fn editor_round_trip_with_rotation() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    let mut c = r.client();
    r.refresh(&mut c).unwrap();

    // Rotate every role, reopening the editor between steps as `fin-plugin` does.
    let new_root = [SigningKey::generate(), SigningKey::generate()];
    r.b.rotate(Role::Root, &new_root, 2).unwrap();
    let mut b = RepoBuilder::open(&r.repo, &r.keys)
        .unwrap()
        .with_clock(r.clock.clone());
    b.rotate(Role::Targets, &[SigningKey::generate()], 1)
        .unwrap();
    b.rotate(Role::Snapshot, &[SigningKey::generate()], 1)
        .unwrap();
    let mut b = RepoBuilder::open(&r.repo, &r.keys)
        .unwrap()
        .with_clock(r.clock.clone());
    b.rotate(Role::Timestamp, &[SigningKey::generate()], 1)
        .unwrap();
    let acme2 = SigningKey::generate();
    b.rotate(
        Role::Publisher("acme".into()),
        std::slice::from_ref(&acme2),
        1,
    )
    .unwrap();
    let pkg = r.pkg("@acme/pack", "1.1.0");
    assert_eq!(
        b.publish(&pkg, &acme).unwrap_err().code,
        "PUBLISHER_NOT_DELEGATED"
    );
    b.publish(&pkg, &acme2).unwrap();
    b.resign_timestamp(3).unwrap();

    let rep = r.refresh(&mut c).unwrap();
    assert_eq!(rep.root_version, 5);
    assert_eq!(rep.roots_added, 4);
    let names: Vec<String> = c.targets().map(|t| t.name).collect();
    assert_eq!(names, ["acme/pack/1.0.0.finpkg", "acme/pack/1.1.0.finpkg"]);
    // The trust store keeps every root exactly as fetched.
    let files = Reg::files(c.store().dir());
    for v in 1..=5 {
        let served = std::fs::read(r.repo.join(format!("metadata/{v}.root.json"))).unwrap();
        assert_eq!(files[&format!("roots/{v}.root.json")], served, "root {v}");
    }
    // A fresh client from 1.root.json walks the same chain.
    let mut fresh = r.client_in(&r.tmp.path().join("fresh"));
    r.refresh(&mut fresh).unwrap();
    assert_eq!(fresh.versions(), c.versions());
}

#[test]
fn keys_round_trip_as_pem() {
    let k = SigningKey::generate();
    let pem = k.to_pem();
    assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----"));
    let back = SigningKey::from_pem(&pem).unwrap();
    assert_eq!(back.public(), k.public());
    let p = PublicKey::from_pem(&k.public().to_pem()).unwrap();
    assert_eq!(p, k.public());
    assert_eq!(PublicKey::from_pem(&pem).unwrap(), k.public());
    // An OpenSSL-style PKCS#8 v1 document (no public key inside) also parses.
    let seed = [7u8; 32];
    let mut v1 = vec![
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ];
    v1.extend_from_slice(&seed);
    let text = pem::encode(&pem::Pem::new("PRIVATE KEY", v1));
    let k1 = SigningKey::from_pem(&text).unwrap();
    assert_eq!(
        SigningKey::from_pem(&k1.to_pem()).unwrap().public(),
        k1.public()
    );
    // Debug shows the key id only.
    let dbg = format!("{k:?}");
    assert!(dbg.contains(&k.public().key_id()) && !dbg.contains(pem.lines().nth(1).unwrap()));
    assert!(SigningKey::from_pem(&k.public().to_pem()).is_err());
}

#[test]
fn key_ring_inside_the_registry_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("registry");
    let k = SigningKey::generate();
    for keys in [repo.join("keys"), repo.join("./x/../keys")] {
        let e = RepoBuilder::init(&repo, &keys, std::slice::from_ref(&k), 1, None).unwrap_err();
        assert_eq!(e.details["reason"], "keysInside");
    }
    assert!(!repo.exists(), "a refused init wrote files");
    RepoBuilder::init(&repo, &tmp.path().join("keys"), &[k], 1, None).unwrap();
    let e = RepoBuilder::open(&repo, &repo.join("metadata")).unwrap_err();
    assert_eq!(e.details["reason"], "keysInside");
}
