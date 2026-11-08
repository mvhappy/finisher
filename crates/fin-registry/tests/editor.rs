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

/// The key ring is owner-only from creation (spec §5.6): its folder, and every folder above
/// it the editor creates, 0700, whichever write creates it first; every private key 0600.
#[cfg(unix)]
#[test]
fn key_ring_folders_are_0700_and_keys_0600() {
    use fin_registry::editor::KeyRing;
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let pems = |d: &std::path::Path| -> Vec<std::path::PathBuf> {
        std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "pem"))
            .collect()
    };
    let tmp = tempfile::tempdir().unwrap();
    let ring = KeyRing::new(tmp.path().join("new/deeper/keys"));
    let key = SigningKey::generate();
    ring.add(&key).unwrap();
    for d in ["new", "new/deeper", "new/deeper/keys"] {
        assert_eq!(mode(&tmp.path().join(d)), 0o700, "{d}");
    }
    assert_eq!(pems(ring.dir()).len(), 1);

    let mut r = Reg::new(None);
    r.publisher("acme");
    r.b.rotate(Role::Timestamp, &[SigningKey::generate()], 1)
        .unwrap();
    assert_eq!(mode(&r.keys), 0o700, "{}", r.keys.display());
    let keys = pems(&r.keys);
    assert!(keys.len() >= 4, "{keys:?}");
    for p in keys {
        assert_eq!(mode(&p), 0o600, "{}", p.display());
    }

    // A key folder that `delegate` creates first (the pending delegations file) is
    // owner-only too, and so stays when a key is added to it later.
    let fresh = tmp.path().join("fresh/keys");
    let mut b = RepoBuilder::open(&r.repo, &fresh).unwrap();
    b.delegate("other", &SigningKey::generate().public())
        .unwrap();
    for d in [tmp.path().join("fresh"), fresh.clone()] {
        assert_eq!(mode(&d), 0o700, "{}", d.display());
    }
    KeyRing::new(&fresh).add(&SigningKey::generate()).unwrap();
    assert_eq!(mode(&fresh), 0o700);
}

/// A manifest naming a file in another case or normalisation than the package stores it is
/// refused on every OS: APFS and NTFS would find `Styles/Bold.json` as `styles/bold.json`,
/// Linux would not, and a package must validate alike everywhere.
#[test]
fn publish_refuses_a_manifest_file_spelt_otherwise_than_the_package() {
    use fin_plugin_api::PackageFiles;
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    let src = r.tmp.path().join("src");
    let manifest = serde_json::to_vec(&serde_json::json!({
        "manifestVersion": 1,
        "id": "@acme/pack",
        "version": "1.0.0",
        "kind": "content",
        "hostApi": "^1.0",
        "license": "MIT",
        "publisher": { "name": "Test" },
        "contributes": { "captionStyles": [{ "name": "bold", "file": "Styles/Bold.json" }] },
    }))
    .unwrap();
    support::write_tree(
        &src,
        &[
            ("finisher-plugin.json", &manifest),
            ("styles/bold.json", b"{\"weight\": 700}"),
            ("caf\u{e9}.json", b"{}"),
        ],
    );
    let pkg = r.tmp.path().join("pack.finpkg");
    fin_registry::package::pack(&src, &[], &pkg).unwrap();
    let before = Reg::files(&r.repo);
    let e = r.b.publish(&pkg, &acme).unwrap_err();
    assert_eq!(e.code, "PLUGIN_MANIFEST_INVALID", "{e:?}");
    assert_eq!(
        e.details["problems"][0]["details"]["reason"], "fileMissing",
        "{e:?}"
    );
    assert_eq!(Reg::files(&r.repo), before);

    // Both directory views answer by exact spelling.
    let pkg_files = fin_registry::package::DirFiles(&src);
    let plan_files = fin_registry::plan::DirFiles(&src);
    let views: [&dyn PackageFiles; 2] = [&pkg_files, &plan_files];
    for files in views {
        assert!(files.exists("styles/bold.json"));
        assert!(files.read("styles/bold.json").is_some());
        assert!(files.exists("caf\u{e9}.json"));
        for other in [
            "Styles/Bold.json",
            "styles/BOLD.json",
            "STYLES/bold.json",
            "cafe\u{301}.json",
        ] {
            assert!(!files.exists(other), "{other}");
            assert!(files.read(other).is_none(), "{other}");
        }
        assert!(!files.exists("styles"), "a directory is not a file");
    }
}

/// `git` with a fixed identity and no hooks or signing, or `None` when git is missing.
fn git(dir: &std::path::Path, args: &[&str]) -> Option<()> {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=true"])
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(())
}

/// A registry published from git survives a checkout under `core.autocrlf=true` (Git for
/// Windows' default) byte for byte: `init` writes `.gitattributes` with `* -text`. Without it
/// the clone's JSON turns CRLF and the client refuses it (`REGISTRY_INVALID {reason: tooLarge}`,
/// the 2026-11-08 Windows run of the e2e `plugins` scenario).
#[test]
fn init_keeps_git_checkouts_byte_for_byte() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    assert_eq!(
        std::fs::read_to_string(r.repo.join(".gitattributes")).unwrap(),
        fin_registry::editor::GITATTRIBUTES
    );
    if git(&r.repo, &["init", "-q"]).is_none() {
        eprintln!("SKIPPED: git is not installed; the checkout half of this test did not run");
        return;
    }
    git(&r.repo, &["add", "-A"]).unwrap();
    git(&r.repo, &["commit", "-q", "-m", "1.0.0"]).unwrap();
    let clone = r.tmp.path().join("clone");
    let (from, to) = (r.repo.display().to_string(), clone.display().to_string());
    git(r.tmp.path(), &["clone", "-q", &from, &to]).unwrap();
    let mut want = Reg::files(&r.repo);
    want.retain(|k, _| !k.starts_with(".git/"));
    let mut got = Reg::files(&clone);
    got.retain(|k, _| !k.starts_with(".git/"));
    assert_eq!(
        got.keys().collect::<Vec<_>>(),
        want.keys().collect::<Vec<_>>()
    );
    for (k, v) in &want {
        assert!(got[k] == *v, "{k} changed in the checkout");
    }
}

#[test]
fn init_keeps_a_publishers_gitattributes() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("registry");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join(".gitattributes"), "*.json binary\n").unwrap();
    RepoBuilder::init(
        &repo,
        &tmp.path().join("keys"),
        &[SigningKey::generate()],
        1,
        None,
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.join(".gitattributes")).unwrap(),
        "*.json binary\n"
    );
}
