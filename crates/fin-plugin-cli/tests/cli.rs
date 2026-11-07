//! `fin-plugin` end to end: the built binary runs, and what it did to a registry is read back
//! with the client (`TrustedRepo`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use fin_registry::editor::SigningKey;
use fin_registry::transport::FsTransport;
use fin_registry::tuf::{Policy, SystemClock, TrustStore, TrustedRepo, sha256_hex};
use serde_json::{Value, json};

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("stdout is JSON ({e}): {}\n{}", self.stdout, self.stderr))
    }
    fn err(&self) -> Value {
        serde_json::from_str(&self.stderr)
            .unwrap_or_else(|e| panic!("stderr is JSON ({e}): {}", self.stderr))
    }
}

/// A scratch area; every command's output is kept for `keys_are_never_printed`.
struct Env {
    tmp: tempfile::TempDir,
    log: std::cell::RefCell<String>,
}

impl Env {
    fn new() -> Env {
        Env {
            tmp: tempfile::tempdir().unwrap(),
            log: Default::default(),
        }
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }
    fn s(&self, rel: &str) -> String {
        self.p(rel).display().to_string()
    }
    fn run(&self, args: &[&str]) -> Out {
        let o = Command::new(env!("CARGO_BIN_EXE_fin-plugin"))
            .args(args)
            .output()
            .unwrap();
        let out = Out {
            code: o.status.code().unwrap(),
            stdout: String::from_utf8(o.stdout).unwrap(),
            stderr: String::from_utf8(o.stderr).unwrap(),
        };
        self.log
            .borrow_mut()
            .push_str(&format!("{}\n{}\n", out.stdout, out.stderr));
        out
    }
    fn ok(&self, args: &[&str]) -> Value {
        let o = self.run(args);
        assert_eq!(o.code, 0, "{args:?}: {}", o.stderr);
        assert!(
            o.stderr.is_empty(),
            "{args:?} wrote to stderr: {}",
            o.stderr
        );
        o.json()
    }
    /// A new key at `name.pem`, returns its path.
    fn keygen(&self, name: &str) -> String {
        let path = self.s(&format!("{name}.pem"));
        self.ok(&[
            "keygen",
            &path,
            "--public",
            &self.s(&format!("{name}.pub.pem")),
        ]);
        path
    }
    /// A content plugin `new`-scaffolded, with its id and version rewritten, packed.
    fn pkg(&self, id: &str, version: &str, extra: Option<(&str, Value)>) -> String {
        let src = self.s(&format!("src-{}-{version}", id.replace(['@', '/'], "_")));
        self.ok(&["new", "content", &src]);
        let mp = Path::new(&src).join("finisher-plugin.json");
        let mut m: Value = serde_json::from_slice(&std::fs::read(&mp).unwrap()).unwrap();
        m["id"] = id.into();
        m["version"] = version.into();
        if let Some((k, v)) = extra {
            m[k] = v;
        }
        std::fs::write(&mp, serde_json::to_vec_pretty(&m).unwrap()).unwrap();
        let out = self.s(&format!("{}-{version}.finpkg", id.replace(['@', '/'], "_")));
        self.ok(&["pack", &src, "-o", &out]);
        out
    }
}

/// A registry with one root key, one publisher `acme`.
struct Reg {
    env: Env,
    repo: String,
    keys: String,
    publisher: String,
    root_sha: String,
}

fn registry() -> Reg {
    let env = Env::new();
    let root = env.keygen("root");
    let publisher = env.keygen("acme");
    let repo = env.s("registry");
    let keys = env.s("keyring");
    let init = env.ok(&[
        "registry",
        "init",
        &repo,
        "--keys",
        &keys,
        "--root-keys",
        &root,
        "--threshold",
        "1",
    ]);
    let root_sha = init["root"]["sha256"].as_str().unwrap().to_string();
    env.ok(&[
        "registry",
        "delegate",
        &repo,
        "--keys",
        &keys,
        "--publisher",
        "acme",
        "--key",
        &env.s("acme.pub.pem"),
    ]);
    Reg {
        env,
        repo,
        keys,
        publisher,
        root_sha,
    }
}

