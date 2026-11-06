//! `<server-data>/fin-config.json`: the per-project default branch (`--branch` falls back to
//! it) and the try branches `fin try` made (`tries: {<branchId>: <parentId>}`). Written
//! temp-then-rename, 0600 on unix. A file this fin cannot read is an error, never a silent
//! `main`: writes would otherwise land on the wrong branch.

use std::path::Path;

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
    match serde_json::from_str::<Value>(&text) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => Err(unreadable(&path, "not a JSON object")),
        Err(e) => Err(unreadable(&path, e)),
    }
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
    let mut doc = load(sd)?;
    let map = section(&mut doc, "defaultBranch");
    match branch {
        Some(b) => map.insert(project_id.to_string(), json!(b)),
        None => map.remove(project_id),
    };
    section(&mut doc, "tries");
    save(sd, &doc)
}

/// Records `branch_id` as a try branch made from `parent_id`.
pub fn record_try(sd: &Path, branch_id: &str, parent_id: &str) -> Result<(), CoreError> {
    let mut doc = load(sd)?;
    section(&mut doc, "tries").insert(branch_id.to_string(), json!(parent_id));
    section(&mut doc, "defaultBranch");
    save(sd, &doc)
}

/// The parent a try branch was made from, `None` for a branch `fin try` did not make.
pub fn try_parent(sd: &Path, branch_id: &str) -> Result<Option<String>, CoreError> {
    Ok(load(sd)?["tries"][branch_id].as_str().map(str::to_string))
}

/// Forgets a try branch once it was kept or dropped.
pub fn forget_try(sd: &Path, branch_id: &str) -> Result<(), CoreError> {
    let mut doc = load(sd)?;
    section(&mut doc, "tries").remove(branch_id);
    section(&mut doc, "defaultBranch");
    save(sd, &doc)
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
        record_try(sd, "br2", "main").unwrap();
        assert_eq!(try_parent(sd, "br2").unwrap().as_deref(), Some("main"));
        assert_eq!(try_parent(sd, "br3").unwrap(), None);
        forget_try(sd, "br2").unwrap();
        assert_eq!(try_parent(sd, "br2").unwrap(), None);
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
    fn a_broken_file_is_an_error_not_main() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), "{nope").unwrap();
        assert!(default_branch(dir.path(), "p").is_err());
        assert!(set_default_branch(dir.path(), "p", Some("b")).is_err());
    }
}
