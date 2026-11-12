//! Review #40 and #41: a Lottie animation must be a JSON object with the template keys, and
//! dotLottie v2 archives keep animations in `a/` (https://dotlottie.io/spec/2.0/).

use std::path::Path;

use fin_core::AssetKind;
use fin_probe::fixtures::{sample_lottie, write_zip};
use fin_probe::probe;
use serde_json::json;

fn unsupported(path: &Path) -> String {
    let err = probe(path).unwrap_err();
    assert_eq!(err.code, "UNSUPPORTED_CONTAINER", "{err:?}");
    err.details["reason"].as_str().unwrap().to_string()
}

fn archive(
    dir: &tempfile::TempDir,
    manifest: &str,
    members: &[(&str, String)],
) -> std::path::PathBuf {
    let path = dir.path().join("t.lottie");
    let mut all = vec![("manifest.json", manifest.to_string())];
    all.extend(members.iter().cloned());
    write_zip(&path, &all, true);
    path
}

const V2: &str = r#"{"version":"2","generator":"t","animations":[{"id":"main"}]}"#;
const V1: &str = r#"{"version":"1.0","generator":"t","animations":[{"id":"main"}]}"#;

#[test]
fn a_positional_array_is_not_an_animation() {
    let dir = tempfile::tempdir().unwrap();
    let anim = "[64,36,30,0,30,null,null,null]".to_string();
    let path = archive(&dir, V1, &[("animations/main.json", anim)]);
    let why = unsupported(&path);
    assert!(why.contains("not a JSON object"), "{why}");
}

#[test]
fn leading_whitespace_before_the_object_is_fine() {
    let dir = tempfile::tempdir().unwrap();
    let anim = format!("\n  {}", sample_lottie());
    let path = archive(&dir, V1, &[("animations/main.json", anim)]);
    assert_eq!(probe(&path).unwrap().kind, AssetKind::LottieTemplate);
}

#[test]
fn every_template_key_is_required() {
    let dir = tempfile::tempdir().unwrap();
    for key in ["w", "h", "fr", "ip", "op", "layers"] {
        let mut anim = sample_lottie();
        anim.as_object_mut().unwrap().remove(key);
        let path = archive(&dir, V1, &[("animations/main.json", anim.to_string())]);
        let why = unsupported(&path);
        assert!(why.contains(key), "{key}: {why}");
    }
    // `layers` must be an array.
    for bad in [json!({}), json!(null), json!("x"), json!(3)] {
        let mut anim = sample_lottie();
        anim["layers"] = bad;
        let path = archive(&dir, V1, &[("animations/main.json", anim.to_string())]);
        let why = unsupported(&path);
        assert!(why.contains("layers must be an array"), "{why}");
    }
}

#[test]
fn a_v2_archive_reads_a_dir() {
    let dir = tempfile::tempdir().unwrap();
    let path = archive(&dir, V2, &[("a/main.json", sample_lottie().to_string())]);
    let p = probe(&path).unwrap();
    assert_eq!(p.kind, AssetKind::LottieTemplate);
    let l = p.lottie.unwrap();
    assert_eq!((l.width, l.height), (1920, 1080));
    assert_eq!(l.slots.len(), 4);
    // State machines, themes and other v2 members are ignored.
    let path = archive(
        &dir,
        r#"{"version":"2","animations":[{"id":"main"}],"themes":[{"id":"t"}],"stateMachines":[{"id":"s"}]}"#,
        &[
            ("a/main.json", sample_lottie().to_string()),
            ("t/t.json", "{}".to_string()),
            ("s/s.json", "{}".to_string()),
        ],
    );
    assert_eq!(probe(&path).unwrap().kind, AssetKind::LottieTemplate);
}

#[test]
fn v1_and_unversioned_manifests_read_animations_dir() {
    let dir = tempfile::tempdir().unwrap();
    for manifest in [
        V1,
        r#"{"version":"1","animations":[{"id":"main"}]}"#,
        r#"{"animations":[{"id":"main"}]}"#,
    ] {
        let path = archive(
            &dir,
            manifest,
            &[("animations/main.json", sample_lottie().to_string())],
        );
        assert_eq!(
            probe(&path).unwrap().kind,
            AssetKind::LottieTemplate,
            "{manifest}"
        );
    }
}

#[test]
fn a_missing_animation_names_the_path_the_version_calls_for() {
    let dir = tempfile::tempdir().unwrap();
    let path = archive(
        &dir,
        V2,
        &[("animations/other.json", sample_lottie().to_string())],
    );
    let why = unsupported(&path);
    assert!(why.contains("a/main.json"), "{why}");
    let path = archive(&dir, V1, &[("a/main.json", sample_lottie().to_string())]);
    let why = unsupported(&path);
    assert!(why.contains("animations/main.json"), "{why}");
}

#[test]
fn an_unknown_manifest_version_is_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    for version in [r#""3""#, r#""abc""#, "2", "null", "[]"] {
        let manifest = format!(r#"{{"version":{version},"animations":[{{"id":"main"}}]}}"#);
        let path = archive(
            &dir,
            &manifest,
            &[("a/main.json", sample_lottie().to_string())],
        );
        let why = unsupported(&path);
        assert!(why.contains("dotLottie version"), "{version}: {why}");
    }
}
