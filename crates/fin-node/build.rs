//! Generates `$OUT_DIR/bundled_plugins.rs`: the compiled-in `BUNDLED_PLUGINS` list (8A plan
//! Task 10, Decision 24). Every folder under `resources/plugins/bundled/` is one first-party
//! plugin; its entry is `(id, version, tree hash)` with the id and version read from its
//! manifest and the hash from `fin_registry::package::tree_hash`, the function the server
//! re-hashes the resources with at start. A build whose resources are not what the list says
//! cannot exist: the list is made from the files.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const MANIFEST: &str = "finisher-plugin.json";

fn main() {
    napi_build::setup();
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let root = manifest_dir.join("../../resources/plugins/bundled");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", root.display());
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets it"));
    std::fs::write(out.join("bundled_plugins.rs"), generate(&root)).expect("write the list");
}

fn generate(root: &Path) -> String {
    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(root) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(e) => panic!("cannot read {}: {e}", root.display()),
    };
    dirs.sort();
    let mut src = String::from(
        "/// The bundled plugins this binary was built with: `(id, version, tree hash)`.\n\
         pub const BUNDLED_PLUGINS: &[(&str, &str, &str)] = &[\n",
    );
    for dir in &dirs {
        let folder = dir
            .file_name()
            .and_then(|n| n.to_str())
            .expect("UTF-8 name");
        let bytes = std::fs::read(dir.join(MANIFEST))
            .unwrap_or_else(|e| panic!("{}: no {MANIFEST}: {e}", dir.display()));
        let m: serde_json::Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("{}: {MANIFEST} is not JSON: {e}", dir.display()));
        let id = m["id"].as_str().expect("the manifest has an id");
        let version = m["version"].as_str().expect("the manifest has a version");
        assert_eq!(
            id,
            format!("@finisher/{folder}"),
            "a bundled plugin lives in resources/plugins/bundled/<name> and is @finisher/<name>"
        );
        let hash = fin_registry::package::tree_hash(dir)
            .unwrap_or_else(|e| panic!("{}: cannot hash: {e}", dir.display()));
        writeln!(src, "    ({id:?}, {version:?}, {hash:?}),").expect("write to a string");
    }
    src.push_str("];\n");
    src
}
