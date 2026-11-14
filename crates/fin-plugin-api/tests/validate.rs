//! Validation rule families (spec §4.1, §4.2; 8A Decision 4).

mod support;

use std::cell::RefCell;

use fin_plugin_api::{ContentChecks, ContributionKind, Platform, SourceClass, ValidateCtx};
use semver::Version;
use serde_json::{Value, json};
use support::{MemFiles, check, check_with, ctx, ctx_with, fixture, reasons, remove, set};

#[test]
fn content_pack_with_permissions_is_invalid() {
    let v = set(
        fixture("content-pack"),
        "/permissions",
        json!([{ "perm": "project:read" }]),
    );
    assert_eq!(reasons(check(&v)), ["contentPermissions"]);
    let v = set(fixture("content-pack"), "/platforms", json!(["linux-x64"]));
    assert_eq!(reasons(check(&v)), ["contentPlatforms"]);
    let v = set(
        fixture("content-pack"),
        "/runtime",
        json!({ "type": "node", "main": "dist/plugin.mjs", "node": ">=24.22" }),
    );
    assert_eq!(reasons(check(&v)), ["contentRuntime"]);
    let v = set(fixture("content-pack"), "/contributes", json!({}));
    assert_eq!(reasons(check(&v)), ["contentEmpty"]);
}

#[test]
fn code_plugin_needs_runtime_and_platforms() {
    assert!(check(&fixture("code-node")).is_ok());
    let v = remove(fixture("code-node"), "/runtime");
    assert_eq!(reasons(check(&v)), ["codeRuntime"]);
    let v = set(fixture("code-node"), "/platforms", json!([]));
    assert_eq!(reasons(check(&v)), ["codePlatforms"]);
    let v = set(
        fixture("code-node"),
        "/platforms",
        json!(["linux-x64", "linux-x64"]),
    );
    assert_eq!(reasons(check(&v)), ["duplicatePlatform"]);
    let v = set(fixture("code-node"), "/platforms", json!(["linux-riscv64"]));
    assert_eq!(reasons(check(&v)), ["schema"]);
    // A code plugin with nothing to contribute installs (8A Decision 4).
    assert!(
        check(&fixture("code-native"))
            .unwrap()
            .contributes
            .is_empty()
    );
}

#[test]
fn node_runtime_needs_min_version() {
    for ok in [">=24.22", "^24", "24.*", "~24.22.2", ">=24.22, <30"] {
        let v = set(fixture("code-node"), "/runtime/node", json!(ok));
        assert!(check(&v).is_ok(), "{ok}");
    }
    for bad in ["*", "<30", "<=29.9"] {
        let v = set(fixture("code-node"), "/runtime/node", json!(bad));
        assert_eq!(reasons(check(&v)), ["nodeMinVersion"], "{bad}");
    }
    let v = remove(fixture("code-node"), "/runtime/node");
    assert_eq!(reasons(check(&v)), ["schema"]);
    let v = set(
        fixture("code-node"),
        "/runtime/main",
        json!("dist/plugin.js"),
    );
    let files = MemFiles::full().with("dist/plugin.js", b"");
    assert_eq!(
        reasons(check_with(&v, &files, &ctx(SourceClass::Registry))),
        ["nodeMain"]
    );
}

#[test]
fn exec_per_platform_matches_platforms() {
    let v = set(
        fixture("code-native"),
        "/platforms",
        json!(["darwin-arm64"]),
    );
    assert_eq!(reasons(check(&v)), ["execPlatforms"]);
    let v = set(
        fixture("code-native"),
        "/platforms",
        json!(["darwin-arm64", "linux-x64", "win32-x64"]),
    );
    assert_eq!(reasons(check(&v)), ["execPlatforms"]);
    let m = check(&fixture("code-native")).unwrap();
    assert_eq!(
        m.executables(),
        ["bin/darwin-arm64/tools", "bin/linux-x64/tools"]
    );
}

