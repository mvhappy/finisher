//! The plugin store (8A plan Task 8): commit re-checks the staged plan and tree, installs
//! read-only and runs nothing; remove and the staging sweep.

mod support;

use std::fs;

use fin_plugin_api::PluginId;
use fin_registry::plan::{Accept, PLAN_TTL_MS, SourceKind};
use serde_json::json;
use support::plans::*;
use support::{listing, write_tree};

const OK: Accept = Accept {
    copyleft: false,
    third_party_code: false,
};

#[test]
fn install_rechecks_staging_hash() {
    let fx = Fx::new();
    let t = fx.target(&content("@acme/pack", "1.0.0"), &default_files());
    let plan = fx.plan_target(t.clone()).unwrap();

    // A wrong digest is refused and the plan stays.
    let e = fx
        .store
        .commit(&plan.plan_id, &"0".repeat(64), &OK, NOW)
        .unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.reason()),
        ("PLAN_MISMATCH", Some("digest"))
    );
    // A staged file changed after planning: refused, and nothing reaches the store.
    let staged = fx.store.staging_dir().join(&plan.plan_id);
    fs::write(staged.join("pkg/styles/bold.json"), b"{\"text\": 1}").unwrap();
    let e = fx
        .store
        .commit(&plan.plan_id, &plan.digest, &OK, NOW)
        .unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.reason()),
        ("PLAN_MISMATCH", Some("tree"))
    );
    assert!(fx.store.installed_manifests().is_empty());
    assert!(listing(&fx.plugins().join("store/@acme/pack")).is_empty());
    assert!(listing(&fx.store.staging_dir()).is_empty());

    // An added file is a change too.
    let plan = fx.plan_target(t.clone()).unwrap();
    let staged = fx.store.staging_dir().join(&plan.plan_id);
    fs::write(staged.join("pkg/extra.txt"), b"x").unwrap();
    let e = fx
        .store
        .commit(&plan.plan_id, &plan.digest, &OK, NOW)
        .unwrap_err();
    assert_eq!(e.reason(), Some("tree"));

    // The plan itself edited (a widened permission, its digest recomputed by hand or not).
    let plan = fx.plan_target(t.clone()).unwrap();
    let path = fx.store.staging_dir().join(&plan.plan_id).join("plan.json");
    let mut v: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    v["risk"] = json!("high");
    fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    let e = fx
        .store
        .commit(&plan.plan_id, &plan.digest, &OK, NOW)
        .unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.reason()),
        ("PLAN_MISMATCH", Some("plan"))
    );

    // Unknown and malformed plan ids.
    for id in ["p000000000000000000000000", "../../etc", ""] {
        let e = fx.store.commit(id, &plan.digest, &OK, NOW).unwrap_err();
        assert_eq!(e.code, "PLAN_EXPIRED", "{id}");
    }

    // Untouched, it installs.
    let plan = fx.plan_target(t).unwrap();
    let got = fx
        .store
        .commit(&plan.plan_id, &plan.digest, &OK, NOW)
        .unwrap();
    assert_eq!(
        got.dir,
        format!("store/@acme/pack/1.0.0-{}", &plan.tree_hash[..12])
    );
    assert_eq!(
        (got.source, got.registry_id.as_deref()),
        (SourceKind::Registry, Some("team"))
    );
    assert_eq!(
        fin_registry::package::tree_hash(&fx.plugins().join(&got.dir)).unwrap(),
        plan.tree_hash
    );
    assert_eq!(fx.store.installed_manifests().len(), 1);
    // A plan is used once.
    let e = fx
        .store
        .commit(&plan.plan_id, &plan.digest, &OK, NOW)
        .unwrap_err();
    assert_eq!(e.code, "PLAN_EXPIRED");
}

#[test]
fn commit_needs_acceptances_and_a_live_plan() {
    let fx = Fx::new();
    let mut m = content("@acme/pack", "1.0.0");
    m["license"] = json!("GPL-3.0-only");
    let plan = fx.plan_target(fx.target(&m, &default_files())).unwrap();
    let e = fx
        .store
        .commit(&plan.plan_id, &plan.digest, &OK, NOW)
        .unwrap_err();
    assert_eq!(e.code, "COPYLEFT_NOT_ACCEPTED");
    let accept = Accept {
        copyleft: true,
        ..OK
    };
    fx.store
        .commit(&plan.plan_id, &plan.digest, &accept, NOW)
        .unwrap();

    let t = fx.target(&code("@acme/tool", "1.0.0", json!([])), &default_files());
    let plan = fx.plan_target(t.clone()).unwrap();
    let e = fx
        .store
        .commit(&plan.plan_id, &plan.digest, &OK, NOW)
        .unwrap_err();
    assert_eq!(e.code, "THIRD_PARTY_CODE_NOT_ACCEPTED");
    let accept = Accept {
        third_party_code: true,
        ..OK
    };
    let late = plan.expires_at;
    let e = fx
        .store
        .commit(&plan.plan_id, &plan.digest, &accept, late)
        .unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.reason()),
        ("PLAN_EXPIRED", Some("expired"))
    );
    assert!(listing(&fx.store.staging_dir()).is_empty());
    let plan = fx.plan_target(t).unwrap();
    fx.store
        .commit(&plan.plan_id, &plan.digest, &accept, NOW)
        .unwrap();
}

