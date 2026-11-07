//! Plans and the store in tests: packages built in code, verified targets made from them as
//! the TUF client would, and a [`PlanHost`] over in-memory catalog ids.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use fin_plugin_api::{
    MANIFEST_FILE, Manifest, Platform, PluginId, SourceClass, StructuralChecks, ValidateCtx,
    host_api,
};
use fin_registry::package::{pack, tree_hash};
use fin_registry::plan::{InstallPlan, PlanHost, PlanInput, PlanRequest, make_plan};
use fin_registry::target::{VerifiedTarget, custom_for};
use fin_registry::{FixedClock, RegistryError, Store};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::write_tree;

pub const NOW: u64 = 1_794_571_200_000;

pub fn content(id: &str, version: &str) -> Value {
    json!({
        "manifestVersion": 1, "id": id, "version": version, "kind": "content",
        "hostApi": "^1.0", "license": "MIT", "publisher": { "name": "Acme Studio" },
        "contributes": { "captionStyles": [{ "name": "bold", "file": "styles/bold.json" }] },
    })
}

pub fn fonts(id: &str) -> Value {
    json!({
        "manifestVersion": 1, "id": id, "version": "1.0.0", "kind": "content",
        "hostApi": "^1.0", "license": "OFL-1.1", "publisher": { "name": "Acme Studio" },
        "contributes": { "fontCatalogs": [{ "file": "fonts/catalog.json" }] },
    })
}

pub fn code(id: &str, version: &str, perms: Value) -> Value {
    json!({
        "manifestVersion": 1, "id": id, "version": version, "kind": "code",
        "hostApi": "^1.0", "license": "MIT", "publisher": { "name": "Acme Studio" },
        "platforms": ["linux-x64"],
        "runtime": { "type": "native", "exec": { "linux-x64": "bin/tool" } },
        "permissions": perms,
    })
}

/// A shell script that would leave `marker` behind if anything ran it.
pub fn tool_script(marker: &Path) -> Vec<u8> {
    format!("#!/bin/sh\ntouch '{}'\n", marker.display()).into_bytes()
}

/// A store, a scratch directory and a host.
pub struct Fx {
    pub tmp: TempDir,
    pub store: Store,
    pub host: TestHost,
    n: std::cell::Cell<u32>,
}

#[derive(Default)]
pub struct TestHost {
    pub taken: BTreeSet<String>,
}

impl PlanHost for TestHost {
    fn font_catalog_ids(&self, bytes: &[u8]) -> Vec<String> {
        let v: Value = serde_json::from_slice(bytes).unwrap_or_default();
        v["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["id"].as_str().map(String::from))
            .collect()
    }

    fn taken_font_ids(&self, _except: &PluginId) -> BTreeSet<String> {
        self.taken.clone()
    }
}

pub fn ctx(source: SourceClass) -> ValidateCtx<'static> {
    ValidateCtx {
        host_api: host_api(),
        platform: Some(Platform::LinuxX64),
        source,
        node: None,
        content_checks: &StructuralChecks,
    }
}

/// The files every manifest above may name.
pub fn default_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("styles/bold.json", br#"{"text": {}}"#.to_vec()),
        (
            "fonts/catalog.json",
            br#"{"entries": [{"id": "acme-sans"}]}"#.to_vec(),
        ),
        ("bin/tool", b"#!/bin/sh\nexit 0\n".to_vec()),
    ]
}

impl Fx {
    pub fn new() -> Fx {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(&tmp.path().join("plugins")).unwrap();
        Fx {
            tmp,
            store,
            host: TestHost::default(),
            n: std::cell::Cell::new(0),
        }
    }

    fn scratch(&self, what: &str) -> PathBuf {
        self.n.set(self.n.get() + 1);
        self.tmp.path().join(format!("{what}-{}", self.n.get()))
    }

    /// An unpacked source tree with `manifest` and `files`.
    pub fn tree(&self, manifest: &Value, files: &[(&str, Vec<u8>)]) -> PathBuf {
        let dir = self.scratch("src");
        let m = serde_json::to_vec_pretty(manifest).unwrap();
        let mut all: Vec<(&str, &[u8])> = vec![(MANIFEST_FILE, &m)];
        all.extend(files.iter().map(|(p, b)| (*p, b.as_slice())));
        write_tree(&dir, &all);
        dir
    }

    /// A packed target as the TUF client hands it over: signed length, hash and custom
    /// metadata computed from the package itself.
    pub fn target(&self, manifest: &Value, files: &[(&str, Vec<u8>)]) -> VerifiedTarget {
        let dir = self.tree(manifest, files);
        let m: Manifest = serde_json::from_value(manifest.clone()).unwrap();
        let archive = self.scratch("pkg").with_extension("finpkg");
        let info = pack(&dir, &m.executables(), &archive).unwrap();
        let bytes: u64 = files.iter().map(|(_, b)| b.len() as u64).sum::<u64>()
            + std::fs::metadata(dir.join(MANIFEST_FILE)).unwrap().len();
        let mbytes = std::fs::read(dir.join(MANIFEST_FILE)).unwrap();
        VerifiedTarget {
            registry_id: "team".into(),
            archive,
            length: info.bytes,
            sha256: info.sha256,
            custom: custom_for(&m, &mbytes, bytes),
            stale_days: None,
            warnings: Vec::new(),
        }
    }

    pub fn bundled(&self, manifest: &Value, files: &[(&str, Vec<u8>)]) -> PlanInput {
        let dir = self.tree(manifest, files);
        let tree_hash = tree_hash(&dir).unwrap();
        PlanInput::Bundled { dir, tree_hash }
    }

    pub fn plan(&self, src: PlanInput, req: &PlanRequest) -> Result<InstallPlan, RegistryError> {
        let source = match src {
            PlanInput::Bundled { .. } => SourceClass::Bundled,
            PlanInput::Registry(_) => SourceClass::Registry,
        };
        make_plan(
            src,
            &self.store,
            req,
            &ctx(source),
            &self.host,
            &FixedClock(NOW),
        )
    }

    pub fn plan_target(&self, t: VerifiedTarget) -> Result<InstallPlan, RegistryError> {
        let req = PlanRequest {
            id: t.custom.id.clone(),
            ..Default::default()
        };
        self.plan(PlanInput::Registry(Box::new(t)), &req)
    }

    pub fn plugins(&self) -> &Path {
        self.store.base()
    }
}

/// Every regular file under `root`, relative, sorted.
pub fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    out.sort();
    out
}