#[test]
fn every_referenced_file_exists() {
    for (fixture_name, file, pointer) in [
        (
            "code-native",
            "bin/linux-x64/tools",
            "/runtime/exec/linux-x64",
        ),
        ("code-native", "i18n/de.json", "/locales/1"),
        ("code-node", "dist/plugin.mjs", "/runtime/main"),
        (
            "content-pack",
            "fonts/catalog.json",
            "/contributes/fontCatalogs/0/file",
        ),
        (
            "content-pack",
            "styles/bold-pop.json",
            "/contributes/captionStyles/0/file",
        ),
    ] {
        let files = MemFiles::full().without(file);
        let errs = check_with(&fixture(fixture_name), &files, &ctx(SourceClass::Registry))
            .expect_err(file);
        assert_eq!(errs.len(), 1, "{file}: {errs:?}");
        assert_eq!(errs[0].reason(), Some("fileMissing"));
        assert_eq!(errs[0].path(), Some(pointer));
    }
    for bad in [
        "../styles/bold-pop.json",
        "/styles/bold-pop.json",
        "styles\\bold-pop.json",
        "C:/styles/bold-pop.json",
        "styles//bold-pop.json",
        "styles/./bold-pop.json",
        "",
    ] {
        let v = set(
            fixture("content-pack"),
            "/contributes/captionStyles/0/file",
            json!(bad),
        );
        assert_eq!(reasons(check(&v)), ["path"], "{bad:?}");
    }
    let v = set(
        fixture("content-pack"),
        "/contributes/captionStyles/0/file",
        json!("finisher-plugin.json"),
    );
    let files = MemFiles::full().with("finisher-plugin.json", b"{}");
    assert_eq!(
        reasons(check_with(&v, &files, &ctx(SourceClass::Registry))),
        ["fileMissing"]
    );
}

#[test]
fn message_keys_in_en_catalogue() {
    let files = MemFiles::full().with("i18n/en.json", br#"{"model.tiny": "x"}"#);
    let errs = check_with(
        &fixture("content-pack"),
        &files,
        &ctx(SourceClass::Registry),
    )
    .unwrap_err();
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].reason(), Some("messageKeyMissing"));
    assert_eq!(errs[0].path(), Some("/contributes/captionStyles/0/title"));
    // Other catalogues may lag behind `en`.
    let files = MemFiles::full().with("i18n/de.json", b"{}");
    assert!(check_with(&fixture("code-native"), &files, &ctx(SourceClass::Registry)).is_ok());
    let v = set(fixture("code-native"), "/locales", json!(["de"]));
    assert_eq!(reasons(check(&v)), ["localeEnMissing"]);
    let v = remove(fixture("content-pack"), "/messages");
    let v = remove(v, "/locales");
    assert_eq!(reasons(check(&v)), ["messagesMissing"]);
    let files = MemFiles::full().with("i18n/en.json", br#"{"style.boldPop": 1}"#);
    assert_eq!(
        reasons(check_with(
            &fixture("content-pack"),
            &files,
            &ctx(SourceClass::Registry)
        )),
        ["messagesCatalogue"]
    );
    let v = set(
        fixture("content-pack"),
        "/contributes/captionStyles/0/title",
        json!("Bold Pop"),
    );
    assert_eq!(reasons(check(&v)), ["messageKey"]);
    let v = set(fixture("content-pack"), "/locales", json!(["en", "EN_us"]));
    let r = reasons(check(&v));
    assert!(r.contains(&"locale".to_string()), "{r:?}");
}

#[test]
fn unconsumable_kind_refused() {
    let host = fin_plugin_api::host_api();
    let mut refused = 0;
    for kind in ContributionKind::ALL {
        if fin_plugin_api::consumable(&host).contains(&kind) {
            continue;
        }
        refused += 1;
        let v = set(
            fixture("content-pack"),
            &format!("/contributes/{}", kind.key()),
            json!([]),
        );
        let errs = check(&v).unwrap_err();
        assert_eq!(errs.len(), 1, "{}: {errs:?}", kind.key());
        assert_eq!(errs[0].code, "PLUGIN_CONTRIBUTION_UNSUPPORTED");
        assert_eq!(errs[0].details["kind"], json!(kind.key()));
        assert_eq!(errs[0].details["hostApi"], json!("1.2.0"));
    }
    // Host API 1.2 consumes font catalogs, caption styles, commands (8B Decision 14) and hooks
    // (7F Decision 20).
    assert_eq!(refused, ContributionKind::ALL.len() - 4);
    // A misspelt kind is refused the same way, never ignored.
    let v = set(
        fixture("content-pack"),
        "/contributes/fontCatalog",
        json!([]),
    );
    assert_eq!(reasons(check(&v)), ["PLUGIN_CONTRIBUTION_UNSUPPORTED"]);
    // A host API that does not consume them refuses the 8A kinds too.
    let mut c = ctx(SourceClass::Registry);
    c.host_api = Version::new(2, 0, 0);
    let v = set(fixture("content-pack"), "/hostApi", json!(">=1.0"));
    let codes: Vec<String> = check_with(&v, &MemFiles::full(), &c)
        .unwrap_err()
        .iter()
        .map(|e| e.code.clone())
        .collect();
    assert_eq!(
        codes,
        [
            "PLUGIN_CONTRIBUTION_UNSUPPORTED",
            "PLUGIN_CONTRIBUTION_UNSUPPORTED"
        ]
    );
}

