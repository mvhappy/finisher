//! Records the shaping crates' versions for `fin_text::fingerprint()` (4C Decision 38): the
//! `cosmic-text` this crate resolves to, the `harfrust`, `skrifa`, `unicode-bidi`,
//! `unicode-linebreak`, `unicode-segmentation` and `unicode-script` it depends on, and the
//! `read-fonts` of that `skrifa`, all read from the workspace `Cargo.lock`.

use std::path::PathBuf;

struct Package {
    name: String,
    version: String,
    deps: Vec<String>,
}

fn parse(lock: &str) -> Vec<Package> {
    let mut out = Vec::new();
    for block in lock.split("[[package]]").skip(1) {
        let mut pkg = Package {
            name: String::new(),
            version: String::new(),
            deps: Vec::new(),
        };
        let mut in_deps = false;
        for line in block.lines() {
            let line = line.trim();
            if in_deps {
                if line.starts_with(']') {
                    in_deps = false;
                } else {
                    pkg.deps.push(line.trim_matches([',', '"']).to_string());
                }
            } else if let Some(v) = line.strip_prefix("name = ") {
                pkg.name = v.trim_matches('"').to_string();
            } else if let Some(v) = line.strip_prefix("version = ") {
                pkg.version = v.trim_matches('"').to_string();
            } else if line.starts_with("dependencies = [") {
                in_deps = !line.ends_with(']');
            }
        }
        out.push(pkg);
    }
    out
}

/// The version of `name` that `parent` depends on: from the dependency entry when the lock
/// file spells it (`"skrifa 0.40.0"`), else the one package of that name.
fn dep_version(pkgs: &[Package], parent: &Package, name: &str) -> String {
    let entry = parent
        .deps
        .iter()
        .find(|d| d.split(' ').next() == Some(name))
        .unwrap_or_else(|| panic!("{} does not depend on {name}", parent.name));
    if let Some(v) = entry.split(' ').nth(1) {
        return v.to_string();
    }
    let all: Vec<&Package> = pkgs.iter().filter(|p| p.name == name).collect();
    assert_eq!(all.len(), 1, "one {name} in Cargo.lock");
    all[0].version.clone()
}

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let lock_path = dir.join("../../Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock_path.display());
    let lock = std::fs::read_to_string(&lock_path).expect("the workspace Cargo.lock");
    let pkgs = parse(&lock);
    let me = pkgs
        .iter()
        .find(|p| p.name == "fin-text")
        .expect("fin-text in Cargo.lock");
    let cosmic_version = dep_version(&pkgs, me, "cosmic-text");
    let cosmic = pkgs
        .iter()
        .find(|p| p.name == "cosmic-text" && p.version == cosmic_version)
        .expect("cosmic-text in Cargo.lock");
    println!("cargo:rustc-env=FIN_TEXT_COSMIC_TEXT={cosmic_version}");
    for name in [
        "harfrust",
        "skrifa",
        "unicode-bidi",
        "unicode-linebreak",
        "unicode-segmentation",
        "unicode-script",
    ] {
        let version = dep_version(&pkgs, cosmic, name);
        let var = name.to_uppercase().replace('-', "_");
        println!("cargo:rustc-env=FIN_TEXT_{var}={version}");
    }
    let skrifa_version = dep_version(&pkgs, cosmic, "skrifa");
    let skrifa = pkgs
        .iter()
        .find(|p| p.name == "skrifa" && p.version == skrifa_version)
        .expect("skrifa in Cargo.lock");
    let read_fonts = dep_version(&pkgs, skrifa, "read-fonts");
    println!("cargo:rustc-env=FIN_TEXT_READ_FONTS={read_fonts}");
}
