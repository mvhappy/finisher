//! Install plans (8A plan Task 8): staging, verification, validation, metadata, warnings and
//! the action against the installed version.

mod support;

use fin_plugin_api::{LicenceClass, Risk};
use fin_registry::plan::{
    InstalledPlugin, PlanAction, PlanInput, PlanRequest, SourceKind, lookalike, plan_digest,
};
use serde_json::json;
use support::listing;
use support::plans::*;

fn codes(p: &fin_registry::plan::InstallPlan) -> Vec<&str> {
    p.warnings.iter().map(|w| w.code.as_str()).collect()
}

#[test]
fn plan_verifies_unpacks_and_validates() {
    let fx = Fx::new();
    let t = fx.target(&content("@acme/pack", "1.0.0"), &default_files());
    let plan = fx.plan_target(t.clone()).unwrap();
    assert_eq!(plan.plugin.id, "@acme/pack");
    assert_eq!(plan.plugin.publisher, "Acme Studio");
    assert_eq!(plan.action, PlanAction::Install);
    assert_eq!(plan.source.kind, SourceKind::Registry);
    assert_eq!(plan.source.registry_id.as_deref(), Some("team"));
    assert_eq!(plan.risk, Risk::Low);
    assert_eq!(plan.target_sha256.as_deref(), Some(t.sha256.as_str()));
    assert_eq!(plan.expires_at, NOW + 10 * 60 * 1000);
    assert_eq!(plan.digest, plan_digest(&plan));
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    // Staged: the plan and the unpacked tree, nothing else.
    let staged = fx.store.staging_dir().join(&plan.plan_id);
    assert_eq!(
        listing(&fx.store.staging_dir()),
        std::slice::from_ref(&plan.plan_id)
    );
    assert_eq!(listing(&staged), ["pkg", "plan.json"]);
    assert_eq!(fx.store.plan(&plan.plan_id).unwrap(), plan);
    assert_eq!(
        fin_registry::package::tree_hash(&staged.join("pkg")).unwrap(),
        plan.tree_hash
    );

    // A target whose bytes are not the signed ones, longer or shorter, is refused and leaves
    // nothing staged.
    let mut bad = t.clone();
    bad.sha256 = "0".repeat(64);
    let e = fx.plan_target(bad).unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.reason()),
        ("TUF_TARGET_MISMATCH", Some("hash"))
    );
    for delta in [-1i64, 1] {
        let mut bad = t.clone();
        bad.length = (bad.length as i64 + delta) as u64;
        let e = fx.plan_target(bad).unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.reason()),
            ("TUF_TARGET_MISMATCH", Some("length"))
        );
    }
    // A manifest that does not validate (a content pack with a permission).
    let mut m = content("@acme/pack", "1.0.1");
    m["permissions"] = json!([{ "perm": "net", "origins": ["https://x.example.org"] }]);
    let e = fx.plan_target(fx.target(&m, &default_files())).unwrap_err();
    assert_eq!(e.code, "PLUGIN_MANIFEST_INVALID");
    // The package must be the plugin asked for.
    let t2 = fx.target(&content("@acme/other", "1.0.0"), &default_files());
    let req = PlanRequest {
        id: "@acme/pack".into(),
        ..Default::default()
    };
    let e = fx
        .plan(PlanInput::Registry(Box::new(t2)), &req)
        .unwrap_err();
    assert_eq!(e.reason(), Some("idMismatch"));
    assert_eq!(
        listing(&fx.store.staging_dir()),
        std::slice::from_ref(&plan.plan_id)
    );
}

