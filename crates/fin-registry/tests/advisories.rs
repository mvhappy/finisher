//! Signed advisories (8A Task 7, spec §6.5).

mod support;

use fin_plugin_api::PluginId;
use fin_registry::advisory::{Advisory, AdvisoryAction, Severity, applicable};
use fin_registry::registry::{Registry, RegistryConfig, RootSource};
use fin_registry::tuf::sha256_hex;
use support::tuf::Reg;
use tough::schema::RoleType;

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

/// `@acme/pack` and `@beta/thing`, each at 1.0.0.
fn two_publishers() -> (
    Reg,
    fin_registry::editor::SigningKey,
    fin_registry::editor::SigningKey,
) {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    let beta = r.publisher("beta");
    r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    r.b.publish(&r.pkg("@beta/thing", "1.0.0"), &beta).unwrap();
    (r, acme, beta)
}

#[test]
fn advisory_from_other_publisher_ignored() {
    let (mut r, acme, beta) = two_publishers();
    r.b.advise(
        advisory("ACME-1", "@beta/thing", "*", AdvisoryAction::Disable),
        &acme,
    )
    .unwrap();
    r.b.advise(
        advisory("ACME-2", "@acme/pack", "*", AdvisoryAction::Warn),
        &acme,
    )
    .unwrap();
    r.b.advise(
        advisory("BETA-1", "@beta/thing", "*", AdvisoryAction::Warn),
        &beta,
    )
    .unwrap();
    let reg = added(&r);
    let mut ids: Vec<String> = reg
        .advisories()
        .into_iter()
        .map(|a| a.advisory.id)
        .collect();
    ids.sort();
    assert_eq!(ids, ["ACME-2", "BETA-1"]);
    let info = reg.info(&PluginId::parse("@beta/thing").unwrap()).unwrap();
    let on_beta: Vec<&str> = info
        .advisories
        .iter()
        .map(|a| a.advisory.id.as_str())
        .collect();
    assert_eq!(on_beta, ["BETA-1"]);
    assert_eq!(info.advisories[0].signed_by, "beta");
}

#[test]
fn registry_wide_advisory_applies() {
    let (mut r, _, _) = two_publishers();
    let targets_key = r.role_keys(RoleType::Targets).remove(0);
    r.b.advise(
        advisory("REG-1", "@beta/thing", "*", AdvisoryAction::Disable),
        &targets_key,
    )
    .unwrap();
    // Replacing by id keeps one entry.
    r.b.advise(
        advisory("REG-1", "@beta/thing", "<2.0.0", AdvisoryAction::Disable),
        &targets_key,
    )
    .unwrap();
    let reg = added(&r);
    let all = reg.advisories();
    assert_eq!(all.len(), 1);
    assert_eq!(
        (all[0].signed_by.as_str(), all[0].advisory.versions.as_str()),
        ("targets", "<2.0.0")
    );
    let info = reg.info(&PluginId::parse("@beta/thing").unwrap()).unwrap();
    assert_eq!(info.advisories[0].advisory.action, AdvisoryAction::Disable);
    assert!(
        reg.info(&PluginId::parse("@acme/pack").unwrap())
            .unwrap()
            .advisories
            .is_empty()
    );
    // A key that is neither a publisher's nor the targets key cannot advise.
    let e =
        r.b.advise(
            advisory("X", "@beta/thing", "*", AdvisoryAction::Warn),
            &fin_registry::editor::SigningKey::generate(),
        )
        .unwrap_err();
    assert_eq!(e.code, "PUBLISHER_NOT_DELEGATED");
}