impl Reg {
    fn publish(&self, pkg: &str) -> Out {
        self.env.run(&[
            "registry",
            "publish",
            &self.repo,
            pkg,
            "--keys",
            &self.keys,
            "--key",
            &self.publisher,
        ])
    }

    /// A client pinned to `1.root.json`'s printed sha, refreshed from the folder.
    fn client(&self) -> TrustedRepo {
        let root = std::fs::read(Path::new(&self.repo).join("metadata/1.root.json")).unwrap();
        assert_eq!(sha256_hex(&root), self.root_sha);
        let store = TrustStore::new(&self.env.p("trust"), &self.root_sha).unwrap();
        let mut c = if store.exists() {
            TrustedRepo::open(store).unwrap()
        } else {
            TrustedRepo::init(store, &root, &self.root_sha).unwrap()
        };
        c.refresh(
            Arc::new(FsTransport::new(&self.repo)),
            &Policy::default(),
            &SystemClock,
        )
        .unwrap();
        c
    }
}

#[test]
fn new_then_validate_then_pack() {
    let env = Env::new();
    for kind in ["content", "code"] {
        let dir = env.s(kind);
        let n = env.ok(&["new", kind, &dir]);
        assert_eq!(n["kind"], kind);
        let v = env.ok(&["validate", &dir]);
        assert_eq!(v["ok"], true);
        assert_eq!(v["kind"], kind);
        let out = env.s(&format!("{kind}.finpkg"));
        let p = env.ok(&["pack", &dir, "-o", &out]);
        assert_eq!(
            p["bytes"].as_u64().unwrap(),
            std::fs::metadata(&out).unwrap().len()
        );
        assert_eq!(p["sha256"].as_str().unwrap().len(), 64);
        assert!(p["entries"].as_u64().unwrap() >= 2);
        assert!(!p["treeHash"].as_str().unwrap().is_empty());
    }
    // A scaffold refuses a folder that holds something.
    let o = env.run(&["new", "content", &env.s("content")]);
    assert_eq!(o.code, 4);
    assert_eq!(o.err()["code"], "DIR_NOT_EMPTY");
}

#[test]
fn validate_reports_all_problems() {
    let env = Env::new();
    let dir = env.s("bad");
    env.ok(&["new", "content", &dir]);
    let mp = Path::new(&dir).join("finisher-plugin.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&mp).unwrap()).unwrap();
    m["version"] = "not-a-version".into();
    m["contributes"] = json!({ "captionStyles": [{ "name": "x", "file": "missing.json" }] });
    std::fs::write(&mp, serde_json::to_vec(&m).unwrap()).unwrap();
    let o = env.run(&["validate", &dir]);
    assert_eq!(o.code, 3);
    let e = o.err();
    assert_eq!(e["code"], "PLUGIN_MANIFEST_INVALID");
    assert!(!e["details"]["problems"].as_array().unwrap().is_empty());
}

#[test]
fn keygen_writes_a_private_key_and_prints_only_its_id() {
    let env = Env::new();
    let k = env.s("k.pem");
    let out = env.ok(&["keygen", &k]);
    let key = SigningKey::read(Path::new(&k)).unwrap();
    assert_eq!(out, json!({ "keyId": key.public().key_id() }));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&k).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(env.run(&["keygen", &k]).code, 4);
}

#[test]
fn init_publishes_a_trusted_root() {
    let r = registry();
    let c = r.client();
    assert_eq!(c.root_version(), 1);
    let root = std::fs::read(Path::new(&r.repo).join("metadata/1.root.json")).unwrap();
    assert_eq!(sha256_hex(&root), r.root_sha);
    // A second init in the same folder is refused.
    let o = r.env.run(&[
        "registry",
        "init",
        &r.repo,
        "--keys",
        &r.env.s("k2"),
        "--root-keys",
        &r.env.s("root.pem"),
        "--threshold",
        "1",
    ]);
    assert_eq!(o.code, 4);
    assert_eq!(o.err()["code"], "REGISTRY_EXISTS");
}

