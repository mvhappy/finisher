//! The native build gate's libopus entry (`third_party/native.json`, checked in full by
//! `scripts/check-native-deps.mjs`): this crate, the submodule at the pinned commit, BSD-3-Clause,
//! and the licence file's BLAKE3 as the submodule has it.

use std::path::Path;
use std::process::Command;

const COMMIT: &str = "22244de5a79bd1d6d623c32e72bf1954b56235be";

fn repo() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// CR LF folded to LF, as `scripts/check-native-deps.mjs` hashes: Git for Windows checks a
/// submodule's text out with CRLF (`core.autocrlf=true`), which the repository's
/// `.gitattributes` does not reach. A lone CR is kept.
fn lf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            continue;
        }
        out.push(b);
    }
    out
}

#[test]
fn a_crlf_checkout_hashes_as_its_lf_text() {
    assert_eq!(lf(b"one\r\ntwo\r\n"), b"one\ntwo\n");
    assert_eq!(lf(b"a\rb\n"), b"a\rb\n");
}

#[test]
fn native_gate_lists_libopus() {
    let text = std::fs::read_to_string(repo().join("third_party/native.json")).unwrap();
    let native: serde_json::Value = serde_json::from_str(&text).unwrap();
    let libs = native["libraries"].as_array().unwrap();
    let l = libs
        .iter()
        .find(|l| l["name"] == "libopus")
        .expect("third_party/native.json lists libopus");
    assert_eq!(l["package"], "fin-opus-sys");
    assert_eq!(l["path"], "third_party/opus");
    assert_eq!(l["tag"], "v1.6.1");
    assert_eq!(l["commit"], COMMIT);
    assert_eq!(l["license"], "BSD-3-Clause");
    let copying = std::fs::read(repo().join("third_party/opus/COPYING")).unwrap();
    assert_eq!(
        l["files"]["COPYING"],
        blake3::hash(&lf(&copying)).to_hex().as_str(),
        "the licence text changed: re-review it and update the hash"
    );
    // The gitlink the gate compares against (git is there in every checkout that builds this).
    let out = Command::new("git")
        .arg("-C")
        .arg(repo())
        .args(["ls-files", "--stage", "third_party/opus"])
        .output()
        .unwrap();
    let tree = String::from_utf8_lossy(&out.stdout);
    if out.status.success() && !tree.trim().is_empty() {
        assert!(tree.contains(COMMIT), "{tree}");
    }
}