#[test]
fn advisory_version_ranges() {
    let (mut r, acme, _) = two_publishers();
    for (id, range) in [
        ("R-1", ">=1.0.0, <1.2.0"),
        ("R-2", "=1.2.0"),
        ("R-3", "*"),
        ("R-4", "not a range"),
        ("R-5", "^2"),
    ] {
        r.b.advise(
            advisory(id, "@acme/pack", range, AdvisoryAction::Warn),
            &acme,
        )
        .unwrap();
    }
    let reg = added(&r);
    let all = reg.advisories();
    let hits = |v: &str| {
        let mut ids: Vec<String> = applicable(&all, "@acme/pack", &v.parse().unwrap())
            .into_iter()
            .map(|a| a.advisory.id.clone())
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(hits("1.0.0"), ["R-1", "R-3"]);
    assert_eq!(hits("1.1.9"), ["R-1", "R-3"]);
    assert_eq!(hits("1.2.0"), ["R-2", "R-3"]);
    assert_eq!(hits("2.5.0"), ["R-3", "R-5"]);
    assert!(applicable(&all, "@acme/other", &"1.0.0".parse().unwrap()).is_empty());
}

#[test]
fn tampered_advisories_fail_the_refresh() {
    let (mut r, acme, _) = two_publishers();
    r.b.advise(
        advisory("A-1", "@acme/pack", "*", AdvisoryAction::Disable),
        &acme,
    )
    .unwrap();
    let mut reg = added(&r);
    let t = reg.trusted().target("acme/advisories.json").unwrap();
    let file = r.repo.join(t.file_path());
    std::fs::write(&file, br#"{"advisories":[]}"#).unwrap();
    let e = reg.refresh(r.clock.as_ref()).unwrap_err();
    assert_eq!(e.code, "TUF_TARGET_MISMATCH");
}

fn ids(all: Vec<fin_registry::advisory::SignedAdvisory>) -> Vec<String> {
    let mut v: Vec<String> = all.into_iter().map(|a| a.advisory.id).collect();
    v.sort();
    v
}

fn reason(e: &fin_registry::tuf::TufError) -> Option<&str> {
    e.details.get("reason").and_then(|v| v.as_str())
}

/// The trusted copy of `name` in the store's current generation.
fn kept_copy(reg: &Registry, name: &str) -> std::path::PathBuf {
    let dir = reg.trusted().store().dir();
    let k = std::fs::read_to_string(dir.join("current")).unwrap();
    dir.join(format!("gen-{}", k.trim()))
        .join("kept")
        .join(name)
}

#[test]
fn advisories_come_from_the_trusted_copy() {
    let (mut r, acme, _) = two_publishers();
    r.b.advise(
        advisory("A-1", "@acme/pack", "*", AdvisoryAction::Disable),
        &acme,
    )
    .unwrap();
    let reg = added(&r);
    let base = r.tmp.path().join("plugins");
    // The reviewer's repro: the folder's file tampered after the refresh used to open as no
    // advisories at all. The trusted copy is read instead.
    let t = reg.trusted().target("acme/advisories.json").unwrap();
    std::fs::write(r.repo.join(t.file_path()), br#"{"advisories":[]}"#).unwrap();
    let kept = kept_copy(&reg, "acme/advisories.json");
    drop(reg);
    let reg = Registry::open(&base, "team").unwrap();
    assert_eq!(ids(reg.advisories()), ["A-1"]);
    drop(reg);
    // The trusted copy tampered or missing: REGISTRY_INVALID, never an empty list.
    for bytes in [Some(&br#"{"advisories":[]}"#[..]), Some(b"{"), None] {
        let good = std::fs::read(&kept).unwrap();
        match bytes {
            Some(b) => std::fs::write(&kept, b).unwrap(),
            None => std::fs::remove_file(&kept).unwrap(),
        }
        let e = Registry::open(&base, "team").unwrap_err();
        assert_eq!(
            (e.code, reason(&e)),
            ("REGISTRY_INVALID", Some("advisories")),
            "{e}"
        );
        std::fs::write(&kept, good).unwrap();
    }
    assert_eq!(
        ids(Registry::open(&base, "team").unwrap().advisories()),
        ["A-1"]
    );
}

#[test]
fn unparseable_signed_advisories_fail_the_refresh() {
    let (mut r, acme, _) = two_publishers();
    r.b.advise(
        advisory("A-1", "@acme/pack", "*", AdvisoryAction::Disable),
        &acme,
    )
    .unwrap();
    let mut reg = added(&r);
    let store = reg.trusted().store().dir().to_path_buf();
    let before = support::tuf::Reg::files(&store);
    // The publisher's key signs a file that is no advisories file.
    let bytes = b"{\"advisories\": [{\"id\": 1}]}".to_vec();
    let name = "acme/advisories.json";
    let sha = sha256_hex(&bytes);
    let p = r
        .repo
        .join("targets")
        .join(fin_registry::tuf::custom::consistent_target_path(
            name, &sha,
        ));
    std::fs::write(&p, &bytes).unwrap();
    let mut role = r.b.role("acme").unwrap().clone();
    role.targets.insert(
        tough::TargetName::new(name).unwrap(),
        tough::schema::Target {
            length: bytes.len() as u64,
            hashes: tough::schema::Hashes {
                sha256: hex::decode(&sha).unwrap().into(),
                _extra: Default::default(),
            },
            custom: Default::default(),
            _extra: Default::default(),
        },
    );
    r.b.commit_unchecked_role("acme", role, &[acme]).unwrap();
    let e = reg.refresh(r.clock.as_ref()).unwrap_err();
    assert_eq!(
        (e.code, reason(&e)),
        ("REGISTRY_INVALID", Some("advisories")),
        "{e}"
    );
    assert_eq!(support::tuf::Reg::files(&store), before);
    assert_eq!(ids(reg.advisories()), ["A-1"]);
}

/// Review M1 (Tasks 10 and 12): the editor refuses to sign text that could rewrite a terminal.
#[test]
fn editor_refuses_control_characters_in_an_advisory() {
    let (mut r, acme, _) = two_publishers();
    let mut a = advisory("ACME-9", "@acme/pack", "*", AdvisoryAction::Warn);
    a.summary = "update now\u{1b}[2J".into();
    let e = r.b.advise(a, &acme).unwrap_err();
    assert_eq!(e.code, "REGISTRY_INVALID");
    let mut long = advisory("ACME-9", "@acme/pack", "*", AdvisoryAction::Warn);
    long.summary = "z".repeat(501);
    assert!(r.b.advise(long, &acme).is_err());
    // Nothing was signed.
    assert!(added(&r).advisories().is_empty());
}
