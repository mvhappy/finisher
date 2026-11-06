//! `Builder`: accumulates ops for one transaction while tracking the state they produce.

use std::collections::BTreeMap;

use fin_core::{CoreError, MemIds, Op, Project, SCHEMA_VERSION, Tx, apply_tx};
use serde_json::{Value, json};

use super::{
    Globals, IN_FLIGHT_CODES, args_hash, author, echo_committed, idem_key_reused, lookup,
    same_command, target_branch,
};
use crate::backend::{Backend, Snapshot};
use crate::output::{VALIDATION, exit_code, usage};

/// Accumulates ops for one transaction while tracking the state they produce.
pub struct Builder<'a> {
    pub backend: &'a dyn Backend,
    base: Snapshot,
    /// The branch reference the tx is committed to.
    branch: String,
    author: String,
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
    /// Each planned step's command input, `{cmd, args}` ([`args_hash`]).
    pub(super) inputs: Vec<Value>,
}

pub(super) fn with_detail(mut e: CoreError, key: &str, value: Value) -> CoreError {
    if let Some(map) = e.details.as_object_mut() {
        map.insert(key.to_string(), value);
    }
    e
}

impl<'a> Builder<'a> {
    pub fn new(backend: &'a dyn Backend, globals: &Globals) -> Result<Builder<'a>, CoreError> {
        let branch = target_branch(backend, globals)?;
        let base = backend.snapshot(&branch)?;
        let ids = MemIds::after(&base.project);
        let lenient = globals.idem_key.is_some() && !globals.dry_run && !backend.dry_run_only();
        Ok(Builder {
            backend,
            work: base.project.clone(),
            base,
            branch,
            author: author(),
            ids,
            bound: BTreeMap::new(),
            ops: Vec::new(),
            lenient,
            deferred: None,
            inputs: Vec::new(),
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
        // New-entity placeholders stay as written: the work copy binds them itself.
        let allocated = local.allocates().map(str::to_string);
        for r in local.refs_mut() {
            if allocated.as_deref() != Some(r.as_str()) {
                *r = self.id(r);
            }
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
            author: self.author.clone(),
            branch: self.branch.clone(),
            base_rev: self.base.rev,
            schema_version: SCHEMA_VERSION,
            args_hash: None,
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
        if globals.idem_key.is_some() {
            tx.args_hash = Some(args_hash(&self.inputs));
        }
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
        // A retry (`--idem-key`): ask what became of the key first. Already committed: that
        // commit is the answer. Set aside (lost to another server's history, salvaged): it is
        // reported, never sent again blindly.
        let earlier = match &globals.idem_key {
            Some(_) => lookup(self.backend, &tx.tx_id)?,
            None => None,
        };
        let generation = self.base.generation.as_ref();
        let sent = match earlier {
            Some(c) => Ok(c),
            None => self.backend.commit(&tx, generation),
        };
        let committed = match (sent, self.deferred) {
            (Ok(committed), _) => committed,
            // The answer was lost (timeout, dropped connection): ask whether it landed.
            (Err(e), _) if IN_FLIGHT_CODES.contains(&e.code.as_str()) => {
                match lookup(self.backend, &tx.tx_id) {
                    Ok(Some(c)) => c,
                    Err(aside) if aside.code == "TX_SET_ASIDE" => return Err(aside),
                    _ => return Err(e),
                }
            }
            // The server also rejected it as invalid, so it was not a retry after all: report the
            // local error, which names the failing input. Any other server answer (conflict,
            // shutting down, internal, transport) is the more useful one and keeps its exit code.
            (Err(e), Some(local)) if exit_code(&e.code) == VALIDATION => return Err(local),
            (Err(e), _) => return Err(e),
        };
        // The command input's hash decides when both have one; a commit without one (an older
        // fin, another client) is compared by its ops.
        let reused = match (&tx.args_hash, &committed.tx.args_hash) {
            (Some(ours), Some(theirs)) => ours != theirs,
            _ => !same_command(&tx.ops, &committed.tx.ops),
        };
        if reused {
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
