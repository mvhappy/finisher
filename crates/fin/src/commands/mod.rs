//! Command implementations. Writes are planned step by step against a working copy of the
//! snapshot (so `end`, `after:` and placeholders from earlier steps resolve), then sent as one
//! transaction — or, for `--dry-run` and the offline hook, applied locally with `MemIds`.

pub mod asset;
pub mod branch;
mod builder;
pub mod clip;
pub mod history;
pub mod project;
pub mod schema;
pub mod try_cmd;
pub mod view;

pub use builder::Builder;
use builder::with_detail;

use std::path::PathBuf;

use clap::Args;
use fin_core::Op;
use fin_core::{CommittedTx, CoreError};
use serde_json::{Value, json};

use crate::backend::{Backend, TxLookup};
use crate::client::IN_FLIGHT_CODES;
use crate::output::usage;

/// Flags accepted by every command.
#[derive(Debug, Clone, Default, Args)]
pub struct Globals {
    /// Project folder; default: the nearest ancestor holding finisher.project.json.
    #[arg(long, global = true, value_name = "dir")]
    pub project: Option<PathBuf>,
    /// Idempotency key; becomes the transaction id (for `project create`: a retry returns the
    /// project that create made).
    #[arg(long = "idem-key", global = true, value_name = "key")]
    pub idem_key: Option<String>,
    /// Validate and compute the result without committing.
    #[arg(long = "dry-run", global = true)]
    pub dry_run: bool,
    /// Branch to read and write: an id, a name or `<name>#<id>`. Default: the one `fin try`
    /// switched to (fin-config.json), else `main`.
    #[arg(long, global = true, value_name = "b")]
    pub branch: Option<String>,
}

/// The branch reference a command names: `--branch`, else the project's default in
/// `fin-config.json`, else `main`; and whether it came from the file.
pub fn configured_branch(
    backend: &dyn Backend,
    globals: &Globals,
) -> Result<(String, bool), CoreError> {
    if let Some(b) = &globals.branch {
        return Ok((b.clone(), false));
    }
    if let Some((sd, project_id)) = backend.config_scope()?
        && let Some(b) = crate::config::default_branch(&sd, &project_id)?
    {
        return Ok((b, true));
    }
    Ok(("main".to_string(), false))
}

/// The branch a command acts on ([`configured_branch`]). A default from the file that names
/// no branch any more is `BRANCH_NOT_FOUND` saying where it comes from and how to clear it.
pub fn target_branch(backend: &dyn Backend, globals: &Globals) -> Result<String, CoreError> {
    let (branch, from_config) = configured_branch(backend, globals)?;
    if from_config && !backend.dry_run_only() {
        let views = branch::list(backend, true)?;
        if branch::find(&views, &branch).is_err() {
            return Err(stale_default(&branch, &views));
        }
    }
    Ok(branch)
}

pub(crate) fn stale_default(branch: &str, views: &[Value]) -> CoreError {
    let mut e = branch::not_found(branch, views);
    e.message =
        format!("no branch {branch:?}: it is this project's default branch from fin-config.json");
    e.details["source"] = json!("fin-config.json");
    e.with_fix("fin branch default --clear (or fin try drop) resets the default to main; --branch <b> overrides it for one command")
}

/// The author of this command's records: `$FIN_AUTHOR` when set and not empty, else `fin`
/// (undo stacks are per author).
pub fn author() -> String {
    std::env::var("FIN_AUTHOR")
        .ok()
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| "fin".to_string())
}

/// One write command's input, as typed flags or as a `batch` line.
#[derive(Debug, Clone)]
pub enum Write {
    AssetAdd(asset::AddArgs),
    ClipInsert(clip::InsertArgs),
    ClipTrim(clip::TrimArgs),
    ClipDelete(clip::DeleteArgs),
}