/// Records what it was asked and refuses files containing `bad`.
#[derive(Default)]
struct Recorder {
    fonts: RefCell<Vec<Vec<u8>>>,
    styles: RefCell<Vec<Vec<u8>>>,
}

impl ContentChecks for Recorder {
    fn font_catalog(&self, bytes: &[u8]) -> Result<(), String> {
        self.fonts.borrow_mut().push(bytes.to_vec());
        refuse_bad(bytes)
    }

    fn caption_style(&self, bytes: &[u8]) -> Result<(), String> {
        self.styles.borrow_mut().push(bytes.to_vec());
        refuse_bad(bytes)
    }
}

fn refuse_bad(bytes: &[u8]) -> Result<(), String> {
    if bytes.windows(3).any(|w| w == b"bad") {
        Err("marked bad".into())
    } else {
        Ok(())
    }
}

#[test]
fn font_catalog_contribution_uses_injected_check() {
    let rec = Recorder::default();
    let c = ctx_with(SourceClass::Registry, &rec);
    assert!(check_with(&fixture("content-pack"), &MemFiles::full(), &c).is_ok());
    assert_eq!(*rec.fonts.borrow(), [br#"{"entries": []}"#.to_vec()]);
    let files = MemFiles::full().with("fonts/catalog.json", b"bad catalog");
    let errs = check_with(&fixture("content-pack"), &files, &c).unwrap_err();
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].reason(), Some("fontCatalog"));
    assert_eq!(errs[0].path(), Some("/contributes/fontCatalogs/0/file"));
    assert!(errs[0].message.contains("marked bad"));
}