#[test]
fn plan_manifest_must_match_custom_metadata() {
    let fx = Fx::new();
    let base = fx.target(&content("@acme/pack", "1.0.0"), &default_files());
    type Edit = Box<dyn Fn(&mut fin_registry::target::TargetCustom)>;
    let cases: Vec<(&str, Edit)> = vec![
        ("id", Box::new(|c| c.id = "@acme/other".into())),
        ("version", Box::new(|c| c.version = "1.0.1".into())),
        (
            "kind",
            Box::new(|c| c.kind = fin_plugin_api::PluginKind::Code),
        ),
        ("hostApi", Box::new(|c| c.host_api = "^2.0".into())),
        (
            "platforms",
            Box::new(|c| c.platforms = vec![fin_plugin_api::Platform::LinuxX64]),
        ),
        ("license", Box::new(|c| c.license = "Apache-2.0".into())),
        (
            "permissionsDigest",
            Box::new(|c| c.permissions_digest = "0".repeat(64)),
        ),
        (
            "manifestSha256",
            Box::new(|c| c.manifest_sha256 = "0".repeat(64)),
        ),
        ("bytesUnpacked", Box::new(|c| c.bytes_unpacked += 1)),
    ];
    for (field, edit) in cases {
        let mut t = base.clone();
        edit(&mut t.custom);
        let req = PlanRequest {
            id: "@acme/pack".into(),
            ..Default::default()
        };
        let e = fx.plan(PlanInput::Registry(Box::new(t)), &req).unwrap_err();
        assert_eq!(e.code, "PLUGIN_MANIFEST_INVALID", "{field}");
        assert_eq!(e.reason(), Some("metadataMismatch"), "{field}");
        assert_eq!(e.details["field"], json!(field));
    }
    // Permission order is not a mismatch: the digest is order-free.
    let perms = json!([{ "perm": "project:read" }, { "perm": "gpu" }]);
    let mut t = fx.target(&code("@acme/tool", "1.0.0", perms), &default_files());
    let swapped: Vec<fin_plugin_api::Permission> =
        serde_json::from_value(json!([{ "perm": "gpu" }, { "perm": "project:read" }])).unwrap();
    assert_eq!(
        t.custom.permissions_digest,
        fin_registry::target::permissions_digest(&swapped)
    );
    t.custom.permissions_digest = fin_registry::target::permissions_digest(&swapped);
    fx.plan_target(t).unwrap();
}

#[test]
fn plan_warns_on_lookalike_id() {
    let known: Vec<String> = ["@finisher/captions-basic", "@acme/pack", "@acme/modern"]
        .map(String::from)
        .into();
    for (id, similar) in [
        ("@finisher/captions-basik", Some("@finisher/captions-basic")),
        ("@flnisher/captions-basic", Some("@finisher/captions-basic")),
        ("@finisher/captionsbasic", Some("@finisher/captions-basic")),
        ("@acme/pak", Some("@acme/pack")),
        ("@acme/modem", Some("@acme/modern")),
        ("@acme/rnodern", Some("@acme/modern")),
        ("@acme/pack", None),
        ("@other/thing", None),
    ] {
        assert_eq!(lookalike(id, &known), similar, "{id}");
    }
    let fx = Fx::new();
    let t = fx.target(&content("@acme/pak", "1.0.0"), &default_files());
    let req = PlanRequest {
        id: "@acme/pak".into(),
        known_ids: known.clone(),
        ..Default::default()
    };
    let plan = fx.plan(PlanInput::Registry(Box::new(t)), &req).unwrap();
    assert_eq!(codes(&plan), ["lookalikeId"]);
    assert_eq!(plan.warnings[0].details["similar"], json!("@acme/pack"));
}