#[test]
fn delegate_and_publish_list_the_package() {
    let r = registry();
    let pkg = r.env.pkg("@acme/pack", "1.0.0", None);
    let out = r.publish(&pkg);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let printed = out.json();
    assert_eq!(printed["name"], "acme/pack/1.0.0.finpkg");
    let c = r.client();
    let t = c
        .targets()
        .find(|t| t.name == "acme/pack/1.0.0.finpkg")
        .expect("listed");
    assert_eq!(t.sha256, printed["sha256"].as_str().unwrap());
    assert_eq!(t.role, "acme");
}

#[test]
fn publish_refuses_existing_version() {
    let r = registry();
    let pkg = r.env.pkg("@acme/pack", "1.0.0", None);
    assert_eq!(r.publish(&pkg).code, 0);
    let o = r.publish(&pkg);
    assert_eq!(o.code, 4);
    assert_eq!(o.err()["code"], "PLUGIN_VERSION_EXISTS");
}

#[test]
fn publish_fills_custom_metadata_from_manifest() {
    let r = registry();
    let pkg = r.env.pkg(
        "@acme/pack",
        "1.2.0",
        Some(("license", "Apache-2.0".into())),
    );
    assert_eq!(r.publish(&pkg).code, 0);
    let c = r.client();
    let t = c
        .targets()
        .find(|t| t.name == "acme/pack/1.2.0.finpkg")
        .unwrap();
    let custom = t.package().expect("package metadata");
    let src = r.env.s("src-_acme_pack-1.2.0/finisher-plugin.json");
    let bytes = std::fs::read(&src).unwrap();
    let m: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(custom.id, m["id"]);
    assert_eq!(custom.version, m["version"]);
    assert_eq!(serde_json::to_value(custom.kind).unwrap(), m["kind"]);
    assert_eq!(custom.host_api, m["hostApi"].as_str().unwrap());
    assert_eq!(custom.license, "Apache-2.0");
    assert_eq!(custom.manifest_sha256, sha256_hex(&bytes));
    let style = std::fs::metadata(r.env.s("src-_acme_pack-1.2.0/styles/default.json"))
        .unwrap()
        .len();
    assert_eq!(custom.bytes_unpacked, bytes.len() as u64 + style);
    assert!(!custom.yanked);
}

#[test]
fn yank_marks_the_version() {
    let r = registry();
    assert_eq!(r.publish(&r.env.pkg("@acme/pack", "1.0.0", None)).code, 0);
    assert_eq!(r.publish(&r.env.pkg("@acme/pack", "1.1.0", None)).code, 0);
    let y = r.env.ok(&[
        "registry",
        "yank",
        &r.repo,
        "@acme/pack@1.0.0",
        "--keys",
        &r.keys,
        "--key",
        &r.publisher,
    ]);
    assert_eq!(y["yanked"], true);
    let c = r.client();
    let yanked = |n: &str| {
        c.targets()
            .find(|t| t.name == n)
            .unwrap()
            .package()
            .unwrap()
            .yanked
    };
    assert!(yanked("acme/pack/1.0.0.finpkg"));
    assert!(!yanked("acme/pack/1.1.0.finpkg"));
    let o = r.env.run(&[
        "registry",
        "yank",
        &r.repo,
        "@acme/pack@9.9.9",
        "--keys",
        &r.keys,
        "--key",
        &r.publisher,
    ]);
    assert_eq!(o.code, 3);
    assert_eq!(o.err()["code"], "PLUGIN_NOT_FOUND");
}

