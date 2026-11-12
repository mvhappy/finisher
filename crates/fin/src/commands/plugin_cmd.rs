//! `fin @<publisher>/<name> <command> --<arg> <value>…`, `fin help @<id>` and `fin plugin
//! commands [<id>]` (sp8b Task 10, spec §3.8). A plugin command's registry name is
//! `plugin:<id>/<command>`, the name 7B's command rules match and `commandOf` returns for
//! `plugin.command.run`.
//!
//! The command list comes from `plugin.commands` and is cached per server under the client's
//! cache directory, rewritten only when its `digest` changes. A run maps its command line with
//! the cached schema (`plugin_args`) and asks the server again only when the cache does not know
//! the command, the command line does not fit it, or the server refuses the name or arguments
//! (`PLUGIN_NOT_FOUND`, `PLUGIN_COMMAND_ARGS`: nothing ran) while the list has changed. Help is
//! asked of the server and falls back to the cache when the server is not reachable
//! (`source: cache`). The cache only shapes the command line and help: the server checks every
//! name and argument itself.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fin_core::CoreError;
use fin_plugin_api::protocol::rpc::{
    PluginCommandInfo, PluginCommandList, PluginCommandRunRequest, PluginCommandRunResult,
};
use serde_json::{Value, json};

use super::plugin_args::{map_args, usage_args};
use super::{Globals, target_branch};
use crate::backend::Backend;
use crate::output::{internal, usage};

/// Overrides the client's cache directory.
pub const CACHE_ENV: &str = "FIN_CACHE_DIR";

/// `fin [globals] @<id> [<command> [args…]]`, split from fin's own flags.
#[derive(Debug, PartialEq)]
pub struct Invocation {
    pub project: Option<PathBuf>,
    pub branch: Option<String>,
    pub dry_run: bool,
    pub id: String,
    pub command: Option<String>,
    pub args: Vec<String>,
}

/// The invocation when the first word after fin's global flags is `@…`; `None` for every other
/// command line (clap parses those).
pub fn split(args: &[OsString]) -> Option<Result<Invocation, CoreError>> {
    let words: Vec<String> = args
        .iter()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let mut inv = Invocation {
        project: None,
        branch: None,
        dry_run: false,
        id: String::new(),
        command: None,
        args: Vec::new(),
    };
    let mut i = 0;
    let mut seen_id = false;
    while i < words.len() {
        let w = &words[i];
        i += 1;
        let (flag, inline) = match w.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (w.as_str(), None),
        };
        let mut value = || match inline.clone() {
            Some(v) => Some(v),
            None => {
                let v = words.get(i).cloned();
                i += 1;
                v
            }
        };
        match flag {
            "--dry-run" if inline.is_none() => inv.dry_run = true,
            "--project" => match value() {
                Some(v) => inv.project = Some(PathBuf::from(v)),
                None => return seen_id.then(|| Err(needs(flag))),
            },
            "--branch" => match value() {
                Some(v) => inv.branch = Some(v),
                None => return seen_id.then(|| Err(needs(flag))),
            },
            "--idem-key" | "--timeline" if seen_id => {
                return Some(Err(usage(
                    format!("plugin commands take no {flag}"),
                    json!({ "flag": flag }),
                    "leave it out: a plugin command run is never retried",
                )));
            }
            "--idem-key" | "--timeline" => {
                value();
            }
            _ if !seen_id => {
                if !w.starts_with('@') {
                    return None;
                }
                inv.id = w.clone();
                seen_id = true;
            }
            _ if inv.command.is_none() && !w.starts_with('-') => inv.command = Some(w.clone()),
            _ => inv.args.push(w.clone()),
        }
    }
    seen_id.then_some(Ok(inv))
}

fn needs(flag: &str) -> CoreError {
    usage(
        format!("{flag} needs a value"),
        json!({ "flag": flag }),
        "run fin for the global flags",
    )
}

/// The registry name of a command.
pub fn name_of(id: &str, command: &str) -> String {
    format!("plugin:{id}/{command}")
}

/// The id part of `<id>@<version>`.
fn id_of(plugin: &str) -> &str {
    match plugin.get(1..).and_then(|r| r.rfind('@')) {
        Some(at) => &plugin[..at + 1],
        None => plugin,
    }
}

/// The client's cache directory: `FIN_CACHE_DIR`, else macOS `~/Library/Caches/Finisher`,
/// Windows `%LOCALAPPDATA%\Finisher\cache`, else `${XDG_CACHE_HOME:-~/.cache}/finisher`.
fn cache_root() -> Option<PathBuf> {
    let var = |n: &str| std::env::var_os(n).filter(|v| !v.is_empty());
    if let Some(dir) = var(CACHE_ENV) {
        return Some(PathBuf::from(dir));
    }
    let home = crate::paths::home();
    if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Caches/Finisher"))
    } else if cfg!(windows) {
        var("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|d| d.join("Finisher").join("cache"))
    } else {
        var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|h| h.join(".cache")))
            .map(|d| d.join("finisher"))
    }
}