#[test]
fn plan_flags_copyleft_and_refuses_noncommercial() {
    let fx = Fx::new();
    for (spdx, class, warning) in [
        ("GPL-3.0-only", LicenceClass::Copyleft, Some("copyleft")),
        ("MPL-2.0", LicenceClass::WeakCopyleft, Some("copyleft")),
        ("MIT OR GPL-3.0-only", LicenceClass::Permissive, None),
        (
            "LicenseRef-acme",
            LicenceClass::Unknown,
            Some("licenceUnknown"),
        ),
    ] {
        let mut m = content("@acme/pack", "1.0.0");
        m["license"] = json!(spdx);
        let plan = fx.plan_target(fx.target(&m, &default_files())).unwrap();
        assert_eq!(plan.licence.class, class, "{spdx}");
        assert_eq!(codes(&plan).first().copied(), warning, "{spdx}");
    }
    let mut m = content("@acme/pack", "1.0.0");
    m["license"] = json!("CC-BY-NC-4.0");
    let e = fx.plan_target(fx.target(&m, &default_files())).unwrap_err();
    assert_eq!(e.reason(), Some("licenceNonCommercial"));
}

#[test]
fn plan_refuses_incompatible_host_api_platform_and_node() {
    let fx = Fx::new();
    let mut m = content("@acme/pack", "1.0.0");
    m["hostApi"] = json!("^2.0");
    let e = fx.plan_target(fx.target(&m, &default_files())).unwrap_err();
    assert_eq!(e.code, "PLUGIN_HOST_API");

    let mut m = code("@acme/tool", "1.0.0", json!([]));
    m["platforms"] = json!(["win32-x64"]);
    m["runtime"] = json!({ "type": "native", "exec": { "win32-x64": "bin/tool" } });
    let e = fx.plan_target(fx.target(&m, &default_files())).unwrap_err();
    assert_eq!(e.code, "PLUGIN_PLATFORM");

    let mut m = code("@acme/tool", "1.0.0", json!([]));
    m["runtime"] = json!({ "type": "node", "main": "dist/plugin.mjs", "node": ">=99.0" });
    let mut files = default_files();
    files.push(("dist/plugin.mjs", b"export {}".to_vec()));
    let t = fx.target(&m, &files);
    let req = PlanRequest {
        id: "@acme/tool".into(),
        ..Default::default()
    };
    let mut ctx = ctx(fin_plugin_api::SourceClass::Registry);
    ctx.node = Some(semver::Version::new(24, 22, 2));
    let e = fin_registry::plan::make_plan(
        PlanInput::Registry(Box::new(t)),
        &fx.store,
        &req,
        &ctx,
        &fx.host,
        &fin_registry::FixedClock::at_ms(NOW),
    )
    .unwrap_err();
    assert_eq!(e.code, "PLUGIN_NODE_VERSION");
}

#[test]
fn plan_refuses_unconsumable_kind() {
    let fx = Fx::new();
    let mut m = content("@acme/pack", "1.0.0");
    m["contributes"]["skills"] = json!([{ "file": "styles/bold.json" }]);
    let e = fx.plan_target(fx.target(&m, &default_files())).unwrap_err();
    assert_eq!(e.code, "PLUGIN_CONTRIBUTION_UNSUPPORTED");
    assert_eq!(e.details["kind"], json!("skills"));
}

#[test]
fn plan_reports_staleness() {
    let fx = Fx::new();
    let mut t = fx.target(&content("@acme/pack", "1.0.0"), &default_files());
    t.stale_days = Some(3);
    t.custom.yanked = true;
    let plan = fx.plan_target(t).unwrap();
    assert_eq!(plan.registry_stale_days, Some(3));
    assert_eq!(codes(&plan), ["yanked", "registryStale"]);
    assert_eq!(plan.warnings[1].details["days"], json!(3));
}