#[test]
fn advise_adds_an_advisory_target() {
    let r = registry();
    assert_eq!(r.publish(&r.env.pkg("@acme/pack", "1.0.0", None)).code, 0);
    let a = r.env.ok(&[
        "registry",
        "advise",
        &r.repo,
        "--keys",
        &r.keys,
        "--plugin",
        "@acme/pack",
        "--versions",
        "<1.1.0",
        "--action",
        "warn",
        "--summary",
        "old",
        "--key",
        &r.publisher,
    ]);
    assert!(a["id"].as_str().unwrap().starts_with("ADV-"));
    let c = r.client();
    assert!(
        c.targets()
            .any(|t| t.name.ends_with("advisories.json") && t.role == "acme")
    );
    // Bad input is a usage error.
    let o = r.env.run(&[
        "registry",
        "advise",
        &r.repo,
        "--keys",
        &r.keys,
        "--plugin",
        "@acme/pack",
        "--versions",
        "<1.1.0",
        "--action",
        "explode",
        "--summary",
        "x",
        "--key",
        &r.publisher,
    ]);
    assert_eq!(o.code, 2);
}

#[test]
fn rotate_key_bumps_root() {
    let r = registry();
    let new = r.env.keygen("root2");
    let o = r.env.ok(&[
        "registry",
        "rotate-key",
        &r.repo,
        "--keys",
        &r.keys,
        "--role",
        "root",
        "--new",
        &new,
    ]);
    assert_eq!(o["root"]["version"], 2);
    let c = r.client();
    assert_eq!(c.root_version(), 2);
    // A publisher's key rotates too.
    let p2 = r.env.keygen("acme2");
    r.env.ok(&[
        "registry",
        "rotate-key",
        &r.repo,
        "--keys",
        &r.keys,
        "--role",
        "acme",
        "--new",
        &p2,
    ]);
}

#[test]
fn resign_timestamp_bumps_its_version() {
    let r = registry();
    let before = r.client().versions().1.unwrap();
    let o = r.env.ok(&[
        "registry",
        "resign-timestamp",
        &r.repo,
        "--keys",
        &r.keys,
        "--expires",
        "10",
    ]);
    let after = o["timestamp"]["version"].as_u64().unwrap();
    assert!(after > before);
    assert_eq!(r.client().versions().1.unwrap(), after);
}

#[test]
fn keys_dir_inside_registry_refused() {
    let env = Env::new();
    let root = env.keygen("root");
    let repo = env.s("registry");
    for keys in [env.s("registry/keys"), format!("{repo}/../registry/keys")] {
        let o = env.run(&[
            "registry",
            "init",
            &repo,
            "--keys",
            &keys,
            "--root-keys",
            &root,
            "--threshold",
            "1",
        ]);
        assert_eq!(o.code, 2, "{}", o.stderr);
        assert_eq!(o.err()["code"], "USAGE");
    }
    assert!(!Path::new(&repo).exists());
}

#[test]
fn usage_errors_exit_2() {
    let env = Env::new();
    for args in [
        vec![],
        vec!["frobnicate"],
        vec!["new", "plugin", "x"],
        vec!["validate"],
        vec!["pack", "dir"],
        vec!["validate", "a", "b"],
        vec!["keygen", "k", "--bogus", "1"],
        vec!["registry"],
        vec!["registry", "nope"],
        vec!["registry", "init", "dir", "--keys"],
    ] {
        let o = env.run(&args);
        assert_eq!(o.code, 2, "{args:?}: {}", o.stderr);
        assert_eq!(o.err()["code"], "USAGE");
    }
}

#[test]
fn help_exits_0_and_explains_git() {
    let env = Env::new();
    for args in [vec!["--help"], vec!["registry", "--help"]] {
        let o = env.run(&args);
        assert_eq!(o.code, 0);
        assert!(o.stderr.is_empty());
    }
    let o = env.run(&["registry", "--help"]);
    assert!(o.stdout.contains(
        "a git registry is a clone of a repository holding this folder; commit and push it, and users `git pull`"
    ), "{}", o.stdout.replace('\n', " "));
}

