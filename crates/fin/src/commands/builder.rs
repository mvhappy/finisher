//! `Builder`: accumulates ops for one transaction while tracking the state they produce.

use std::collections::BTreeMap;

use fin_core::{CoreError, ItemBody, MemIds, Op, Project, SCHEMA_VERSION, Tx, apply_tx};
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
    /// The first step a lenient build could not plan at all (it pushed no ops): answered from
    /// the log when the key already committed, else returned, never sent as a partial tx.
    unplanned: Option<CoreError>,
    /// Each planned step's command input, `{cmd, args}` ([`args_hash`]).
    pub(super) inputs: Vec<Value>,
    /// `--timeline`: the timeline commands that name none act on (default: the first).
    pub timeline: Option<String>,
    /// `--project`: the folder an agent session's file arguments are confined to (14c).
    pub project: Option<std::path::PathBuf>,
}

/// `$name` with a batch line's number: `$clip` → `$clip3`, `$split2` → `$split2_3`.
fn suffixed(placeholder: &str, suffix: &str) -> String {
    if placeholder.ends_with(|c: char| c.is_ascii_digit()) {
        format!("{placeholder}_{suffix}")
    } else {
        format!("{placeholder}{suffix}")
    }
}

/// Replaces every string of `value` that `names` maps (echo data of a composed step).
pub fn rename_json(value: &mut Value, names: &BTreeMap<String, String>) {
    match value {
        Value::String(s) => {
            if let Some(n) = names.get(s.as_str()) {
                *s = n.clone();
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| rename_json(v, names)),
        Value::Object(map) => map.values_mut().for_each(|v| rename_json(v, names)),
        _ => {}
    }
}

/// Whether `ops` may need font pins (plan 4C Decision 35): caption text or styles. Other ops
/// add no face and no script, so they are not sent to `fonts.plan`.
fn needs_fonts(ops: &[Op]) -> bool {
    ops.iter().any(|op| {
        let name = op.name();
        name.starts_with("caption.")
            || name.starts_with("style.")
            || match op {
                Op::ItemInsert(i) => matches!(i.body, ItemBody::Caption { .. }),
                Op::ItemRestore(r) => matches!(r.item.body, ItemBody::Caption { .. }),
                _ => false,
            }
    })
}

/// What `resolved.fonts` says about one planned pin: its op, key and source.
fn pin_echo(op: &Op) -> Value {
    let v = serde_json::to_value(op).unwrap_or(Value::Null);
    json!({ "op": op.name(), "key": v.get("key"), "source": v.pointer("/pin/source") })
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
            unplanned: None,
            inputs: Vec::new(),
            project: globals.project.clone(),
            timeline: globals.timeline.clone(),
        })
    }

    /// Keeps a step's planning error for [`Builder::finish`] when the build is lenient (a
    /// retry may find its tx committed: a composed command plans against a state its first
    /// attempt changed, e.g. `item unlink lk1` once lk1 is gone). Returns the error otherwise.
    pub(super) fn defer_step(&mut self, e: CoreError) -> Result<(), CoreError> {
        if self.lenient && exit_code(&e.code) == VALIDATION {
            self.unplanned.get_or_insert(e);
            return Ok(());
        }
        Err(e)
    }

    /// Pushes the ops of a `fin_core::compose` builder, planned against [`Builder::work`] (so
    /// naming working ids): ids this tx bound go back to the placeholders that bound them, and
    /// the builder's own placeholders (`$clip`, `$split`, …) gain `suffix` (a batch line's
    /// number; empty for one command). Returns the renaming, for the step's echo
    /// ([`rename_json`]).
    pub fn push_composed(
        &mut self,
        ops: Vec<Op>,
        suffix: &str,
    ) -> Result<BTreeMap<String, String>, CoreError> {
        let mut names: BTreeMap<String, String> = self
            .bound
            .iter()
            .map(|(placeholder, id)| (id.clone(), placeholder.clone()))
            .collect();
        if !suffix.is_empty() {
            for op in &ops {
                if let Some(p) = op.allocates() {
                    names.insert(p.to_string(), suffixed(p, suffix));
                }
            }
        }
        for mut op in ops {
            op.for_each_ref(|r| {
                if let Some(n) = names.get(r.as_str()) {
                    *r = n.clone();
                }
            });
            self.push(op)?;
        }
        Ok(names)
    }

    /// The `font.pin` ops the server plans for this tx (Decision 35), and what
    /// `resolved.fonts` says. Nothing for a tx without caption text or styles, or for a
    /// lenient build that already failed locally (a retry: its commit holds its pins);
    /// offline, nothing is planned and `measured` is false.
    fn plan_fonts(&self) -> Result<(Vec<Op>, Option<Value>), CoreError> {
        if !needs_fonts(&self.ops) || self.deferred.is_some() || self.unplanned.is_some() {
            return Ok((Vec::new(), None));
        }
        if self.backend.dry_run_only() {
            return Ok((
                Vec::new(),
                Some(json!({ "planned": [], "measured": false })),
            ));
        }
        let plan = self.backend.fonts_plan(&self.branch, &self.ops)?;
        let planned: Vec<Value> = plan.ops.iter().map(pin_echo).collect();
        Ok((
            plan.ops,
            Some(json!({ "planned": planned, "notes": plan.notes })),
        ))
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
        // A placeholder an earlier step bound names that entity (a later `keyframe.set` of it).
        let allocated = local
            .allocates()
            .filter(|p| !self.bound.contains_key(*p))
            .map(str::to_string);
        local.for_each_ref(|r| {
            if allocated.as_deref() != Some(r.as_str()) {
                *r = self.id(r);
            }
        });
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
            via: None,
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
        let dry = globals.dry_run || self.backend.dry_run_only();
        // A retry (`--idem-key`): ask what became of the key first. Already committed: that
        // commit is the answer (with the pins it was planned with). Set aside (lost to another
        // server's history, salvaged): it is reported, never sent again blindly.
        let earlier = match &globals.idem_key {
            Some(_) if !dry => lookup(self.backend, &tx.tx_id)?,
            _ => None,
        };
        if let (None, Some(e)) = (&earlier, &self.unplanned) {
            return Err(e.clone());
        }
        let mut fonts = None;
        if earlier.is_none() {
            let (pins, echo) = self.plan_fonts()?;
            fonts = echo;
            tx.ops.splice(0..0, pins);
        }
        let resolved = with_fonts(resolved, &fonts);
        if dry {
            let applied = apply_tx(
                &self.base.project,
                &tx,
                &mut MemIds::after(&self.base.project),
            )?;
            let mut doc = json!({
                "dryRun": true,
                "txId": tx.tx_id,
                "bindings": applied.bindings,
                "diff": applied.diff,
                "resolved": resolved,
            });
            batch_fonts(&mut doc, fonts);
            return Ok(doc);
        }
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
        let mut doc = json!({
            "rev": committed.rev,
            "txId": committed.tx.tx_id,
            "bindings": committed.bindings,
            "diff": committed.diff,
            "resolved": resolved,
        });
        batch_fonts(&mut doc, fonts);
        Ok(doc)
    }
}

/// `resolved.fonts` on a single command's echo (an object).
fn with_fonts(mut resolved: Value, fonts: &Option<Value>) -> Value {
    if let (Some(f), Some(map)) = (fonts, resolved.as_object_mut()) {
        map.insert("fonts".into(), f.clone());
    }
    resolved
}

/// A batch's echo is a list of steps: its fonts go on the document.
fn batch_fonts(doc: &mut Value, fonts: Option<Value>) {
    if let Some(f) = fonts
        && doc["resolved"].is_array()
    {
        doc["fonts"] = f;
    }
}
