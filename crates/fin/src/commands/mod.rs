//! Command implementations. Writes are planned step by step against a working copy of the
//! snapshot (so `end`, `after:` and placeholders from earlier steps resolve), then sent as one
//! transaction — or, for `--dry-run` and the offline hook, applied locally with `MemIds`.

pub mod asset;
pub mod clip;
pub mod project;
pub mod schema;
pub mod view;

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Args;
use fin_core::{CommittedTx, CoreError};
use fin_core::{MemIds, Op, Project, SCHEMA_VERSION, Tx, apply_tx};
use serde_json::{Value, json};

use crate::backend::{Backend, Snapshot};
use crate::output::{VALIDATION, exit_code, usage};

/// Flags accepted by every command.
#[derive(Debug, Clone, Default, Args)]
pub struct Globals {
    /// Project folder; default: the nearest ancestor holding finisher.project.json.
    #[arg(long, global = true, value_name = "dir")]
    pub project: Option<PathBuf>,
    /// Idempotency key; becomes the transaction id.
    #[arg(long = "idem-key", global = true, value_name = "key")]
    pub idem_key: Option<String>,
    /// Validate and compute the result without committing.
    #[arg(long = "dry-run", global = true)]
    pub dry_run: bool,
}

/// One write command's input, as typed flags or as a `batch` line.
#[derive(Debug, Clone)]
pub enum Write {
    AssetAdd(asset::AddArgs),
    ClipInsert(clip::InsertArgs),
    ClipTrim(clip::TrimArgs),
    ClipDelete(clip::DeleteArgs),
}

/// Accumulates ops for one transaction while tracking the state they produce.
pub struct Builder<'a> {
    pub backend: &'a dyn Backend,
    base: Snapshot,
    /// The base project with every planned op applied.
    pub work: Project,
    ids: MemIds,
    bound: BTreeMap<String, String>,
    ops: Vec<Op>,
    /// `--idem-key` on a real commit: an op that fails local validation is still sent, because
    /// it may be a retry of a tx that already committed (the server then returns that commit).
    lenient: bool,
    /// The first local validation error a lenient build let through.
    deferred: Option<CoreError>,
}

fn with_detail(mut e: CoreError, key: &str, value: Value) -> CoreError {
    if let Some(map) = e.details.as_object_mut() {
        map.insert(key.to_string(), value);
    }
    e
}