#[test]
fn third_party_code_is_high_risk_and_warned_first() {
    let fx = Fx::new();
    let mut m = code("@acme/tool", "1.0.0", json!([{ "perm": "project:read" }]));
    m["license"] = json!("GPL-3.0-only");
    let plan = fx.plan_target(fx.target(&m, &default_files())).unwrap();
    assert_eq!(plan.risk, Risk::High);
    assert_eq!(codes(&plan), ["thirdPartyCode", "copyleft"]);
    assert_eq!(plan.warnings[0].details["confined"], json!(false));
    assert_eq!(plan.permissions[0].risk, Risk::Low);
    // Plan 7C Task 7: where the job host applies the `plugin` profile, the warning says so (it
    // stays high risk) and the digest differs.
    let confined = fx
        .plan(
            PlanInput::Registry(Box::new(fx.target(&m, &default_files()))),
            &PlanRequest {
                id: "@acme/tool".into(),
                plugin_confined: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(confined.risk, Risk::High);
    assert_eq!(confined.warnings[0].details["confined"], json!(true));
    assert_ne!(confined.digest, plan.digest);
    // Bundled code is not third-party.
    let src = fx.bundled(
        &code("@finisher/tool", "1.0.0", json!([])),
        &default_files(),
    );
    let req = PlanRequest {
        id: "@finisher/tool".into(),
        ..Default::default()
    };
    let plan = fx.plan(src, &req).unwrap();
    assert_eq!(
        (plan.risk, plan.source.kind),
        (Risk::Low, SourceKind::Bundled)
    );
    assert!(plan.warnings.is_empty());
}

#[test]
fn bundled_tree_must_match_its_hash() {
    let fx = Fx::new();
    let PlanInput::Bundled { dir, .. } =
        fx.bundled(&content("@finisher/pack", "1.0.0"), &default_files())
    else {
        unreachable!()
    };
    let req = PlanRequest {
        id: "@finisher/pack".into(),
        ..Default::default()
    };
    let src = PlanInput::Bundled {
        dir,
        tree_hash: "0".repeat(64),
    };
    let e = fx.plan(src, &req).unwrap_err();
    assert_eq!(e.code, "PLUGIN_BUNDLED_MISMATCH");
    assert!(listing(&fx.store.staging_dir()).is_empty());
}

/// The staged copy is re-hashed against the compiled-in hash: a named executable without its
/// execute bit in the shipped tree stages differently and is refused.
#[cfg(unix)]
#[test]
fn bundled_staged_copy_must_match() {
    let fx = Fx::new();
    let m = code("@finisher/tool", "1.0.0", json!([]));
    let dir = fx.tree(&m, &default_files());
    let tree_hash = fin_registry::package::tree_hash(&dir).unwrap();
    let req = PlanRequest {
        id: "@finisher/tool".into(),
        ..Default::default()
    };
    let e = fx
        .plan(
            PlanInput::Bundled {
                dir: dir.clone(),
                tree_hash,
            },
            &req,
        )
        .unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.reason()),
        ("PLUGIN_BUNDLED_MISMATCH", Some("treeHash"))
    );
    assert!(listing(&fx.store.staging_dir()).is_empty());
    set_exec(&dir.join("bin/tool"));
    let tree_hash = fin_registry::package::tree_hash(&dir).unwrap();
    fx.plan(PlanInput::Bundled { dir, tree_hash }, &req)
        .unwrap();
}