/// `tx.get`: `Some` when `tx_id` committed, `None` when the server never saw it, and
/// `TX_SET_ASIDE` (exit 4) when it was set aside.
fn lookup(backend: &dyn Backend, tx_id: &str) -> Result<Option<CommittedTx>, CoreError> {
    match backend.tx_get(tx_id)? {
        TxLookup::Committed(c) => Ok(Some(*c)),
        TxLookup::Unknown => Ok(None),
        TxLookup::SetAside(aside) => {
            let reason = aside["reason"].as_str().unwrap_or("UNKNOWN").to_string();
            Err(CoreError::new(
                "TX_SET_ASIDE",
                format!("transaction {tx_id} was committed once but set aside ({reason})"),
            )
            .with_details(aside)
            .with_fix(
                "its edit is no longer in the project (fin project status counts the quarantine): read the project again and, if the edit is still wanted, run the command with a new --idem-key",
            ))
        }
    }
}

/// Whether a commit returned for this tx's id is this command's commit (possibly rebased) rather
/// than an earlier, different command sent with the same `--idem-key`. Rebasing keeps op count,
/// op kinds and the clip a trim or delete targets; insert positions may move.
fn same_command(planned: &[Op], committed: &[Op]) -> bool {
    planned.len() == committed.len()
        && planned.iter().zip(committed).all(|(p, c)| {
            p.name() == c.name()
                && match (p, c) {
                    (Op::ItemTrim(a), Op::ItemTrim(b)) => a.item == b.item,
                    (Op::ItemDelete(a), Op::ItemDelete(b)) => a.item == b.item,
                    _ => true,
                }
        })
}

fn idem_key_reused(committed: &CommittedTx) -> CoreError {
    CoreError::new(
        "IDEM_KEY_REUSED",
        format!(
            "--idem-key {} already committed a different command at rev {}",
            committed.tx.tx_id, committed.rev
        ),
    )
    .with_details(json!({
        "txId": committed.tx.tx_id,
        "rev": committed.rev,
        "committedOps": committed.tx.ops,
    }))
    .with_fix("use a new --idem-key for a different command")
}

/// Echoes what was committed rather than what was planned: the server may have rebased the tx
/// over concurrent commits (moving insert positions), or returned an earlier commit for the same
/// `--idem-key`. Each planned step pushed exactly one op, so steps and ops pair up in order.
fn echo_committed(resolved: &mut Value, ops: &[Op]) {
    let patch = |r: &mut Value, op: &Op| {
        if let (Op::ItemInsert(i), Some(map)) = (op, r.as_object_mut())
            && let fin_core::ItemBody::Clip { source_in, .. } = &i.body
        {
            map.insert("at".to_string(), json!(i.at));
            map.insert("in".to_string(), json!(source_in));
            map.insert("duration".to_string(), json!(i.duration));
            if let Ok(out) = source_in.checked_add(i.duration) {
                map.insert("out".to_string(), json!(out));
            }
        }
    };
    match resolved {
        Value::Array(steps) => {
            for (step, op) in steps.iter_mut().zip(ops) {
                if let Some(r) = step.get_mut("resolved") {
                    patch(r, op);
                }
            }
        }
        single => {
            if let Some(op) = ops.first() {
                patch(single, op);
            }
        }
    }
}

/// `blake3:<hex>` of the command input `inputs` (`[{cmd, args}]`, keys sorted): what
/// `Tx.argsHash` carries for an `--idem-key` commit. It hashes what the user asked for, not
/// the ops: a retry plans its ops against a newer snapshot (`end`, `after:` and rebases move
/// positions), and must still count as the same command.
pub fn args_hash(inputs: &[Value]) -> String {
    // `serde_json::Map` keeps keys sorted (no `preserve_order`), so this is canonical.
    let bytes = serde_json::to_vec(inputs).expect("JSON values serialize");
    format!("blake3:{}", blake3::hash(&bytes).to_hex())
}

/// One step's command input for [`args_hash`]: its args as given, defaults left out, except
/// that a media path is the file it names: canonical (`..` and symlinks resolved) when it
/// exists, else absolute. The same words from another directory name another file, and
/// other words (`../media/a.mov` inside `media/`) the same one.
fn input(step: &Write) -> Value {
    let (cmd, args) = match step {
        Write::AssetAdd(a) => {
            let path = crate::paths::resolve_str(&a.path);
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            ("asset.add", json!({ "path": path.display().to_string() }))
        }
        Write::ClipInsert(a) => ("clip.insert", json!(a)),
        Write::ClipTrim(a) => ("clip.trim", json!(a)),
        Write::ClipDelete(a) => ("clip.delete", json!(a)),
    };
    json!({ "cmd": cmd, "args": args })
}