#[cfg(unix)]
#[test]
fn install_spawns_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::new();
    let marker = fx.tmp.path().join("ran");
    let mut files = default_files();
    files[2].1 = tool_script(&marker);
    // Only the manifest's executable gets an execute bit.
    let t = fx.target(&code("@acme/tool", "1.0.0", json!([])), &files);
    let plan = fx.plan_target(t).unwrap();
    let accept = Accept {
        third_party_code: true,
        ..OK
    };
    let got = fx
        .store
        .commit(&plan.plan_id, &plan.digest, &accept, NOW)
        .unwrap();
    assert!(!marker.exists(), "something ran the plugin's executable");
    let dir = fx.plugins().join(&got.dir);
    let mode = |p: &str| fs::metadata(dir.join(p)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode("bin/tool"), 0o555);
    assert_eq!(mode("styles/bold.json"), 0o444);
    assert_eq!(mode("finisher-plugin.json"), 0o444);
}

#[test]
fn store_files_are_read_only() {
    let fx = Fx::new();
    let plan = fx
        .plan_target(fx.target(&content("@acme/pack", "1.0.0"), &default_files()))
        .unwrap();
    let got = fx
        .store
        .commit(&plan.plan_id, &plan.digest, &OK, NOW)
        .unwrap();
    let dir = fx.plugins().join(&got.dir);
    let all = files_under(&dir);
    assert_eq!(all.len(), 4, "{all:?}");
    for f in all {
        let meta = fs::metadata(dir.join(&f)).unwrap();
        assert!(meta.permissions().readonly(), "{}", f.display());
    }
}

#[test]
fn remove_keeps_data_unless_purged() {
    let fx = Fx::new();
    let id = PluginId::parse("@acme/pack").unwrap();
    let v1 = semver::Version::new(1, 0, 0);
    let install = |version: &str| {
        let t = fx.target(&content("@acme/pack", version), &default_files());
        let plan = fx.plan_target(t).unwrap();
        fx.store
            .commit(&plan.plan_id, &plan.digest, &OK, NOW)
            .unwrap()
    };
    install("1.0.0");
    install("1.1.0");
    let data = fx.store.data_dir(&id).join("_");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("state.json"), b"{}").unwrap();

    fx.store.remove(&id, &v1, false).unwrap();
    assert_eq!(fx.store.installed_manifests().len(), 1, "1.1.0 stays");
    assert!(data.join("state.json").exists());
    let e = fx.store.remove(&id, &v1, false).unwrap_err();
    assert_eq!(e.code, "PLUGIN_NOT_INSTALLED");

    fx.store
        .remove(&id, &semver::Version::new(1, 1, 0), true)
        .unwrap();
    assert!(fx.store.installed_manifests().is_empty());
    assert!(!fx.store.data_dir(&id).exists());
    assert!(
        !fx.plugins().join("store/@acme").exists(),
        "empty directories go"
    );
}

#[test]
fn staging_swept_after_expiry() {
    let fx = Fx::new();
    let a = fx
        .plan_target(fx.target(&content("@acme/aa", "1.0.0"), &default_files()))
        .unwrap();
    let b = fx
        .plan_target(fx.target(&content("@acme/bb", "1.0.0"), &default_files()))
        .unwrap();
    // A plan whose plan.json is gone, and a half-built one just started.
    let broken = fx.store.staging_dir().join(&b.plan_id).join("plan.json");
    fs::remove_file(broken).unwrap();
    let building = fx
        .store
        .staging_dir()
        .join(".tmp-p111111111111111111111111");
    fs::create_dir(&building).unwrap();

    assert_eq!(fx.store.sweep_staging(NOW), 1, "only the broken plan");
    assert_eq!(
        listing(&fx.store.staging_dir()),
        [
            ".tmp-p111111111111111111111111".to_string(),
            a.plan_id.clone()
        ]
    );
    assert_eq!(fx.store.sweep_staging(NOW + PLAN_TTL_MS - 1), 0);
    assert_eq!(fx.store.sweep_staging(NOW + PLAN_TTL_MS), 1);
    assert_eq!(
        listing(&fx.store.staging_dir()),
        [".tmp-p111111111111111111111111"]
    );
}

/// The directories under `store/@acme/pack/`.
fn pack_dirs(fx: &Fx) -> Vec<String> {
    let root = fx.plugins().join("store/@acme/pack");
    if root.exists() {
        listing(&root)
    } else {
        Vec::new()
    }
}

