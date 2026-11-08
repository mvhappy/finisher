//! Local-folder registries (8A Task 7): roots at add time, search, yanks, kept trust,
//! staleness and rolled-back folders.

mod support;

use std::path::{Path, PathBuf};

use fin_plugin_api::{PluginId, PluginKind};
use fin_registry::editor::{Role, SigningKey};
use fin_registry::registry::{Registry, RegistryConfig, RootSource};
use fin_registry::search::SearchQuery;
use fin_registry::tuf::{FixedClock, sha256_hex};
use support::tuf::Reg;

fn base(r: &Reg) -> PathBuf {
    r.tmp.path().join("server/plugins")
}

fn cfg(r: &Reg, id: &str, sha: &str) -> RegistryConfig {
    RegistryConfig {
        id: id.into(),
        path: r.repo.clone(),
        root_sha256: sha.into(),
    }
}

fn first_sha(r: &Reg) -> String {
    sha256_hex(&r.b.first_root_bytes().unwrap())
}

/// Adds the registry of `r` as `team`, pinned to `1.root.json`, and refreshes it.
fn added(r: &Reg) -> Registry {
    let sha = first_sha(r);
    let mut reg = Registry::add(
        &base(r),
        cfg(r, "team", &sha),
        RootSource::Sha256(sha.clone()),
    )
    .unwrap();
    reg.refresh(r.clock.as_ref()).unwrap();
    reg
}

fn copy_dir(from: &Path, to: &Path) {
    let _ = std::fs::remove_dir_all(to);
    for (rel, bytes) in Reg::files(from) {
        let p = to.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
}

fn id(s: &str) -> PluginId {
    PluginId::parse(s).unwrap()
}

#[test]
fn registry_add_needs_root() {
    let mut r = Reg::new(None);
    let sha = first_sha(&r);
    let b = base(&r);
    // A hash that is no root of the folder: refused, nothing written.
    let wrong = "0".repeat(64);
    let e = Registry::add(
        &b,
        cfg(&r, "team", &wrong),
        RootSource::Sha256(wrong.clone()),
    )
    .unwrap_err();
    assert_eq!(e.code, "REGISTRY_UNTRUSTED");
    assert!(
        !b.exists(),
        "a refused add wrote {:?}",
        Reg::files(&b).keys()
    );
    // Another registry's root file.
    let other = Reg::new(None);
    let bytes = other.b.first_root_bytes().unwrap();
    let e = Registry::add(
        &b,
        cfg(&r, "team", &sha256_hex(&bytes)),
        RootSource::File(bytes),
    )
    .unwrap_err();
    assert_eq!(e.code, "REGISTRY_UNTRUSTED");
    // The source and the config must name the same root.
    let e =
        Registry::add(&b, cfg(&r, "team", &wrong), RootSource::Sha256(sha.clone())).unwrap_err();
    assert_eq!(e.code, "REGISTRY_UNTRUSTED");
    assert!(!b.exists());
    // Bad ids.
    for bad in ["", "Team", "-x", "a/b"] {
        let e = Registry::add(&b, cfg(&r, bad, &sha), RootSource::Sha256(sha.clone())).unwrap_err();
        assert_eq!(e.code, "REGISTRY_INVALID", "{bad:?}");
    }
    // The console flow: the candidate's hash typed back.
    let cand = Registry::root_candidate(&r.repo).unwrap();
    assert_eq!(
        (cand.sha256.as_str(), cand.version, cand.threshold),
        (sha.as_str(), 1, 1)
    );
    assert_eq!(cand.root_key_ids, [r.root_key.public().key_id()]);
    let mut reg = Registry::add(
        &b,
        cfg(&r, "team", &sha),
        RootSource::Confirmed(cand.sha256),
    )
    .unwrap();
    reg.refresh(r.clock.as_ref()).unwrap();
    assert_eq!(
        Registry::add(&b, cfg(&r, "team", &sha), RootSource::Sha256(sha.clone()))
            .unwrap_err()
            .code,
        "REGISTRY_EXISTS"
    );
    // A newer root of the chain may be pinned (by file); the walk starts there.
    r.b.rotate(Role::Root, &[SigningKey::generate()], 1)
        .unwrap();
    let newer = r.b.root_bytes().unwrap();
    let mut reg2 = Registry::add(
        &b,
        cfg(&r, "team2", &sha256_hex(&newer)),
        RootSource::File(newer),
    )
    .unwrap();
    assert_eq!(reg2.refresh(r.clock.as_ref()).unwrap().root_version, 2);
    assert_eq!(Registry::list(&b), ["team", "team2"]);
}

#[test]
fn search_reads_custom_metadata_only() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    let beta = r.publisher("beta");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    r.b.publish(&r.pkg("@acme/pack", "1.2.0"), &acme).unwrap();
    r.b.publish(&r.pkg("@acme/other", "0.1.0"), &acme).unwrap();
    r.b.publish(&r.pkg("@beta/thing", "3.0.0"), &beta).unwrap();
    // A target whose custom metadata disagrees with its name is not offered.
    let mut role = r.b.role("acme").unwrap().clone();
    let real = role.targets.values().next().unwrap().clone();
    let mut fake = real.clone();
    fake.custom.insert("version".into(), "9.0.0".into());
    role.targets.insert(
        tough::TargetName::new("acme/pack/8.0.0.finpkg").unwrap(),
        fake,
    );
    r.b.commit_unchecked_role("acme", role, &[acme]).unwrap();
    let reg = added(&r);
    // No package byte is read: delete them all.
    std::fs::remove_dir_all(r.repo.join("targets")).unwrap();
    let all = reg.search(&SearchQuery::default());
    let ids: Vec<(&str, &str)> = all
        .iter()
        .map(|l| (l.id.as_str(), l.latest.as_str()))
        .collect();
    assert_eq!(
        ids,
        [
            ("@acme/other", "0.1.0"),
            ("@acme/pack", "1.2.0"),
            ("@beta/thing", "3.0.0")
        ]
    );
    assert_eq!(all[1].versions, ["1.0.0", "1.2.0"]);
    assert_eq!(all[1].registry_id, "team");
    let q = SearchQuery {
        text: Some("PACK".into()),
        kind: Some(PluginKind::Content),
    };
    assert_eq!(reg.search(&q).len(), 1);
    let q = SearchQuery {
        text: None,
        kind: Some(PluginKind::Code),
    };
    assert!(reg.search(&q).is_empty());
    let info = reg.info(&id("@acme/pack")).unwrap();
    let vs: Vec<&str> = info.versions.iter().map(|v| v.version.as_str()).collect();
    assert_eq!(vs, ["1.0.0", "1.2.0"]);
    assert_eq!(info.versions[0].target, "acme/pack/1.0.0.finpkg");
    assert!(reg.info(&id("@acme/missing")).is_none());
}