/// Plans one write step; `n` numbers placeholders in a batch (`$clip3`), `None` for a single
/// command (`$clip`). Returns the step's resolved values.
pub fn plan(b: &mut Builder<'_>, step: &Write, n: Option<usize>) -> Result<Value, CoreError> {
    b.inputs.push(input(step));
    let suffix = n.map(|n| n.to_string()).unwrap_or_default();
    match step {
        Write::AssetAdd(a) => asset::plan_add(b, a, &format!("$asset{suffix}")),
        Write::ClipInsert(a) => clip::plan_insert(b, a, &format!("$clip{suffix}")),
        Write::ClipTrim(a) => clip::plan_trim(b, a),
        Write::ClipDelete(a) => clip::plan_delete(b, a),
    }
}

/// Runs one write command.
pub fn write(backend: &dyn Backend, globals: &Globals, step: Write) -> Result<Value, CoreError> {
    let mut b = Builder::new(backend, globals)?;
    let resolved = plan(&mut b, &step, None)?;
    b.finish(globals, resolved)
}

fn batch_step(cmd: &str, args: Value) -> Result<Write, CoreError> {
    let parse_err = |e: serde_json::Error| {
        usage(
            format!("invalid args for {cmd}: {e}"),
            json!({ "cmd": cmd }),
            format!("see `fin schema {cmd}`"),
        )
    };
    Ok(match cmd {
        "asset.add" => Write::AssetAdd(serde_json::from_value(args).map_err(parse_err)?),
        "clip.insert" => Write::ClipInsert(serde_json::from_value(args).map_err(parse_err)?),
        "clip.trim" => Write::ClipTrim(serde_json::from_value(args).map_err(parse_err)?),
        "clip.delete" => Write::ClipDelete(serde_json::from_value(args).map_err(parse_err)?),
        other => {
            return Err(usage(
                format!("{other} cannot be batched"),
                json!({ "cmd": other, "alternatives": crate::registry::BATCHABLE }),
                "batch only write commands",
            ));
        }
    })
}

/// `fin batch`: JSONL of `{ "cmd", "args" }` on stdin → one transaction. Line `n` binds
/// `$clipN` / `$assetN`.
pub fn batch(backend: &dyn Backend, globals: &Globals, input: &str) -> Result<Value, CoreError> {
    let mut steps = Vec::new();
    for (i, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let n = i + 1;
        let at_line = |e: CoreError| with_detail(e, "line", json!(n));
        let entry: Value = serde_json::from_str(line).map_err(|e| {
            at_line(usage(
                format!("line {n} is not JSON: {e}"),
                json!({}),
                "send one {\"cmd\",\"args\"} object per line",
            ))
        })?;
        let Some(cmd) = entry.get("cmd").and_then(Value::as_str) else {
            return Err(at_line(usage(
                format!("line {n} has no \"cmd\""),
                json!({}),
                "add \"cmd\": \"clip.insert\" or similar",
            )));
        };
        let args = entry.get("args").cloned().unwrap_or_else(|| json!({}));
        steps.push((n, cmd.to_string(), batch_step(cmd, args).map_err(at_line)?));
    }
    if steps.is_empty() {
        return Err(usage(
            "batch received no commands",
            json!({}),
            "pipe JSONL to stdin: {\"cmd\":\"clip.delete\",\"args\":{\"clip\":\"c1\"}}",
        ));
    }
    let mut b = Builder::new(backend, globals)?;
    let mut resolved = Vec::with_capacity(steps.len());
    for (n, cmd, step) in &steps {
        let r = plan(&mut b, step, Some(*n)).map_err(|e| with_detail(e, "line", json!(n)))?;
        resolved.push(json!({ "line": n, "cmd": cmd, "resolved": r }));
    }
    b.finish(globals, Value::Array(resolved))
}

#[cfg(test)]
mod tests;
