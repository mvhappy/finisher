//! A registry's package as a plan's source (8A Task 9b): the verified target, the advisories
//! that become plan warnings, the revocations a refresh applies, and a plan from it end to end.

mod support;

use fin_plugin_api::{PluginId, SourceClass, StructuralChecks, ValidateCtx, host_api};
use fin_registry::Store;
use fin_registry::advisory::{Advisory, AdvisoryAction, Severity};
use fin_registry::plan::{CatalogClash, PlanHost, PlanInput, PlanRequest, make_plan};
use fin_registry::registry::{Registry, RegistryConfig, RootSource};
use fin_registry::tuf::sha256_hex;
use serde_json::json;
use support::tuf::Reg;

fn advisory(id: &str, plugin: &str, versions: &str, action: AdvisoryAction) -> Advisory {
    Advisory {
        id: id.into(),
        plugin: plugin.into(),
        versions: versions.into(),
        severity: Severity::High,
        action,
        summary: "test advisory".into(),
        url: None,
    }
}

fn added(r: &Reg) -> Registry {
    let sha = sha256_hex(&r.b.first_root_bytes().unwrap());
    let cfg = RegistryConfig {
        id: "team".into(),
        path: r.repo.clone(),
        root_sha256: sha.clone(),
    };
    let mut reg =
        Registry::add(&r.tmp.path().join("plugins"), cfg, RootSource::Sha256(sha)).unwrap();
    reg.refresh(r.clock.as_ref()).unwrap();
    reg
}

fn id(s: &str) -> PluginId {
    PluginId::parse(s).unwrap()
}

struct NoFonts;

impl PlanHost for NoFonts {
    fn catalog_clash(&self, _: &[Vec<u8>], _: &PluginId) -> Option<CatalogClash> {
        None
    }
}

#[test]
fn verified_target_fetches_the_newest_or_named_version() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    for v in ["1.0.0", "1.1.0", "1.2.0"] {
        r.b.publish(&r.pkg("@acme/pack", v), &acme).unwrap();
    }
    r.b.yank("@acme/pack", &semver::Version::new(1, 2, 0), &acme)
        .unwrap();
    r.b.advise(
        advisory("ACME-1", "@acme/pack", "<1.1.0", AdvisoryAction::Warn),
        &acme,
    )
    .unwrap();
    let reg = added(&r);
    let out = r.tmp.path().join("fetched.finpkg");
    // The newest version not yanked, as signed.
    let t = reg
        .verified_target(&id("@acme/pack"), None, &out, r.clock.as_ref())
        .unwrap();
    assert_eq!(
        (t.registry_id.as_str(), t.custom.version.as_str()),
        ("team", "1.1.0")
    );
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(
        (bytes.len() as u64, sha256_hex(&bytes)),
        (t.length, t.sha256.clone())
    );
    assert!(t.warnings.is_empty());
    assert_eq!(t.stale_days, None);
    // A named version, yanked ones too; its advisories become warnings.
    let old = reg
        .verified_target(
            &id("@acme/pack"),
            Some(&semver::Version::new(1, 0, 0)),
            &out,
            r.clock.as_ref(),
        )
        .unwrap();
    assert_eq!(old.custom.version, "1.0.0");
    assert_eq!(old.warnings.len(), 1);
    assert_eq!(old.warnings[0].code, "advisory");
    assert_eq!(
        serde_json::Value::Object(old.warnings[0].details.clone()),
        json!({ "id": "ACME-1", "severity": "high", "action": "warn", "signedBy": "acme" })
    );
    let yanked = reg
        .verified_target(
            &id("@acme/pack"),
            Some(&semver::Version::new(1, 2, 0)),
            &out,
            r.clock.as_ref(),
        )
        .unwrap();
    assert!(yanked.custom.yanked);
    // Nothing signed under that id or version.
    for (pid, v) in [
        ("@acme/none", None),
        ("@acme/pack", Some(semver::Version::new(9, 0, 0))),
    ] {
        let e = reg
            .verified_target(&id(pid), v.as_ref(), &out, r.clock.as_ref())
            .unwrap_err();
        assert_eq!(e.code, "PLUGIN_NOT_FOUND");
        assert_eq!(e.details["registryId"], json!("team"));
    }
}

#[test]
fn verified_target_refuses_a_swapped_file() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    let info = r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    let reg = added(&r);
    let file = r.repo.join(info.file_path());
    let mut bytes = std::fs::read(&file).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&file, &bytes).unwrap();
    let out = r.tmp.path().join("fetched.finpkg");
    let e = reg
        .verified_target(&id("@acme/pack"), None, &out, r.clock.as_ref())
        .unwrap_err();
    assert_eq!(e.code, "TUF_TARGET_MISMATCH");
    assert!(!out.exists(), "a target that did not verify was written");
}

#[test]
fn revocations_name_disabled_versions_only() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    let beta = r.publisher("beta");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    r.b.publish(&r.pkg("@beta/thing", "1.0.0"), &beta).unwrap();
    r.b.advise(
        advisory("A-1", "@acme/pack", "<1.1.0", AdvisoryAction::Disable),
        &acme,
    )
    .unwrap();
    r.b.advise(
        advisory("A-2", "@acme/pack", "*", AdvisoryAction::Warn),
        &acme,
    )
    .unwrap();
    // A publisher cannot revoke another's plugin.
    r.b.advise(
        advisory("A-3", "@beta/thing", "*", AdvisoryAction::Disable),
        &acme,
    )
    .unwrap();
    let reg = added(&r);
    let installed = |v: &str| {
        vec![
            ("@acme/pack".to_string(), v.to_string()),
            ("@beta/thing".into(), "1.0.0".into()),
            ("@acme/bad".into(), "not semver".into()),
        ]
    };
    assert_eq!(
        reg.revocations(&installed("1.0.0")),
        vec![(
            "@acme/pack".to_string(),
            "1.0.0".to_string(),
            "A-1".to_string()
        )]
    );
    assert!(reg.revocations(&installed("1.1.0")).is_empty());
}

#[test]
fn plan_from_a_registry_target_and_staging_leftovers() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    let reg = added(&r);
    let store = Store::open(&r.tmp.path().join("plugins")).unwrap();
    let out = store.fetch_path();
    assert!(out.starts_with(store.staging_dir()));
    let t = reg
        .verified_target(&id("@acme/pack"), None, &out, r.clock.as_ref())
        .unwrap();
    let ctx = ValidateCtx {
        host_api: host_api(),
        platform: None,
        source: SourceClass::Registry,
        node: None,
        content_checks: &StructuralChecks,
    };
    let req = PlanRequest {
        id: "@acme/pack".into(),
        test_agent_facing: true,
        ..Default::default()
    };
    let plan = make_plan(
        PlanInput::Registry(Box::new(t)),
        &store,
        &req,
        &ctx,
        &NoFonts,
        r.clock.as_ref(),
    )
    .unwrap();
    assert_eq!(plan.source.registry_id.as_deref(), Some("team"));
    assert!(
        plan.agent_facing,
        "the test flag marks the plan agent-facing"
    );
    // A plan nobody approves is discarded; a fetched file left behind is swept with old plans.
    assert!(store.discard(&plan.plan_id));
    assert!(!store.discard(&plan.plan_id));
    assert!(store.plan(&plan.plan_id).is_err());
    assert!(out.exists());
    assert_eq!(store.sweep_staging(0), 0, "a young leftover stays");
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&out)
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert_eq!(store.sweep_staging(0), 1);
    assert!(!out.exists());
}