impl<'a> Builder<'a> {
    pub fn new(backend: &'a dyn Backend, globals: &Globals) -> Result<Builder<'a>, CoreError> {
        let base = backend.snapshot()?;
        let ids = MemIds::after(&base.project);
        let lenient = globals.idem_key.is_some() && !globals.dry_run && !backend.dry_run_only();
        Ok(Builder {
            backend,
            work: base.project.clone(),
            base,
            ids,
            bound: BTreeMap::new(),
            ops: Vec::new(),
            lenient,
            deferred: None,
        })
    }

    /// The id a reference names in the working copy: placeholders bound by earlier steps map to
    /// their working ids, anything else is returned unchanged.
    pub fn id(&self, reference: &str) -> String {
        self.bound
            .get(reference)
            .cloned()
            .unwrap_or_else(|| reference.to_string())
    }

    /// Validates `op` against the working copy and appends it to the transaction.
    pub fn push(&mut self, op: Op) -> Result<(), CoreError> {
        let mut local = op.clone();
        match &mut local {
            Op::ClipInsert { track, asset, .. } => {
                *track = self.id(track);
                *asset = self.id(asset);
            }
            Op::ClipTrim { clip, .. } | Op::ClipDelete { clip } => *clip = self.id(clip),
            _ => {}
        }
        let tx = self.tx(vec![local]);
        let applied = match apply_tx(&self.work, &tx, &mut self.ids) {
            Ok(applied) => applied,
            Err(e) => {
                let e = with_detail(e, "opIndex", json!(self.ops.len()));
                if !self.lenient {
                    return Err(e);
                }
                self.deferred.get_or_insert(e);
                self.ops.push(op);
                return Ok(());
            }
        };
        self.bound.extend(applied.bindings);
        self.work = applied.project;
        self.ops.push(op);
        Ok(())
    }

    fn tx(&self, ops: Vec<Op>) -> Tx {
        Tx {
            tx_id: String::new(),
            author: "fin".to_string(),
            branch: "main".to_string(),
            base_rev: self.base.rev,
            schema_version: SCHEMA_VERSION,
            ops,
        }
    }

    /// Commits (or dry-runs) the planned ops and builds the write document.
    pub fn finish(self, globals: &Globals, resolved: Value) -> Result<Value, CoreError> {
        let tx_id = match &globals.idem_key {
            Some(key) if key.trim().is_empty() => {
                return Err(usage(
                    "--idem-key must not be empty",
                    json!({ "field": "idem-key" }),
                    "pass a non-empty key",
                ));
            }
            Some(key) => key.clone(),
            None => ulid::Ulid::new().to_string(),
        };
        let mut tx = self.tx(self.ops.clone());
        tx.tx_id = tx_id;
        if globals.dry_run || self.backend.dry_run_only() {
            let applied = apply_tx(
                &self.base.project,
                &tx,
                &mut MemIds::after(&self.base.project),
            )?;
            return Ok(json!({
                "dryRun": true,
                "txId": tx.tx_id,
                "bindings": applied.bindings,
                "diff": applied.diff,
                "resolved": resolved,
            }));
        }
        let committed = match (self.backend.commit(&tx), self.deferred) {
            (Ok(committed), _) => committed,
            // The server also rejected it as invalid, so it was not a retry after all: report the
            // local error, which names the failing input. Any other server answer (conflict,
            // shutting down, internal, transport) is the more useful one and keeps its exit code.
            (Err(e), Some(local)) if exit_code(&e.code) == VALIDATION => return Err(local),
            (Err(e), _) => return Err(e),
        };
        if !same_command(&tx.ops, &committed.tx.ops) {
            return Err(idem_key_reused(&committed));
        }
        let mut resolved = resolved;
        echo_committed(&mut resolved, &committed.tx.ops);
        Ok(json!({
            "rev": committed.rev,
            "txId": committed.tx.tx_id,
            "bindings": committed.bindings,
            "diff": committed.diff,
            "resolved": resolved,
        }))
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
                    (Op::ClipTrim { clip: a, .. }, Op::ClipTrim { clip: b, .. })
                    | (Op::ClipDelete { clip: a }, Op::ClipDelete { clip: b }) => a == b,
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
        if let (
            Op::ClipInsert {
                at,
                source_in,
                duration,
                ..
            },
            Some(map),
        ) = (op, r.as_object_mut())
        {
            map.insert("at".to_string(), json!(at));
            map.insert("in".to_string(), json!(source_in));
            map.insert("duration".to_string(), json!(duration));
            if let Ok(out) = source_in.checked_add(*duration) {
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

/// Plans one write step; `n` numbers placeholders in a batch (`$clip3`), `None` for a single
/// command (`$clip`). Returns the step's resolved values.
pub fn plan(b: &mut Builder<'_>, step: &Write, n: Option<usize>) -> Result<Value, CoreError> {
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
mod tests {
    use std::path::Path;

    use fin_core::{CommittedTx, Rational, new_project};

    use super::*;
    use crate::backend::Probed;

    /// An empty project at rev 1 whose server rejects every commit with `answer`.
    struct Rejecting {
        answer: CoreError,
    }

    impl Backend for Rejecting {
        fn snapshot(&self) -> Result<Snapshot, CoreError> {
            let rate: Rational = "30".parse()?;
            Ok(Snapshot {
                rev: 1,
                project: new_project("p".into(), "P".into(), rate)?,
            })
        }
        fn commit(&self, _tx: &Tx) -> Result<CommittedTx, CoreError> {
            Err(self.answer.clone())
        }
        fn probe(&self, _path: &Path) -> Result<Probed, CoreError> {
            unreachable!()
        }
        fn create_project(
            &self,
            _dir: &Path,
            _name: &str,
            _rate: Rational,
        ) -> Result<Value, CoreError> {
            unreachable!()
        }
        fn open_project(&self, _dir: &Path) -> Result<Value, CoreError> {
            unreachable!()
        }
    }

    fn delete_missing_with_key(answer: CoreError) -> CoreError {
        let globals = Globals {
            idem_key: Some("k".into()),
            ..Globals::default()
        };
        let step = Write::ClipDelete(clip::DeleteArgs { clip: "c9".into() });
        write(&Rejecting { answer }, &globals, step).unwrap_err()
    }

    #[test]
    fn a_deferred_local_error_only_replaces_server_validation_errors() {
        let server_invalid = CoreError::new("NOT_FOUND", "server says no");
        let local = delete_missing_with_key(server_invalid);
        assert_eq!(local.code, "NOT_FOUND");
        assert_ne!(local.message, "server says no");
        assert_eq!(local.details["opIndex"], 0);
        for code in [
            "CONFLICT",
            "BASE_REV_AHEAD",
            "SERVER_SHUTTING_DOWN",
            "ENGINE_CLOSED",
            "INTERNAL",
            "SERVER_TIMEOUT",
        ] {
            let e = delete_missing_with_key(CoreError::new(code, "server"));
            assert_eq!((e.code.as_str(), e.message.as_str()), (code, "server"));
        }
        let no_key = write(
            &Rejecting {
                answer: CoreError::new("CONFLICT", "server"),
            },
            &Globals::default(),
            Write::ClipDelete(clip::DeleteArgs { clip: "c9".into() }),
        );
        assert_eq!(no_key.unwrap_err().code, "NOT_FOUND");
    }

    #[test]
    fn a_rebased_commit_is_the_same_command_but_another_kind_or_clip_is_not() {
        let r = |s: &str| s.parse::<Rational>().unwrap();
        let insert = |at: &str| Op::ClipInsert {
            id: "$clip".into(),
            track: "t1".into(),
            at: r(at),
            asset: "a1".into(),
            source_in: r("0"),
            duration: r("2"),
        };
        let delete = |clip: &str| Op::ClipDelete { clip: clip.into() };
        assert!(same_command(&[insert("0")], &[insert("4")]));
        assert!(same_command(&[delete("c1")], &[delete("c1")]));
        assert!(!same_command(&[insert("0")], &[delete("c1")]));
        assert!(!same_command(&[delete("c1")], &[delete("c2")]));
        assert!(!same_command(
            &[delete("c1")],
            &[delete("c1"), delete("c2")]
        ));
    }
}