#[test]
fn update_and_downgrade_against_the_installed_version() {
    let fx = Fx::new();
    let installed = |version: &str| InstalledPlugin {
        id: "@acme/tool".into(),
        version: version.into(),
        kind: fin_plugin_api::PluginKind::Code,
        source: SourceKind::Registry,
        registry_id: Some("team".into()),
        permissions: serde_json::from_value(json!([{ "perm": "project:read" }])).unwrap(),
        tree_hash: "0".repeat(64),
        target_sha256: None,
        dir: "store/@acme/tool/x".into(),
    };
    let perms = json!([{ "perm": "project:read" }, { "perm": "net", "origins": ["https://a.example.org"] }]);
    let t = fx.target(&code("@acme/tool", "1.1.0", perms), &default_files());
    let mut req = PlanRequest {
        id: "@acme/tool".into(),
        installed: Some(installed("1.0.0")),
        ..Default::default()
    };
    let e = fx
        .plan(PlanInput::Registry(Box::new(t.clone())), &req)
        .unwrap_err();
    assert_eq!(e.code, "PLUGIN_ALREADY_INSTALLED");
    req.update = true;
    let plan = fx
        .plan(PlanInput::Registry(Box::new(t.clone())), &req)
        .unwrap();
    assert_eq!(
        (plan.action, plan.from.as_deref()),
        (PlanAction::Update, Some("1.0.0"))
    );
    assert_eq!(plan.diff.added.len(), 1);
    assert!(plan.diff.permissions_widen());

    req.installed = Some(installed("1.1.0"));
    assert_eq!(
        fx.plan(PlanInput::Registry(Box::new(t.clone())), &req)
            .unwrap_err()
            .code,
        "PLUGIN_ALREADY_INSTALLED"
    );
    req.installed = Some(installed("2.0.0"));
    let e = fx
        .plan(PlanInput::Registry(Box::new(t.clone())), &req)
        .unwrap_err();
    assert_eq!(e.code, "PLUGIN_DOWNGRADE_REFUSED");
    // A downgrade needs a person naming the exact version.
    req.allow_downgrade = true;
    let e = fx
        .plan(PlanInput::Registry(Box::new(t.clone())), &req)
        .unwrap_err();
    assert_eq!(e.code, "PLUGIN_DOWNGRADE_REFUSED");
    req.version = Some(semver::Version::new(1, 2, 0));
    let e = fx
        .plan(PlanInput::Registry(Box::new(t.clone())), &req)
        .unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.reason()),
        ("PLUGIN_MANIFEST_INVALID", Some("versionMismatch"))
    );
    req.version = Some(semver::Version::new(1, 1, 0));
    let plan = fx
        .plan(PlanInput::Registry(Box::new(t.clone())), &req)
        .unwrap();
    assert_eq!(plan.action, PlanAction::Downgrade);
    // Naming the version alone does not downgrade.
    req.allow_downgrade = false;
    let e = fx.plan(PlanInput::Registry(Box::new(t)), &req).unwrap_err();
    assert_eq!(e.code, "PLUGIN_DOWNGRADE_REFUSED");
}

#[test]
fn catalog_id_taken_refused() {
    let mut fx = Fx::new();
    fx.host.taken.insert("acme-sans".into());
    let t = fx.target(&fonts("@acme/fonts"), &default_files());
    let e = fx.plan_target(t).unwrap_err();
    assert_eq!(e.code, "PLUGIN_MANIFEST_INVALID");
    assert_eq!(e.reason(), Some("catalogIdTaken"));
    assert_eq!(e.details["entry"], json!("acme-sans"));
    assert_eq!(e.details["rule"], json!("catalogIdTaken"));
    // Free ids plan; an id repeated inside the pack is refused too.
    fx.host.taken.clear();
    fx.plan_target(fx.target(&fonts("@acme/fonts"), &default_files()))
        .unwrap();
    let mut files = default_files();
    files[1].1 = br#"{"entries": [{"id": "a"}, {"id": "a"}]}"#.to_vec();
    let e = fx
        .plan_target(fx.target(&fonts("@acme/fonts"), &files))
        .unwrap_err();
    assert_eq!(e.reason(), Some("catalogIdTaken"));
}

/// Review M1: a plan whose registry is not where the installed version came from carries a
/// `registryChanged` warning, and it is part of the digest.
#[test]
fn registry_changed_is_a_warning_in_the_digest() {
    let fx = Fx::new();
    let t = fx.target(&content("@acme/pack", "1.0.0"), &default_files());
    let plain = fx
        .plan(
            PlanInput::Registry(Box::new(t.clone())),
            &PlanRequest {
                id: "@acme/pack".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let changed = fx
        .plan(
            PlanInput::Registry(Box::new(t)),
            &PlanRequest {
                id: "@acme/pack".into(),
                registry_changed: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(plain.warnings.iter().all(|w| w.code != "registryChanged"));
    assert!(changed.warnings.iter().any(|w| w.code == "registryChanged"));
    assert_ne!(plain.digest, changed.digest);
}