#[test]
fn yanked_hidden_but_exact_installable() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    r.b.publish(&r.pkg("@acme/pack", "1.1.0"), &acme).unwrap();
    r.b.publish(&r.pkg("@acme/gone", "1.0.0"), &acme).unwrap();
    r.b.yank("@acme/pack", &"1.1.0".parse().unwrap(), &acme)
        .unwrap();
    r.b.yank("@acme/gone", &"1.0.0".parse().unwrap(), &acme)
        .unwrap();
    let reg = added(&r);
    let listed = reg.search(&SearchQuery::default());
    assert_eq!(listed.len(), 1, "a fully yanked plugin is hidden");
    assert_eq!(
        (listed[0].latest.as_str(), listed[0].versions.len()),
        ("1.0.0", 1)
    );
    let latest = reg.resolve(&id("@acme/pack"), None).unwrap();
    assert_eq!((latest.version.as_str(), latest.yanked), ("1.0.0", false));
    let exact = reg
        .resolve(&id("@acme/pack"), Some(&"1.1.0".parse().unwrap()))
        .unwrap();
    assert_eq!((exact.version.as_str(), exact.yanked), ("1.1.0", true));
    assert!(reg.resolve(&id("@acme/gone"), None).is_none());
    assert!(
        reg.resolve(&id("@acme/gone"), Some(&"1.0.0".parse().unwrap()))
            .is_some()
    );
    let info = reg.info(&id("@acme/pack")).unwrap();
    assert_eq!(info.versions.iter().filter(|v| v.yanked).count(), 1);
    // A yanked version stays published: it cannot be re-published.
    let e =
        r.b.publish(&r.pkg("@acme/pack", "1.1.0"), &acme)
            .unwrap_err();
    assert_eq!(e.code, "PLUGIN_VERSION_EXISTS");
}

