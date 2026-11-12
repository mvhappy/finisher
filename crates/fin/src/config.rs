//! `<server-data>/fin-config.json`: the per-project default branch (`--branch` falls back to
//! it) and the try branches `fin try` made, per project
//! (`tries: {<projectId>: {<branchId>: <parentId>}}`). Written temp-then-rename, 0600 on unix. A file this fin cannot read is an error, never a silent
//! `main`: writes would otherwise land on the wrong branch.

use std::path::Path;
use std::time::{Duration, Instant};

use fin_core::CoreError;
use serde_json::{Map, Value, json};

use crate::output::internal;

pub const FILE: &str = "fin-config.json";

fn unreadable(path: &Path, why: impl std::fmt::Display) -> CoreError {
    internal(
        format!("cannot use {}: {why}", path.display()),
        json!({ "path": path.display().to_string() }),
    )
    .with_fix("fix the JSON in that file, or delete it to forget the default branch and tries")
}

fn load(sd: &Path) -> Result<Value, CoreError> {
    let path = sd.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(e) => return Err(unreadable(&path, e)),
    };
    let doc = serde_json::from_str::<Value>(&text).map_err(|e| unreadable(&path, e))?;
    check(&doc).map_err(|why| unreadable(&path, why))?;
    Ok(doc)
}

/// The file's shape (review #25: a section or an entry of the wrong type is a broken file, not
/// an absent one): an object whose `defaultBranch`, when present, maps project ids to branch
/// references (non-empty strings) and whose `tries`, when present, maps project ids to
/// `{<branchId>: <parentId>}` objects of non-empty strings. Other keys are left alone.
fn check(doc: &Value) -> Result<(), String> {
    let root = doc.as_object().ok_or("not a JSON object")?;
    let strings = |at: &str, v: &Value| -> Result<(), String> {
        let map = v.as_object().ok_or(format!("{at} is not an object"))?;
        for (k, v) in map {
            if !v.as_str().is_some_and(|s| !s.is_empty()) {
                return Err(format!(
                    "{at}.{k} is not a branch reference (a non-empty string)"
                ));
            }
        }
        Ok(())
    };
    if let Some(d) = root.get("defaultBranch") {
        strings("defaultBranch", d)?;
    }
    if let Some(t) = root.get("tries") {
        let map = t.as_object().ok_or("tries is not an object")?;
        for (project, v) in map {
            strings(&format!("tries.{project}"), v)?;
        }
    }
    Ok(())
}

fn save(sd: &Path, doc: &Value) -> Result<(), CoreError> {
    let path = sd.join(FILE);
    let tmp = sd.join(format!("{FILE}.tmp.{}", std::process::id()));
    let io = |e: std::io::Error| unreadable(&path, e);
    std::fs::create_dir_all(sd).map_err(io)?;
    let mut text = serde_json::to_string_pretty(doc).expect("JSON values serialize");
    text.push('\n');
    std::fs::write(&tmp, text).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(io)?;
    }
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(e)
    })
}

fn section<'a>(doc: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    let root = doc.as_object_mut().expect("load returns an object");
    let entry = root.entry(key).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    entry.as_object_mut().expect("just made an object")
}

/// Runs a read-modify-write of the file under `fin-config.json.lock` (created exclusively; a
/// lock older than [`STALE`] belongs to a fin that died and is taken over).
fn locked<T>(sd: &Path, f: impl FnOnce() -> Result<T, CoreError>) -> Result<T, CoreError> {
    const STALE: Duration = Duration::from_secs(30);
    let path = sd.join(format!("{FILE}.lock"));
    std::fs::create_dir_all(sd).map_err(|e| unreadable(&path, e))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let old = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > STALE);
                if old {
                    let _ = std::fs::remove_file(&path);
                } else if Instant::now() > deadline {
                    return Err(unreadable(&path, "another fin holds the lock"));
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            Err(e) => return Err(unreadable(&path, e)),
        }
    }
    let result = f();
    let _ = std::fs::remove_file(&path);
    result
}

fn update(sd: &Path, f: impl FnOnce(&mut Value)) -> Result<(), CoreError> {
    locked(sd, || {
        let mut doc = load(sd)?;
        section(&mut doc, "tries");
        section(&mut doc, "defaultBranch");
        f(&mut doc);
        save(sd, &doc)
    })
}

/// The default branch reference stored for `project_id`.
pub fn default_branch(sd: &Path, project_id: &str) -> Result<Option<String>, CoreError> {
    Ok(load(sd)?["defaultBranch"][project_id]
        .as_str()
        .map(str::to_string))
}

/// Sets (`Some`) or clears (`None`) the default branch of `project_id`.
pub fn set_default_branch(
    sd: &Path,
    project_id: &str,
    branch: Option<&str>,
) -> Result<(), CoreError> {
    update(sd, |doc| {
        let map = section(doc, "defaultBranch");
        match branch {
            Some(b) => map.insert(project_id.to_string(), json!(b)),
            None => map.remove(project_id),
        };
    })
}

/// The try branches of `project_id`: `tries: {<projectId>: {<branchId>: <parentId>}}`.
fn project_tries<'a>(doc: &'a mut Value, project_id: &str) -> &'a mut Map<String, Value> {
    let tries = section(doc, "tries");
    let entry = tries.entry(project_id).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    entry.as_object_mut().expect("just made an object")
}

