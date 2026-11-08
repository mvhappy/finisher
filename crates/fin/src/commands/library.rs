//! `fin library ls|status|scan|files|show|search|set` (plan 5D Task 14, spec §7.6): the server's
//! indexed libraries, each command one `library.*` RPC, so offline every one is
//! `SERVER_UNAVAILABLE`. A library is named by its id or its name (the server resolves both).
//! Paths below a library's root are relative to it, `/`-separated; an absolute path under the
//! root is turned into one here (the server refuses absolute paths), one outside it is
//! `INVALID_PATH`.
//!
//! - `fin library ls`: `{libraries}`, each with its state, counts and warnings.
//! - `fin library status <lib>`: the library, its running scans, per-stage progress and its watch.
//! - `fin library scan <lib> [--full] [--dir <path>]…`: queues a scan and returns `{job}` at
//!   once (`fin wait <job>` waits for it): a full scan, a targeted one of the directories, or
//!   else a poll. A paused or offline library answers `LIBRARY_PAUSED` or `LIBRARY_OFFLINE`
//!   with the reason in `details`.
//! - `fin library files <lib> [<dir>] [--state <s>] [--kind <k>] [--limit <n>] [--cursor <c>]`:
//!   one folder's subfolders and files (`next` is the next page's cursor).
//! - `fin library show <lib> (<path> | --id <fileId>)`: one file with its probe and stages;
//!   several files with one name (case or Unicode twins) are `PATH_AMBIGUOUS`: ask by `--id`.
//! - `fin library search …` and `fin search …`: see [`super::search`].
//! - `fin library set <lib> [--name <n>] [--include <glob>]… [--exclude <glob>]… [--kinds
//!   all|<k>,…] [--poll demand|<s>] [--full daily|weekly|manual] [--pause|--resume]`: the
//!   fields an agent may change (`library.scan`). `--no-include` and `--no-exclude` clear the
//!   filters. The fields only a person decides (`--hydrate`, `--bandwidth`, `--alias`,
//!   `--no-aliases`, `--accept-volume`, `--accept-unmounted <dir>`, `--accept-empty-root`) are
//!   listed too: `fin` sends them and the server refuses them with `HUMAN_ONLY` (exit 7); the
//!   owner's console does them. Adding and removing libraries is the console's alone.

use clap::{Args, Subcommand};
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::library_set::{SetArgs, set_payload};
use super::search::{self, SearchArgs};
use crate::backend::Backend;
use crate::output::{unavailable, usage};

/// The file states `--state` takes.
pub const FILE_STATES: &[&str] = &[
    "present",
    "missing",
    "cloudOnly",
    "unreadable",
    "probeFailed",
    "skipped",
    "offline",
];
/// The most files or folders a `fin library files` page holds.
const FILES_LIMIT_MAX: u32 = 500;