#[test]
fn remove_and_readd_keeps_trust() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    let old = r.tmp.path().join("old-state");
    copy_dir(&r.repo, &old);
    r.b.publish(&r.pkg("@acme/pack", "1.1.0"), &acme).unwrap();
    let reg = added(&r);
    let sha = first_sha(&r);
    let b = base(&r);
    let trusted = reg.trusted().versions();
    drop(reg);
    Registry::remove(&b, "team").unwrap();
    assert_eq!(
        Registry::remove(&b, "team").unwrap_err().code,
        "REGISTRY_NOT_FOUND"
    );
    assert!(
        b.join("trust")
            .join(&sha)
            .join("roots/1.root.json")
            .exists()
    );
    // The folder goes back to its older state while the registry is gone.
    copy_dir(&old, &r.repo);
    let mut again =
        Registry::add(&b, cfg(&r, "team", &sha), RootSource::Sha256(sha.clone())).unwrap();
    assert_eq!(
        again.trusted().versions(),
        trusted,
        "re-adding did not resume"
    );
    let e = again.refresh(r.clock.as_ref()).unwrap_err();
    assert_eq!(e.code, "TUF_ROLLBACK", "{e}");
    // Forgetting is refused while a registry trusts the root.
    assert_eq!(
        Registry::forget(&b, &sha).unwrap_err().reason(),
        Some("inUse")
    );
}

#[test]
fn forget_deletes_trust() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    let old = r.tmp.path().join("old-state");
    copy_dir(&r.repo, &old);
    r.b.publish(&r.pkg("@acme/pack", "1.1.0"), &acme).unwrap();
    drop(added(&r));
    let sha = first_sha(&r);
    let b = base(&r);
    Registry::remove(&b, "team").unwrap();
    Registry::forget(&b, &sha).unwrap();
    assert!(!b.join("trust").join(&sha).exists());
    // With trust forgotten, the older state is simply what is trusted now.
    copy_dir(&old, &r.repo);
    let mut fresh = Registry::add(&b, cfg(&r, "team", &sha), RootSource::Sha256(sha)).unwrap();
    fresh.refresh(r.clock.as_ref()).unwrap();
    assert!(
        fresh
            .resolve(&id("@acme/pack"), Some(&"1.1.0".parse().unwrap()))
            .is_none()
    );
}

#[test]
fn stale_days_reported() {
    let mut r = Reg::new(None);
    r.b.resign_timestamp(1).unwrap();
    let mut reg = added(&r);
    let clock = FixedClock::new(fin_registry::tuf::Clock::now(r.clock.as_ref()));
    assert_eq!(reg.stale_days(&clock), None);
    clock.advance_days(3);
    assert_eq!(reg.stale_days(&clock), Some(2));
    let rep = reg.refresh(&clock).unwrap();
    assert_eq!(rep.warnings[0].code, "REGISTRY_STALE");
    clock.advance_days(7);
    assert_eq!(reg.refresh(&clock).unwrap_err().code, "TUF_EXPIRED");
    assert_eq!(reg.stale_days(&clock), Some(9));
}

#[test]
fn git_pulled_folder_rollback_refused() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    let before_pull = r.tmp.path().join("before");
    copy_dir(&r.repo, &before_pull);
    let mut reg = added(&r);
    // The publisher pushes; the user pulls (the folder advances) and refreshes.
    r.b.publish(&r.pkg("@acme/pack", "1.1.0"), &acme).unwrap();
    reg.refresh(r.clock.as_ref()).unwrap();
    let trusted = Reg::files(reg.trusted().store().dir());
    // `git reset --hard` to the older commit: the folder is the earlier state again.
    copy_dir(&before_pull, &r.repo);
    let e = reg.refresh(r.clock.as_ref()).unwrap_err();
    assert_eq!(e.code, "TUF_ROLLBACK", "{e}");
    assert_eq!(Reg::files(reg.trusted().store().dir()), trusted);
    assert!(
        reg.resolve(&id("@acme/pack"), Some(&"1.1.0".parse().unwrap()))
            .is_some()
    );
}