#[test]
fn keys_are_never_printed() {
    let r = registry();
    let pkg = r.env.pkg("@acme/pack", "1.0.0", None);
    assert_eq!(r.publish(&pkg).code, 0);
    r.publish(&pkg); // an error path too
    r.env.run(&[
        "registry",
        "rotate-key",
        &r.repo,
        "--keys",
        &r.keys,
        "--role",
        "root",
        "--new",
        &r.env.s("nonexistent.pem"),
    ]);
    r.env.run(&[
        "registry",
        "init",
        &r.repo,
        "--keys",
        &r.keys,
        "--root-keys",
        &r.publisher,
        "--threshold",
        "1",
    ]);
    let log = r.env.log.borrow().clone();
    assert!(!log.contains("PRIVATE KEY"));
    let mut checked = 0;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&r.keys)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.push(r.env.p("root.pem"));
    files.push(r.env.p("acme.pem"));
    for f in files {
        for line in std::fs::read_to_string(&f).unwrap().lines() {
            if !line.starts_with("-----") && line.len() > 8 {
                assert!(
                    !log.contains(line),
                    "a key body line of {} was printed",
                    f.display()
                );
                checked += 1;
            }
        }
    }
    assert!(checked >= 5, "checked {checked} key files");
}

#[test]
fn trust_reset_clock_clears_only_the_watermark() {
    use fin_registry::registry::{Registry, RegistryConfig, RootSource};
    use fin_registry::tuf::{Clock, FixedClock};
    let r = registry();
    let plugins = r.env.p("server/plugins");
    let cfg = RegistryConfig {
        id: "team".into(),
        path: PathBuf::from(&r.repo),
        root_sha256: r.root_sha.clone(),
    };
    let mut reg = Registry::add(&plugins, cfg, RootSource::Sha256(r.root_sha.clone())).unwrap();
    // A clock 2 days fast records a watermark the corrected clock is more than a day behind.
    let real = SystemClock.now();
    reg.refresh(&FixedClock::new(
        real + std::time::Duration::from_secs(2 * 86_400),
    ))
    .unwrap();
    assert_eq!(
        reg.refresh(&FixedClock::new(real)).unwrap_err().code,
        "CLOCK_SUSPECT"
    );
    let gen_files = |reg: &Registry| {
        let dir = reg.trusted().store().dir();
        let k = std::fs::read_to_string(dir.join("current")).unwrap();
        std::fs::read_dir(dir.join(format!("gen-{}", k.trim())))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<std::collections::BTreeSet<_>>()
    };
    let before = gen_files(&reg);
    drop(reg);
    let plugins_s = plugins.display().to_string();
    let o = r.env.ok(&[
        "registry",
        "trust",
        "reset-clock",
        "--plugins",
        &plugins_s,
        "--id",
        "team",
    ]);
    assert_eq!(o, json!({ "id": "team", "lastRefreshAt": null }));
    let mut reg = Registry::open(&plugins, "team").unwrap();
    assert_eq!(reg.trusted().last_refresh_at().unwrap(), None);
    assert_eq!(gen_files(&reg), before);
    reg.refresh(&FixedClock::new(real)).unwrap();
    // An unknown registry, and usage errors.
    let o = r.env.run(&[
        "registry",
        "trust",
        "reset-clock",
        "--plugins",
        &plugins_s,
        "--id",
        "nope",
    ]);
    assert_eq!(
        (o.code, o.err()["code"].as_str()),
        (3, Some("REGISTRY_NOT_FOUND"))
    );
    for args in [
        vec!["registry", "trust"],
        vec!["registry", "trust", "forget-all"],
        vec!["registry", "trust", "reset-clock", "--id", "team"],
    ] {
        assert_eq!(r.env.run(&args).code, 2, "{args:?}");
    }
}

#[test]
fn registry_help_names_the_targets_key_for_a_first_publish() {
    let env = Env::new();
    let o = env.run(&["registry", "--help"]);
    assert_eq!(o.code, 0);
    assert!(
        o.stdout
            .contains("first publish also signs its delegation into `targets`")
    );
    assert!(o.stdout.contains("registry trust reset-clock"));
}