/// The cache file of this server, as this client reaches it: keyed by `FIN_SERVER` (else the
/// server data directory) and the credential file, so a person's list and an agent's never mix.
pub fn cache_file() -> Option<PathBuf> {
    let server = match std::env::var_os("FIN_SERVER").filter(|v| !v.is_empty()) {
        Some(s) => format!("server:{}", s.to_string_lossy()),
        None => format!("data:{}", crate::endpoint::server_data()?.display()),
    };
    let credential = std::env::var_os(crate::auth::CREDENTIAL_ENV).unwrap_or_default();
    let key = format!("{server}\0{}", credential.to_string_lossy());
    let hex = blake3::hash(key.as_bytes()).to_hex();
    Some(
        cache_root()?
            .join("plugin-commands")
            .join(format!("{}.json", &hex[..32])),
    )
}

fn read_cache(file: &Path) -> Option<PluginCommandList> {
    serde_json::from_slice(&std::fs::read(file).ok()?).ok()
}

/// Writes the list unless the cache holds the same digest; a cache that cannot be written is
/// left as it is (a sandboxed agent may not write there).
fn write_cache(file: &Path, list: &PluginCommandList) {
    if read_cache(file).is_some_and(|c| c.digest == list.digest) {
        return;
    }
    let Some(dir) = file.parent() else { return };
    let tmp = dir.join(format!(".{}.tmp", std::process::id()));
    let ok = std::fs::create_dir_all(dir).is_ok()
        && serde_json::to_vec(list).is_ok_and(|b| std::fs::write(&tmp, b).is_ok())
        && std::fs::rename(&tmp, file).is_ok();
    if !ok {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// `plugin.commands`, cached.
fn fetch(backend: &dyn Backend) -> Result<PluginCommandList, CoreError> {
    let value = backend.server_call("plugin.commands", json!({}))?;
    let list: PluginCommandList = serde_json::from_value(value.clone()).map_err(|e| {
        internal(
            format!("plugin.commands answered an unexpected shape: {e}"),
            json!({ "answer": value }),
        )
    })?;
    if let Some(file) = cache_file() {
        write_cache(&file, &list);
    }
    Ok(list)
}

fn entry(c: &PluginCommandInfo) -> Value {
    let id = id_of(&c.plugin);
    let args = usage_args(c.args.as_value());
    let line = if args.is_empty() {
        format!("fin {id} {}", c.command)
    } else {
        format!("fin {id} {} {args}", c.command)
    };
    json!({
        "name": c.name, "plugin": c.plugin, "command": c.command, "usage": line,
        "title": c.title, "writes": c.writes, "timeoutS": c.timeout_s, "args": c.args,
    })
}

fn not_found(id: &str, command: Option<&str>, list: &PluginCommandList) -> CoreError {
    let of_plugin: Vec<&str> = list
        .commands
        .iter()
        .filter(|c| id_of(&c.plugin) == id)
        .map(|c| c.command.as_str())
        .collect();
    let (message, available) = match command {
        Some(cmd) if !of_plugin.is_empty() => {
            (format!("{id} has no command {cmd:?}"), json!(of_plugin))
        }
        _ => {
            let mut ids: Vec<&str> = list.commands.iter().map(|c| id_of(&c.plugin)).collect();
            ids.dedup();
            (
                format!("no enabled plugin {id} serves commands"),
                json!(ids),
            )
        }
    };
    let mut details = json!({ "id": id, "available": available });
    if let Some(cmd) = command {
        details["name"] = json!(name_of(id, cmd));
    }
    CoreError::new("PLUGIN_NOT_FOUND", message)
        .with_details(details)
        .with_fix("run fin plugin commands for every command you can run")
}

/// `fin plugin commands [<id>]` and `fin help @<id>`: `{digest, source, commands}`, each with
/// its `usage` line. Offline, the cached list (`source: cache`).
pub fn list(backend: &dyn Backend, id: Option<&str>) -> Result<Value, CoreError> {
    let (list, source) = match fetch(backend) {
        Ok(l) => (l, "server"),
        Err(e) if e.code == "SERVER_UNAVAILABLE" => {
            match cache_file().as_deref().and_then(read_cache) {
                Some(l) => (l, "cache"),
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let chosen: Vec<Value> = list
        .commands
        .iter()
        .filter(|c| id.is_none_or(|i| id_of(&c.plugin) == i))
        .map(entry)
        .collect();
    if let Some(i) = id
        && chosen.is_empty()
    {
        return Err(not_found(i, None, &list));
    }
    let mut doc = json!({ "digest": list.digest, "source": source, "commands": chosen });
    if let Some(i) = id {
        doc["plugin"] = json!(i);
    }
    Ok(doc)
}

/// The command and its mapped arguments from `list`, or why not.
fn resolve(list: &PluginCommandList, inv: &Invocation, command: &str) -> Result<Value, CoreError> {
    let name = name_of(&inv.id, command);
    let Some(c) = list.commands.iter().find(|c| c.name == name) else {
        return Err(not_found(&inv.id, Some(command), list));
    };
    map_args(&name, c.args.as_value(), &inv.args)
}

/// `fin @<id> <command> …`: `plugin.command.run` on the project and branch, its result as the
/// server answers it. `fin @<id>` alone lists the plugin's commands.
pub fn run(backend: &dyn Backend, inv: &Invocation) -> Result<Value, CoreError> {
    let Some(command) = inv.command.as_deref() else {
        return list(backend, Some(&inv.id));
    };
    let cached = cache_file().as_deref().and_then(read_cache);
    let (mut list, mut fresh) = match cached {
        Some(l) => (l, false),
        None => (fetch(backend)?, true),
    };
    let mut args = match resolve(&list, inv, command) {
        Ok(a) => a,
        Err(_) if !fresh => {
            list = fetch(backend)?;
            fresh = true;
            resolve(&list, inv, command)?
        }
        Err(e) => return Err(e),
    };
    let project_id = backend.project_id()?;
    let globals = Globals {
        project: inv.project.clone(),
        idem_key: None,
        dry_run: inv.dry_run,
        branch: inv.branch.clone(),
        timeline: None,
    };
    let branch = target_branch(backend, &globals)?;
    loop {
        let req = PluginCommandRunRequest {
            project_id: project_id.clone(),
            branch: Some(branch.clone()),
            name: name_of(&inv.id, command),
            args: args.clone(),
            dry_run: inv.dry_run.then_some(true),
        };
        let payload = serde_json::to_value(&req)
            .map_err(|e| internal(format!("cannot encode the run: {e}"), json!({})))?;
        match backend.server_call("plugin.command.run", payload) {
            Ok(v) => {
                let result: PluginCommandRunResult =
                    serde_json::from_value(v.clone()).map_err(|e| {
                        internal(
                            format!("plugin.command.run answered an unexpected shape: {e}"),
                            json!({ "answer": v }),
                        )
                    })?;
                return serde_json::to_value(result)
                    .map_err(|e| internal(format!("cannot encode the result: {e}"), json!({})));
            }
            // Nothing ran: a stale cache may have mapped the line; ask again once.
            Err(e)
                if !fresh
                    && matches!(e.code.as_str(), "PLUGIN_NOT_FOUND" | "PLUGIN_COMMAND_ARGS") =>
            {
                let before = list.digest.clone();
                list = fetch(backend)?;
                fresh = true;
                if list.digest == before {
                    return Err(e);
                }
                args = resolve(&list, inv, command)?;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split_str(line: &str) -> Option<Result<Invocation, CoreError>> {
        let args: Vec<OsString> = line.split(' ').map(OsString::from).collect();
        split(&args)
    }

    #[test]
    fn split_finds_the_plugin_after_fin_flags() {
        assert!(split_str("fin clip insert --track t1").is_none());
        assert!(split_str("fin --project p view").is_none());
        let inv = split_str("fin --project p @a/b go --x 1 --branch dev --dry-run --y")
            .unwrap()
            .unwrap();
        assert_eq!(inv.project, Some(PathBuf::from("p")));
        assert_eq!(inv.branch.as_deref(), Some("dev"));
        assert!(inv.dry_run);
        assert_eq!(inv.id, "@a/b");
        assert_eq!(inv.command.as_deref(), Some("go"));
        assert_eq!(inv.args, ["--x", "1", "--y"]);
        let bare = split_str("fin @a/b").unwrap().unwrap();
        assert_eq!(bare.command, None);
        assert_eq!(
            split_str("fin @a/b go --idem-key k")
                .unwrap()
                .unwrap_err()
                .code,
            "USAGE"
        );
        assert_eq!(
            split_str("fin @a/b go --branch").unwrap().unwrap_err().code,
            "USAGE"
        );
    }

    #[test]
    fn ids_and_names() {
        assert_eq!(id_of("@acme/tools@1.2.0"), "@acme/tools");
        assert_eq!(id_of("@acme/tools"), "@acme/tools");
        assert_eq!(name_of("@acme/tools", "go"), "plugin:@acme/tools/go");
    }
}