/// Records `branch_id` as a try branch of `project_id` made from `parent_id`.
pub fn record_try(
    sd: &Path,
    project_id: &str,
    branch_id: &str,
    parent_id: &str,
) -> Result<(), CoreError> {
    update(sd, |doc| {
        project_tries(doc, project_id).insert(branch_id.to_string(), json!(parent_id));
    })
}

/// The parent a try branch of `project_id` was made from, `None` for a branch `fin try` did
/// not make.
pub fn try_parent(
    sd: &Path,
    project_id: &str,
    branch_id: &str,
) -> Result<Option<String>, CoreError> {
    Ok(load(sd)?["tries"][project_id][branch_id]
        .as_str()
        .map(str::to_string))
}

/// Forgets a try branch once it was kept or dropped.
pub fn forget_try(sd: &Path, project_id: &str, branch_id: &str) -> Result<(), CoreError> {
    update(sd, |doc| {
        project_tries(doc, project_id).remove(branch_id);
        if project_tries(doc, project_id).is_empty() {
            section(doc, "tries").remove(project_id);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_branch_and_tries_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let sd = dir.path();
        assert_eq!(default_branch(sd, "p1").unwrap(), None);
        set_default_branch(sd, "p1", Some("br2")).unwrap();
        set_default_branch(sd, "p2", Some("x")).unwrap();
        assert_eq!(default_branch(sd, "p1").unwrap().as_deref(), Some("br2"));
        set_default_branch(sd, "p1", None).unwrap();
        assert_eq!(default_branch(sd, "p1").unwrap(), None);
        assert_eq!(default_branch(sd, "p2").unwrap().as_deref(), Some("x"));
        record_try(sd, "p1", "br2", "main").unwrap();
        record_try(sd, "p2", "br2", "br1").unwrap();
        assert_eq!(
            try_parent(sd, "p1", "br2").unwrap().as_deref(),
            Some("main")
        );
        assert_eq!(try_parent(sd, "p2", "br2").unwrap().as_deref(), Some("br1"));
        assert_eq!(try_parent(sd, "p1", "br3").unwrap(), None);
        forget_try(sd, "p1", "br2").unwrap();
        assert_eq!(try_parent(sd, "p1", "br2").unwrap(), None);
        // Forgetting again (a retried `fin try drop`) leaves no empty entry behind.
        forget_try(sd, "p1", "br2").unwrap();
        assert!(load(sd).unwrap()["tries"].get("p1").is_none());
        assert_eq!(try_parent(sd, "p2", "br2").unwrap().as_deref(), Some("br1"));
        assert!(!sd.join("fin-config.json.lock").exists());
        let doc: Value =
            serde_json::from_str(&std::fs::read_to_string(sd.join(FILE)).unwrap()).unwrap();
        assert!(doc["defaultBranch"].is_object() && doc["tries"].is_object());
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        set_default_branch(dir.path(), "p", Some("b")).unwrap();
        let mode = std::fs::metadata(dir.path().join(FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn concurrent_updates_all_land() {
        let dir = tempfile::tempdir().unwrap();
        let sd = dir.path().to_path_buf();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let sd = sd.clone();
                std::thread::spawn(move || record_try(&sd, "p", &format!("br{i}"), "main").unwrap())
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        for i in 0..8 {
            assert!(try_parent(&sd, "p", &format!("br{i}")).unwrap().is_some());
        }
    }

    #[test]
    fn a_broken_file_is_an_error_not_main() {
        let dir = tempfile::tempdir().unwrap();
        for bad in [
            "{nope",
            "[]",
            r#"{"defaultBranch":42}"#,
            r#"{"defaultBranch":{"p":false}}"#,
            r#"{"defaultBranch":{"p":""}}"#,
            r#"{"defaultBranch":{"other":7}}"#,
            r#"{"tries":[]}"#,
            r#"{"tries":{"p":"br1"}}"#,
            r#"{"tries":{"p":{"br1":null}}}"#,
        ] {
            std::fs::write(dir.path().join(FILE), bad).unwrap();
            assert!(default_branch(dir.path(), "p").is_err(), "{bad}");
            assert!(try_parent(dir.path(), "p", "br1").is_err(), "{bad}");
            assert!(
                set_default_branch(dir.path(), "p", Some("b")).is_err(),
                "{bad}"
            );
            assert!(set_default_branch(dir.path(), "p", None).is_err(), "{bad}");
            assert!(record_try(dir.path(), "p", "br1", "main").is_err(), "{bad}");
            assert_eq!(std::fs::read_to_string(dir.path().join(FILE)).unwrap(), bad);
        }
    }

    #[test]
    fn absent_sections_and_entries_are_none() {
        let dir = tempfile::tempdir().unwrap();
        for good in ["{}", r#"{"defaultBranch":{},"tries":{}}"#, r#"{"other":1}"#] {
            std::fs::write(dir.path().join(FILE), good).unwrap();
            assert_eq!(default_branch(dir.path(), "p").unwrap(), None, "{good}");
            assert_eq!(try_parent(dir.path(), "p", "br1").unwrap(), None, "{good}");
        }
    }
}