#[test]
fn half_deleted_install_is_replaced() {
    let fx = Fx::new();
    let t = fx.target(&content("@acme/pack", "1.0.0"), &default_files());
    let install = || {
        let plan = fx.plan_target(t.clone()).unwrap();
        let got = fx
            .store
            .commit(&plan.plan_id, &plan.digest, &OK, NOW)
            .unwrap();
        (plan, got)
    };
    let (plan, got) = install();
    let dir = fx.plugins().join(&got.dir);
    // A file deleted from the installed tree (the store stays writable to the user).
    fs::remove_file(dir.join("styles/bold.json")).unwrap();
    let (_, again) = install();
    assert_eq!(again.dir, got.dir);
    assert_eq!(
        fin_registry::package::tree_hash(&dir).unwrap(),
        plan.tree_hash
    );
    assert!(dir.join("styles/bold.json").exists());
    assert_eq!(
        pack_dirs(&fx),
        [got.dir.rsplit('/').next().unwrap()],
        "nothing left aside"
    );
    // A changed file is replaced the same way; an intact install is kept as it is.
    let path = dir.join("styles/bold.json");
    let mut perms = fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&path, perms).unwrap();
    fs::write(&path, b"{\"text\": 2}").unwrap();
    install();
    assert_eq!(
        fin_registry::package::tree_hash(&dir).unwrap(),
        plan.tree_hash
    );
    install();
    assert_eq!(pack_dirs(&fx).len(), 1);
}

#[test]
fn remove_hides_the_directory_before_deleting() {
    let fx = Fx::new();
    let id = PluginId::parse("@acme/pack").unwrap();
    let plan = fx
        .plan_target(fx.target(&content("@acme/pack", "1.0.0"), &default_files()))
        .unwrap();
    let got = fx
        .store
        .commit(&plan.plan_id, &plan.digest, &OK, NOW)
        .unwrap();
    let styles = fx.plugins().join(&got.dir).join("styles");
    // A deletion that fails half way (a directory the server cannot write; root can, so the
    // failure only happens for other users).
    let blocked = cfg!(unix) && {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&styles, fs::Permissions::from_mode(0o555)).unwrap();
        }
        fs::write(styles.join("probe"), b"").is_err()
    };
    fx.store
        .remove(&id, &semver::Version::new(1, 0, 0), false)
        .unwrap();
    assert!(
        fx.store.installed_manifests().is_empty(),
        "nothing looks installed"
    );
    let e = fx
        .store
        .remove(&id, &semver::Version::new(1, 0, 0), false)
        .unwrap_err();
    assert_eq!(e.code, "PLUGIN_NOT_INSTALLED");
    let left = pack_dirs(&fx);
    if blocked {
        assert_eq!(left.len(), 1, "{left:?}");
        assert!(left[0].starts_with(".tmp-rm-"), "{left:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let hidden = fx
                .plugins()
                .join("store/@acme/pack")
                .join(&left[0])
                .join("styles");
            fs::set_permissions(&hidden, fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(fx.store.sweep_store(), 1);
    } else {
        assert!(left.is_empty(), "{left:?}");
    }
    assert!(!fx.plugins().join("store/@acme").exists());
}

#[test]
fn sweep_store_deletes_leftovers() {
    let fx = Fx::new();
    let plan = fx
        .plan_target(fx.target(&content("@acme/pack", "1.0.0"), &default_files()))
        .unwrap();
    let got = fx
        .store
        .commit(&plan.plan_id, &plan.digest, &OK, NOW)
        .unwrap();
    let root = fx.plugins().join("store/@acme/pack");
    // A commit interrupted between its renames, a tree moved aside and a failed removal, each
    // with read-only files as the store leaves them.
    for name in [".tmp-p0123456789abcdef01234567", ".tmp-old-x", ".tmp-rm-y"] {
        let d = root.join(name);
        write_tree(
            &d,
            &[("finisher-plugin.json", b"{}"), ("styles/bold.json", b"{}")],
        );
        for f in ["finisher-plugin.json", "styles/bold.json"] {
            let mut p = fs::metadata(d.join(f)).unwrap().permissions();
            p.set_readonly(true);
            fs::set_permissions(d.join(f), p).unwrap();
        }
    }
    // Leftovers in another plugin's directory, and a publisher left with nothing installed.
    write_tree(
        &fx.plugins().join("store/@gone/thing/.tmp-rm-z"),
        &[("a", b"x")],
    );
    assert_eq!(
        fx.store.installed_manifests().len(),
        1,
        "temporary directories are not installed"
    );
    assert_eq!(fx.store.sweep_store(), 4);
    assert_eq!(pack_dirs(&fx), [got.dir.rsplit('/').next().unwrap()]);
    assert!(!fx.plugins().join("store/@gone").exists());
    assert_eq!(fx.store.sweep_store(), 0);
}
