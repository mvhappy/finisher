//! The native build gate's vo-aacenc entry (`third_party/native.json`, checked in full by
//! `scripts/check-native-deps.mjs`): this crate, the submodule at the pinned commit of
//! Finisher's fork (spec §6.4, 6C Owner question 6), Apache-2.0, and the licence and notice
//! files' BLAKE3 as the submodule has them. Runs on every host (the files are data).

use std::path::Path;
use std::process::Command;

const COMMIT: &str = "a277487e051e92e99a532294eed3c673f4d879f2";

fn repo() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// The bytes with CRLF line ends read as LF: the hash is of the text as the submodule has it,
/// and a Windows checkout (`core.autocrlf=true`, which a parent `.gitattributes` does not reach
/// inside a submodule) has the same text with CRLF ends (`scripts/check-native-deps.mjs` does the
/// same).
fn lf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for (i, &b) in bytes.iter().enumerate() {
        if !(b == b'\r' && bytes.get(i + 1) == Some(&b'\n')) {
            out.push(b);
        }
    }
    out
}

#[test]
fn native_gate_lists_vo_aacenc() {
    let text = std::fs::read_to_string(repo().join("third_party/native.json")).unwrap();
    let native: serde_json::Value = serde_json::from_str(&text).unwrap();
    let libs = native["libraries"].as_array().unwrap();
    let l = libs
        .iter()
        .find(|l| l["name"] == "vo-aacenc")
        .expect("third_party/native.json lists vo-aacenc");
    assert_eq!(l["package"], "fin-aacenc-sys");
    assert_eq!(l["path"], "third_party/vo-aacenc");
    // The pin is upstream's last commit, after the v0.1.3 tag (whose release it still is,
    // `src/version.rs`): no tag names it, so the entry has none.
    assert!(l.get("tag").is_none(), "{l}");
    assert_eq!(l["commit"], COMMIT);
    assert_eq!(l["license"], "Apache-2.0");
    for f in ["COPYING", "NOTICE"] {
        let bytes = std::fs::read(repo().join("third_party/vo-aacenc").join(f)).unwrap();
        assert_eq!(
            l["files"][f],
            blake3::hash(&lf(&bytes)).to_hex().as_str(),
            "{f} changed: re-review it and update the hash"
        );
    }
    // The gitlink the gate compares against (git is there in every checkout that builds this).
    let out = Command::new("git")
        .arg("-C")
        .arg(repo())
        .args(["ls-files", "--stage", "third_party/vo-aacenc"])
        .output()
        .unwrap();
    let tree = String::from_utf8_lossy(&out.stdout);
    if out.status.success() && !tree.trim().is_empty() {
        assert!(tree.contains(COMMIT), "{tree}");
    }
}

/// `.gitmodules` names Finisher's fork (6C Owner question 6: the merge prerequisite), shallow
/// like the other codec submodules.
#[test]
fn the_submodule_is_finishers_fork() {
    let text = std::fs::read_to_string(repo().join(".gitmodules")).unwrap();
    let section: Vec<&str> = text
        .split("[submodule")
        .find(|s| s.contains("path = third_party/vo-aacenc"))
        .expect(".gitmodules lists third_party/vo-aacenc")
        .lines()
        .map(str::trim)
        .collect();
    assert!(
        section.contains(&"url = https://github.com/mvhappy/vo-aacenc"),
        "{section:?}"
    );
    assert!(section.contains(&"shallow = true"), "{section:?}");
}
