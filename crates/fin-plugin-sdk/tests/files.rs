//! `files` (feature): checked reads of `in/` files and declared-size writes to `out/`.

use std::path::Path;

use fin_jobs::{FileGrant, FileMode};
use fin_plugin_sdk::files::{create_output, read_input};
use serde_json::json;

fn grant(path: &Path, mode: FileMode, bytes: Option<u64>) -> FileGrant {
    FileGrant {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        path: path.to_owned(),
        mode,
        bytes,
    }
}

#[test]
fn inputs_are_read_checked_and_capped() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let file = root.join("pcm.bin");
    std::fs::write(&file, b"abcdef").unwrap();
    assert_eq!(
        read_input(&grant(&file, FileMode::Read, None), 64).unwrap(),
        b"abcdef"
    );
    // An input that is not what the host granted is the host's fault: `PLUGIN_PROTOCOL
    // {reason, file}`, the file by its grant name, never its path.
    let e = read_input(&grant(&file, FileMode::Read, None), 3).unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.details.clone()),
        (
            "PLUGIN_PROTOCOL",
            json!({ "reason": "size", "file": "pcm.bin" })
                .as_object()
                .unwrap()
                .clone()
        )
    );
    let e = read_input(&grant(&file, FileMode::Read, Some(8)), 64).unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.details["reason"].clone()),
        ("PLUGIN_PROTOCOL", json!("size"))
    );
    let sub = root.join("d");
    std::fs::create_dir(&sub).unwrap();
    let e = read_input(&grant(&sub, FileMode::Read, None), 64).unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.details["reason"].clone()),
        ("PLUGIN_PROTOCOL", json!("notRegular"))
    );
    let e = read_input(&grant(&file, FileMode::Write, None), 64).unwrap_err();
    assert_eq!(e.code, "PLUGIN_PERMISSION_DENIED");
    assert_eq!(
        (e.details["perm"].clone(), e.details["access"].clone()),
        (json!("fs"), json!("read"))
    );
    #[cfg(unix)]
    {
        let link = root.join("link.bin");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        let e = read_input(&grant(&link, FileMode::Read, None), 64).unwrap_err();
        assert_eq!(e.code, "PLUGIN_PROTOCOL", "a link is never followed");
        assert_eq!(e.details["reason"], "link");
        assert!(
            !e.details
                .values()
                .any(|v| v.as_str().is_some_and(|s| s.contains('/')))
        );
        // An output under a linked directory: `PLUGIN_OUTPUT_INVALID {reason: link}`.
        let linked = root.join("out-link");
        std::os::unix::fs::symlink(&sub, &linked).unwrap();
        let out = linked.join("o.bin");
        let e = create_output(&grant(&out, FileMode::Write, None), 6)
            .err()
            .unwrap();
        assert_eq!(
            (e.code.as_str(), e.details["reason"].clone()),
            ("PLUGIN_OUTPUT_INVALID", json!("link"))
        );
    }
}

#[test]
fn outputs_take_exactly_their_declared_size() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let out = root.join("artifact.json");
    let g = grant(&out, FileMode::Write, None);

    let mut f = create_output(&g, 4).unwrap();
    f.write_all(b"ab").unwrap();
    f.write_all(b"cd").unwrap();
    assert_eq!(f.write_all(b"e").unwrap_err().details["reason"], "size");
    assert_eq!(f.finish().unwrap(), out);
    assert_eq!(std::fs::read(&out).unwrap(), b"abcd");

    let mut f = create_output(&g, 4).unwrap();
    f.write_all(b"ab").unwrap();
    let e = f.finish().unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.details["reason"].as_str()),
        ("PLUGIN_OUTPUT_INVALID", Some("truncated"))
    );

    let sized = grant(&out, FileMode::Write, Some(8));
    assert_eq!(
        create_output(&sized, 4).err().unwrap().details["reason"],
        "size"
    );
    let read_only = grant(&out, FileMode::Read, None);
    let e = create_output(&read_only, 4).err().unwrap();
    assert_eq!(e.code, "PLUGIN_PERMISSION_DENIED");
    assert_eq!(
        (e.details["perm"].clone(), e.details["access"].clone()),
        (json!("fs"), json!("write"))
    );
}