#[derive(Debug, Clone, Subcommand)]
pub enum LibraryCmd {
    /// The server's libraries.
    Ls,
    /// One library: state, counts, running scans, stage progress, watch.
    Status(RefArgs),
    /// Queues a scan and returns its job.
    Scan(ScanArgs),
    /// One folder's subfolders and files.
    Files(FilesArgs),
    /// One file with its probe and stages.
    Show(ShowArgs),
    /// Names and folders across the libraries, ranked.
    Search(SearchArgs),
    /// Changes a library's settings.
    Set(SetArgs),
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RefArgs {
    /// The library's id or name.
    pub lib: String,
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScanArgs {
    /// The library's id or name.
    pub lib: String,
    /// Scan every folder again (default: a poll of what changed).
    #[arg(long)]
    #[serde(default)]
    pub full: bool,
    /// Scan only this folder (repeatable; a path below the root).
    #[arg(long, value_name = "path")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dir: Vec<String>,
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilesArgs {
    /// The library's id or name.
    pub lib: String,
    /// The folder, below the root (default: the root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// Only files in this state (present, missing, cloudOnly, unreadable, probeFailed, skipped,
    /// offline).
    #[arg(long, value_name = "s")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Only this kind of file.
    #[arg(long, value_name = "k")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// At most this many (1 to 500, default 100).
    #[arg(long, value_name = "n")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next` of the previous page.
    #[arg(long, value_name = "c")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowArgs {
    /// The library's id or name.
    pub lib: String,
    /// The file's path below the root (or absolute, under it).
    #[arg(conflicts_with = "id")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The file's id (from a listing or a search; the answer to `PATH_AMBIGUOUS`).
    #[arg(long, value_name = "fileId")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
}

/// Every library command needs the server: offline it is `SERVER_UNAVAILABLE`.
pub fn offline(operation: &str) -> CoreError {
    unavailable(json!({ "mode": "offline", "operation": operation }))
        .with_fix("libraries live on the server: start finisherd and run it again")
}

/// The components of `path`, split at `/` and `\`, empty ones dropped.
fn parts(path: &str) -> Vec<&str> {
    path.split(['/', '\\']).filter(|p| !p.is_empty()).collect()
}

/// Whether `path` is absolute on some OS (`/x`, `\\x`, `C:\x`, `C:/x`).
fn is_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with('/')
        || path.starts_with('\\')
        || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// `path` below `root` as a relative `/`-separated path (empty for the root itself); `None`
/// when it is not under it. A Windows-style root (a drive or `\\server\share`) compares
/// without regard to case, as its volume does.
pub fn below_root(root: &str, path: &str) -> Option<String> {
    let (r, p) = (parts(root), parts(path));
    let windows = root.contains('\\') || root.as_bytes().get(1) == Some(&b':');
    let same = |a: &str, b: &str| {
        if windows {
            a.to_lowercase() == b.to_lowercase()
        } else {
            a == b
        }
    };
    if p.len() < r.len() || !r.iter().zip(&p).all(|(a, b)| same(a, b)) {
        return None;
    }
    Some(p[r.len()..].join("/"))
}

/// What the server calls `path`: itself when relative, else relative to the library's root
/// (found with one `library.status`).
fn rel(backend: &dyn Backend, lib: &str, path: &str) -> Result<String, CoreError> {
    if !is_absolute(path) {
        return Ok(path.to_string());
    }
    let status = backend.server_call("library.status", json!({ "id": lib }))?;
    let root = status["library"]["root"].as_str().unwrap_or_default();
    below_root(root, path).ok_or_else(|| {
        CoreError::new("INVALID_PATH", "the path is not inside the library's root")
            .with_details(json!({ "path": path }))
            .with_fix("name a path below the library's root, or a relative one")
    })
}

fn check_state(state: &str) -> Result<(), CoreError> {
    if FILE_STATES.contains(&state) {
        return Ok(());
    }
    Err(usage(
        format!("--state {state:?} is not a file state"),
        json!({ "state": state, "states": FILE_STATES }),
        format!("use one of {}", FILE_STATES.join(", ")),
    ))
}

pub fn run(backend: &dyn Backend, cmd: &LibraryCmd) -> Result<Value, CoreError> {
    // Flags are checked first, so a bad one is a usage error with or without a server.
    let (tag, payload): (&str, Value) = match cmd {
        LibraryCmd::Search(a) => return search::run(backend, a),
        LibraryCmd::Ls => ("library.list", json!({})),
        LibraryCmd::Status(a) => ("library.status", json!({ "id": a.lib })),
        LibraryCmd::Set(a) => ("library.set", set_payload(a)?),
        LibraryCmd::Scan(a) => ("library.scan", scan_payload(backend, a)?),
        LibraryCmd::Files(a) => ("library.files", files_payload(backend, a)?),
        LibraryCmd::Show(a) => ("library.file", show_payload(backend, a)?),
    };
    backend.server_call(tag, payload)
}

fn scan_payload(backend: &dyn Backend, a: &ScanArgs) -> Result<Value, CoreError> {
    if backend.dry_run_only() {
        return Err(offline("library.scan"));
    }
    let mut p = json!({ "id": a.lib });
    if a.full {
        p["full"] = json!(true);
    }
    if !a.dir.is_empty() {
        let dirs: Result<Vec<String>, CoreError> =
            a.dir.iter().map(|d| rel(backend, &a.lib, d)).collect();
        p["dirs"] = json!(dirs?);
    }
    Ok(p)
}

fn files_payload(backend: &dyn Backend, a: &FilesArgs) -> Result<Value, CoreError> {
    if let Some(s) = &a.state {
        check_state(s)?;
    }
    if let Some(n) = a.limit
        && !(1..=FILES_LIMIT_MAX).contains(&n)
    {
        return Err(usage(
            format!("--limit {n} is outside 1 to {FILES_LIMIT_MAX}"),
            json!({ "limit": n, "max": FILES_LIMIT_MAX }),
            format!("ask for at most {FILES_LIMIT_MAX} entries and page with --cursor"),
        ));
    }
    if backend.dry_run_only() {
        return Err(offline("library.files"));
    }
    let mut p = json!({ "id": a.lib });
    if let Some(d) = &a.dir {
        p["dir"] = json!(rel(backend, &a.lib, d)?);
    }
    for (key, v) in [
        ("state", &a.state),
        ("kind", &a.kind),
        ("cursor", &a.cursor),
    ] {
        if let Some(v) = v {
            p[key] = json!(v);
        }
    }
    if let Some(n) = a.limit {
        p["limit"] = json!(n);
    }
    Ok(p)
}

fn show_payload(backend: &dyn Backend, a: &ShowArgs) -> Result<Value, CoreError> {
    if a.path.is_none() && a.id.is_none() {
        return Err(usage(
            "fin library show takes a path or --id <fileId>",
            json!({ "lib": a.lib }),
            "run fin library show <lib> <path>, or fin library show <lib> --id <fileId>",
        ));
    }
    if backend.dry_run_only() {
        return Err(offline("library.file"));
    }
    let mut p = json!({ "id": a.lib });
    match (&a.path, a.id) {
        (Some(path), _) => p["path"] = json!(rel(backend, &a.lib, path)?),
        (None, Some(id)) => p["fileId"] = json!(id),
        (None, None) => {}
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_paths_under_the_root_become_relative() {
        assert_eq!(
            below_root("/mnt/nas/raw", "/mnt/nas/raw/clips/a.mov").as_deref(),
            Some("clips/a.mov")
        );
        assert_eq!(
            below_root("/mnt/nas/raw/", "/mnt/nas/raw").as_deref(),
            Some("")
        );
        assert_eq!(below_root("/mnt/nas/raw", "/mnt/nas/other/a.mov"), None);
        assert_eq!(below_root("/mnt/nas/raw", "/mnt/nas/rawer/a.mov"), None);
        assert_eq!(below_root("/mnt/nas/raw", "/MNT/nas/raw/a.mov"), None);
        // A Windows-style root compares without case and takes either separator.
        assert_eq!(
            below_root(r"D:\Media", "d:/media/Clips/a.mov").as_deref(),
            Some("Clips/a.mov")
        );
        assert_eq!(
            below_root(r"\\nas\media", r"\\NAS\Media\a.mov").as_deref(),
            Some("a.mov")
        );
        assert!(is_absolute("/x") && is_absolute(r"C:\x") && is_absolute(r"\\nas\x"));
        assert!(!is_absolute("clips/a.mov") && !is_absolute("a.mov"));
    }
}