#[test]
fn caption_style_contribution_uses_injected_check() {
    let rec = Recorder::default();
    let c = ctx_with(SourceClass::Registry, &rec);
    assert!(check_with(&fixture("content-pack"), &MemFiles::full(), &c).is_ok());
    assert_eq!(*rec.styles.borrow(), [br#"{"text": {}}"#.to_vec()]);
    let files = MemFiles::full().with("styles/bold-pop.json", b"bad style");
    let errs = check_with(&fixture("content-pack"), &files, &c).unwrap_err();
    assert_eq!(errs[0].reason(), Some("captionStyle"));
    let styles = json!([
        { "name": "bold-pop", "file": "styles/bold-pop.json" },
        { "name": "bold-pop", "file": "styles/bold-pop.json" },
        { "name": "Bold", "file": "styles/bold-pop.json" }
    ]);
    let v = set(
        fixture("content-pack"),
        "/contributes/captionStyles",
        styles,
    );
    assert_eq!(reasons(check(&v)), ["duplicateStyle", "styleName"]);
    // Without the injected check, a style that is not a JSON object still fails.
    let files = MemFiles::full().with("styles/bold-pop.json", b"[1]");
    assert_eq!(
        reasons(check_with(
            &fixture("content-pack"),
            &files,
            &ctx(SourceClass::Registry)
        )),
        ["captionStyle"]
    );
}

#[test]
fn compatibility_with_the_running_host() {
    let v = set(fixture("code-node"), "/hostApi", json!("^2.0"));
    let errs = check(&v).unwrap_err();
    assert_eq!(errs[0].code, "PLUGIN_HOST_API");
    assert_eq!(errs[0].details["range"], json!("^2.0"));
    assert_eq!(errs[0].details["host"], json!("1.2.0"));
    let mut c: ValidateCtx = ctx(SourceClass::Registry);
    c.platform = Some(Platform::Win32Arm64);
    c.node = Some(Version::new(24, 21, 0));
    let codes: Vec<String> = check_with(&fixture("code-node"), &MemFiles::full(), &c)
        .unwrap_err()
        .iter()
        .map(|e| e.code.clone())
        .collect();
    assert_eq!(codes, ["PLUGIN_PLATFORM", "PLUGIN_NODE_VERSION"]);
    c.platform = Some(Platform::LinuxArm64);
    c.node = Some(Version::new(24, 22, 2));
    assert!(check_with(&fixture("code-node"), &MemFiles::full(), &c).is_ok());
    // Content packs run anywhere.
    c.platform = Some(Platform::Win32Arm64);
    assert!(check_with(&fixture("content-pack"), &MemFiles::full(), &c).is_ok());
}

#[test]
fn permission_grammar() {
    let net = |origins: Value| {
        set(
            fixture("code-native"),
            "/permissions/1",
            json!({ "perm": "net", "origins": origins }),
        )
    };
    for ok in ["https://api.example.org", "https://a.b.example:8443"] {
        assert!(check(&net(json!([ok]))).is_ok(), "{ok}");
    }
    for bad in [
        "http://api.example.org",
        "https://api.example.org/",
        "https://api.example.org/v1",
        "https://API.example.org",
        "https://localhost",
        "https://user@api.example.org",
        "https://api.example.org:0",
        "https://api.example.org:70000",
        "https://*.example.org",
    ] {
        assert_eq!(reasons(check(&net(json!([bad])))), ["origins"], "{bad}");
    }
    assert_eq!(reasons(check(&net(json!([])))), ["emptyPermissionList"]);
    let dup = net(json!([
        "https://api.example.org",
        "https://api.example.org"
    ]));
    assert_eq!(reasons(check(&dup)), ["duplicateEntry"]);
    let v = set(
        fixture("code-native"),
        "/permissions/0",
        json!({ "perm": "gpu" }),
    );
    let v = set(v, "/permissions/2", json!({ "perm": "gpu" }));
    assert_eq!(reasons(check(&v)), ["duplicatePermission"]);
    // hooks:modify needs the same hook observed.
    let v = set(
        fixture("code-node"),
        "/permissions/1/hooks",
        json!(["PreCommand", "SessionStart"]),
    );
    let errs = check(&v).unwrap_err();
    assert_eq!(errs[0].reason(), Some("modifyWithoutObserve"));
    assert_eq!(errs[0].details["hook"], json!("SessionStart"));
    let v = set(
        fixture("code-node"),
        "/permissions/2/names",
        json!(["Bad Name"]),
    );
    assert_eq!(reasons(check(&v)), ["names"]);
    let v = set(
        fixture("code-node"),
        "/permissions/0/hooks",
        json!(["post-command"]),
    );
    let r = reasons(check(&v));
    assert!(r.starts_with(&["hooks".to_string()]), "{r:?}");
}

#[test]
fn licences_refused_by_source() {
    let v = set(fixture("content-pack"), "/license", json!("CC-BY-NC-4.0"));
    assert_eq!(reasons(check(&v)), ["licenceNonCommercial"]);
    let v = set(
        fixture("content-pack"),
        "/license",
        json!("LicenseRef-acme"),
    );
    assert!(
        check(&v).is_ok(),
        "unknown is a plan warning outside bundled sources"
    );
    assert_eq!(
        reasons(check_with(
            &v,
            &MemFiles::full(),
            &ctx(SourceClass::Bundled)
        )),
        ["licenceUnknown"]
    );
    let v = set(fixture("content-pack"), "/license", json!("GPL-3.0-only"));
    assert!(
        check(&v).is_ok(),
        "copyleft is flagged by the plan, not refused"
    );
    let v = set(
        fixture("code-native"),
        "/models/0/license",
        json!("CC-BY-NC-SA-4.0"),
    );
    assert_eq!(reasons(check(&v)), ["licenceNonCommercial"]);
    let v = set(fixture("code-native"), "/models/0/sha256", json!("ABC"));
    assert_eq!(reasons(check(&v)), ["model"]);
    let v = set(
        fixture("content-pack"),
        "/publisher/url",
        json!("http://acme.example.org"),
    );
    assert_eq!(reasons(check(&v)), ["https"]);
}
