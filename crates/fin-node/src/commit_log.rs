//! `log.commit` (plan D, Decision 22): one entry per attempt to append to the op log,
//! classified **inside `with_store`** from the typed result, the request and the store's write
//! note (`Store::take_write_outcome`), never from the reply JSON. Also `error.internal` and
//! `storage.state` (Decisions 26, 27), which are built from typed values the same way.
//!
//! The order of a logged call:
//! 1. [`Attempt::edit`] / [`Attempt::history`] (only when the journal's gate is on): the
//!    request's ids, its op counts for an edit, and under training a clone of the request tx
//!    (kept only for a failed edit's intent);
//! 2. the note is cleared, the store call runs, the note is read, under the same guard;
//! 3. the reply is encoded;
//! 4. the attempt is classified and the committed records are moved into the payload, so
//!    recording clones no tx on success.
//!
//! Outcome, in order: an error is `conflict` (`CONFLICT`) or `refused`, with its code; note
//! `Planned` (or a result that says `dryRun`) is `dryRun`; note `Deduped` is `deduped`;
//! nothing committed is `noOp`, with the result's reason; an edit whose committed `baseRev`
//! differs from the request's is `rebased` (`rebasedOver` the difference); else `accepted`.
//!
//! No napi here: the `commit_overhead` example includes this file (and `journal/ctx.rs`) to
//! measure the very code the engine runs.

use std::time::Instant;

use fin_core::events::names::{DETAIL_KEYS, OP_KINDS, RPC_METHODS};
use fin_core::events::payloads::{
    CommitOutcome, CommitRecord, Counts, ErrorInternal, LogCommit, StorageMode, StorageState,
    TxIntent,
};
use fin_core::events::{BranchId, Code, Ctx, Name, Source};
use fin_core::{CommittedTx, CoreError, Link, Op, Tx};
use fin_journal::Journal;
use fin_store::{
    BranchCreated, Generation, HistoryResult, MergeReport, RecoverReport, Store, StoreError,
    WriteOutcome,
};
use serde::Serialize;
use serde_json::Value;

use crate::journal::ctx::{
    RawActor, RawCtx, actor_of_meta, ctx_of, guard_of_meta, when_lost_of_meta,
};

/// The most txIds one entry links (an `undo --to` of many units, a large merge).
pub(crate) const MAX_LINKS: usize = 256;

/// What an entry adds to its payload (envelope fields and `ctx`), for the 32 KiB intent rule:
/// a generous bound, so an entry kept with its intent stays under it.
const ENVELOPE_BYTES: usize = 640;

/// The error codes `error.internal` records: `INTERNAL` and the store's unexpected ones.
const INTERNAL_CODES: [&str; 3] = ["INTERNAL", "DB_ERROR", "IO_ERROR"];

/// The request side of a history attempt, taken before the store call.
pub(crate) struct Req {
    pub(crate) tx_id: String,
    pub(crate) author: String,
    /// The branch reference the call names; kept only when it is a branch id.
    pub(crate) branch: Option<String>,
}

/// An attempt being logged: what the entry needs from before the store call. Made only while
/// the journal is on.
pub(crate) struct Attempt {
    journal: Journal,
    training: bool,
    record: CommitRecord,
    project: String,
    req: Req,
    actor: RawActor,
    ops: Counts<Name>,
    base_rev: Option<u64>,
    /// A failed edit's intent (training only): the request tx, normalised when it fails.
    request: Option<Tx>,
    started: Instant,
}

/// What a typed result says, for the classification and the payload.
pub(crate) trait Logged {
    /// The records it committed (or planned), in order.
    fn records(&self) -> &[CommittedTx];
    fn into_records(self) -> Vec<CommittedTx>;
    /// The result itself says it was a dry run.
    fn dry_run(&self) -> bool {
        false
    }
    /// Nothing was committed (a no-op pick or revert, an empty merge or recovery).
    fn nothing(&self) -> bool {
        self.records().is_empty()
    }
    /// Why nothing was committed, when the result says (`ALREADY_PRESENT`, ...).
    fn no_op(&self) -> Option<&str> {
        None
    }
    /// The raw txIds the call acts on.
    fn links(&self) -> Vec<&str> {
        vec![]
    }
    /// Conflict reasons the call reports without failing (a merge's skipped candidates).
    fn conflicts(&self) -> Vec<&str> {
        vec![]
    }
}

impl Logged for CommittedTx {
    fn records(&self) -> &[CommittedTx] {
        std::slice::from_ref(self)
    }
    fn into_records(self) -> Vec<CommittedTx> {
        vec![self]
    }
}

impl Logged for BranchCreated {
    fn records(&self) -> &[CommittedTx] {
        std::slice::from_ref(&self.record)
    }
    fn into_records(self) -> Vec<CommittedTx> {
        vec![self.record]
    }
    fn dry_run(&self) -> bool {
        self.dry_run
    }
}

impl Logged for HistoryResult {
    fn records(&self) -> &[CommittedTx] {
        &self.committed
    }
    fn into_records(self) -> Vec<CommittedTx> {
        self.committed
    }
    fn dry_run(&self) -> bool {
        self.dry_run
    }
    fn no_op(&self) -> Option<&str> {
        self.no_op.as_deref()
    }
    /// The units an undo undoes, the undo a redo reverses, the merge group a revert reverts
    /// (`undone`), and the origin a cherry-pick copies.
    fn links(&self) -> Vec<&str> {
        let picked = self
            .committed
            .iter()
            .filter_map(|c| match c.link.as_deref() {
                Some(Link::CherryPick { origin, .. }) => Some(origin.as_str()),
                _ => None,
            });
        self.undone
            .iter()
            .map(String::as_str)
            .chain(picked)
            .collect()
    }
}

impl Logged for MergeReport {
    fn records(&self) -> &[CommittedTx] {
        &self.committed
    }
    fn into_records(self) -> Vec<CommittedTx> {
        self.committed
    }
    fn dry_run(&self) -> bool {
        self.dry_run
    }
    /// The source txIds the merge copies.
    fn links(&self) -> Vec<&str> {
        self.merged.iter().map(String::as_str).collect()
    }
    fn conflicts(&self) -> Vec<&str> {
        self.skipped.iter().map(|c| c.reason.as_str()).collect()
    }
}

impl Logged for RecoverReport {
    fn records(&self) -> &[CommittedTx] {
        &self.committed
    }
    fn into_records(self) -> Vec<CommittedTx> {
        self.committed
    }
    fn dry_run(&self) -> bool {
        self.dry_run
    }
    /// A dismissal writes no record but sets rows aside: not nothing.
    fn nothing(&self) -> bool {
        self.committed.is_empty() && self.dismissed == 0
    }
    /// The records the recovery wrote.
    fn links(&self) -> Vec<&str> {
        self.committed.iter().map(|c| c.tx.tx_id.as_str()).collect()
    }
}

/// Whether an entry with `outcome` carries the intent (training only): what this attempt
/// committed (`accepted`, `rebased`), or the request of a failed one. A dry run's plan, a
/// deduped retry (the intent is the first attempt's entry's) and a no-op carry none.
fn keeps_intent(outcome: CommitOutcome) -> bool {
    matches!(outcome, CommitOutcome::Accepted | CommitOutcome::Rebased)
}

fn op_counts<'a>(ops: impl Iterator<Item = &'a Op>) -> Counts<Name> {
    let mut out = Counts::default();
    for op in ops {
        out.add(OP_KINDS.get_or_other(op.name()), 1);
    }
    out
}

fn code(s: &str) -> Code {
    Code::parse(s).unwrap_or_else(|| Code::parse("OTHER").expect("a valid code"))
}

/// Conflict reason → count, from `CONFLICT`'s `details.conflicts`.
fn conflict_counts<'a>(reasons: impl Iterator<Item = &'a str>) -> Counts<Code> {
    let mut out = Counts::default();
    for r in reasons {
        out.add(code(r), 1);
    }
    out
}

fn encode(value: &impl Serialize) -> Result<String, CoreError> {
    serde_json::to_string(value)
        .map_err(|e| CoreError::new("INTERNAL", format!("cannot encode result: {e}")))
}

/// What the store call ended with, for [`Attempt::emit`].
struct Done {
    outcome: CommitOutcome,
    code: Option<Code>,
    micros: u64,
    tx_id: String,
    branch: Option<String>,
    links: Vec<String>,
    conflicts: Counts<Code>,
    rebased_over: Option<u64>,
    /// The intent's txs (training only).
    intent: Option<Vec<Tx>>,
}

impl Attempt {
    fn start(
        j: &Journal,
        record: CommitRecord,
        project: &str,
        meta: Option<&str>,
        req: impl FnOnce() -> Option<Req>,
    ) -> Option<Attempt> {
        let gate = j.gate()?;
        let req = req()?;
        Some(Attempt {
            journal: j.clone(),
            training: gate.has_training(),
            record,
            project: project.to_owned(),
            req,
            actor: actor_of_meta(meta),
            ops: Counts::default(),
            base_rev: None,
            request: None,
            started: Instant::now(),
        })
    }

    /// An edit: its op counts and `baseRev` from the request, and under training a clone of
    /// it for a failure's intent.
    pub(crate) fn edit(j: &Journal, project: &str, tx: &Tx, meta: Option<&str>) -> Option<Self> {
        let mut a = Attempt::start(j, CommitRecord::Edit, project, meta, || {
            Some(Req {
                tx_id: tx.tx_id.clone(),
                author: tx.author.clone(),
                branch: Some(tx.branch.clone()),
            })
        })?;
        a.ops = op_counts(tx.ops.iter());
        a.base_rev = Some(tx.base_rev);
        if a.training {
            a.request = Some(tx.clone());
        }
        a.started = Instant::now();
        Some(a)
    }

    /// A branch, history or recovery call (`req`: `None` while the journal is off).
    pub(crate) fn history(
        j: &Journal,
        record: CommitRecord,
        project: &str,
        meta: Option<&str>,
        req: Option<Req>,
    ) -> Option<Self> {
        Attempt::start(j, record, project, meta, || req)
    }

    fn micros(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    /// A failed call: `conflict` or `refused`, with the request's normalised tx as the intent.
    pub(crate) fn failed(mut self, micros: u64, e: &CoreError) {
        let conflicts = e.details["conflicts"]
            .as_array()
            .map(|all| conflict_counts(all.iter().filter_map(|c| c["reason"].as_str())))
            .unwrap_or_default();
        let intent = self.request.take().map(|t| {
            let normal = fin_core::normalize(t.clone()).unwrap_or(t);
            vec![normal]
        });
        let tx_id = std::mem::take(&mut self.req.tx_id);
        let branch = self.req.branch.take();
        self.emit(Done {
            outcome: match e.code.as_str() {
                "CONFLICT" => CommitOutcome::Conflict,
                _ => CommitOutcome::Refused,
            },
            code: Some(code(&e.code)),
            micros,
            tx_id,
            branch,
            links: vec![],
            conflicts,
            rebased_over: None,
            intent,
        });
    }

    /// A committed edit (or its deduped retry).
    pub(crate) fn committed(self, note: Option<WriteOutcome>, micros: u64, c: CommittedTx) {
        let rebased_over = self
            .base_rev
            .filter(|b| *b != c.tx.base_rev)
            .map(|b| c.tx.base_rev.saturating_sub(b));
        let outcome = match note {
            Some(WriteOutcome::Planned) => CommitOutcome::DryRun,
            Some(WriteOutcome::Deduped) => CommitOutcome::Deduped,
            _ if rebased_over.is_some() => CommitOutcome::Rebased,
            _ => CommitOutcome::Accepted,
        };
        let training = self.training;
        self.emit(Done {
            outcome,
            code: None,
            micros,
            tx_id: c.tx.tx_id.clone(),
            branch: Some(c.tx.branch.clone()),
            links: vec![],
            conflicts: Counts::default(),
            rebased_over: rebased_over.filter(|_| outcome == CommitOutcome::Rebased),
            // A plan or a retry committed nothing now: no intent, as for history calls.
            intent: (training && keeps_intent(outcome)).then(|| vec![c.tx]),
        });
    }

    /// A branch, history or recovery result.
    fn done<R: Logged>(mut self, note: Option<WriteOutcome>, micros: u64, r: R) {
        let dry = note == Some(WriteOutcome::Planned) || r.dry_run();
        let outcome = if dry {
            CommitOutcome::DryRun
        } else if note == Some(WriteOutcome::Deduped) {
            CommitOutcome::Deduped
        } else if r.nothing() {
            CommitOutcome::NoOp
        } else {
            CommitOutcome::Accepted
        };
        let code = match outcome {
            CommitOutcome::NoOp => r.no_op().map(code),
            _ => None,
        };
        // The request's txId on every path (a merge's or recovery's is already `<id>-1`, its
        // first record's).
        let tx_id = std::mem::take(&mut self.req.tx_id);
        let branch = r
            .records()
            .first()
            .map(|c| c.tx.branch.clone())
            .or(self.req.branch.take());
        self.ops = op_counts(r.records().iter().flat_map(|c| c.tx.ops.iter()));
        let links = r
            .links()
            .into_iter()
            .take(MAX_LINKS)
            .map(str::to_owned)
            .collect();
        let conflicts = conflict_counts(r.conflicts().into_iter());
        let intent = (self.training && keeps_intent(outcome))
            .then(|| r.into_records().into_iter().map(|c| c.tx).collect());
        self.emit(Done {
            outcome,
            code,
            micros,
            tx_id,
            branch,
            links,
            conflicts,
            rebased_over: None,
            intent,
        });
    }

    fn emit(self, d: Done) {
        let j = &self.journal;
        let links = d.links.iter().filter_map(|l| j.id_ref(l)).collect();
        let mut p = LogCommit {
            record: self.record,
            outcome: d.outcome,
            code: d.code,
            ops: self.ops,
            base_rev: self.base_rev,
            rebased_over: d.rebased_over,
            conflicts: d.conflicts,
            links,
            micros: d.micros,
            intent: None,
            intent_omitted: None,
        };
        if let (Some(txs), Some(key)) = (d.intent, j.keyer()) {
            p.attach_intent(TxIntent::from_txs(txs, &*key), ENVELOPE_BYTES);
        }
        let raw = RawCtx {
            project: Some(self.project),
            branch: d.branch.filter(|b| BranchId::parse(b).is_some()),
            tx_id: Some(d.tx_id),
            author: Some(self.req.author),
            actor: Some(self.actor),
        };
        let ctx: Option<Ctx> = ctx_of(j, raw).ok();
        j.record(Source::FinNode, ctx, p.into());
    }
}

/// `tx.commit` inside `with_store`: the commit under `meta`'s write guard
/// ([`guard_of_meta`]; a malformed one is `INVALID_OP` before anything is logged or written),
/// `after` (the font rescan) on success, the reply encoded, then the `log.commit` entry.
/// Returns the `CommittedTx` JSON.
pub(crate) fn commit(
    s: &mut Store,
    j: &Journal,
    tx: Tx,
    generation: Option<&Generation>,
    meta: Option<&str>,
    after: impl FnOnce(&Store, &CommittedTx),
) -> Result<String, CoreError> {
    let guard = guard_of_meta(meta)?;
    let attempt = Attempt::edit(j, s.project_id(), &tx, meta);
    let _ = s.take_write_outcome();
    let r = s.with_guard(guard, |s| s.commit_at_generation(tx, generation));
    let note = s.take_write_outcome();
    let micros = attempt.as_ref().map_or(0, Attempt::micros);
    match r {
        Ok(c) => {
            after(s, &c);
            let json = encode(&c)?;
            if let Some(a) = attempt {
                a.committed(note, micros, c);
            }
            Ok(json)
        }
        Err(e) => {
            let e = CoreError::from(e);
            if let Some(a) = attempt {
                a.failed(micros, &e);
            }
            Err(e)
        }
    }
}

/// A branch, history or recovery call inside `with_store` (or a long call's store): `call`
/// under `meta`'s write guard (as [`commit`]), the reply encoded, then the `log.commit`
/// entry. `req`: the request's ids, `None` while the journal is off. Returns the result's
/// JSON.
pub(crate) fn history<R: Logged + Serialize>(
    s: &mut Store,
    j: &Journal,
    record: CommitRecord,
    meta: Option<&str>,
    req: Option<Req>,
    call: impl FnOnce(&mut Store) -> Result<R, StoreError>,
) -> Result<String, CoreError> {
    let guard = guard_of_meta(meta)?;
    let when_lost = when_lost_of_meta(meta);
    let attempt = Attempt::history(j, record, s.project_id(), meta, req);
    let _ = s.take_write_outcome();
    let r = s.with_guard(guard, |s| s.with_when_lost(when_lost, call));
    let note = s.take_write_outcome();
    let micros = attempt.as_ref().map_or(0, Attempt::micros);
    match r {
        Ok(r) => {
            let json = encode(&r)?;
            if let Some(a) = attempt {
                a.done(note, micros, r);
            }
            Ok(json)
        }
        Err(e) => {
            let e = CoreError::from(e);
            if let Some(a) = attempt {
                a.failed(micros, &e);
            }
            Err(e)
        }
    }
}

/// `error.internal` for a call that failed with `INTERNAL` or one of the store's unexpected
/// codes (`e`: the `CoreError` JSON). `call` is its RPC method (`other` when it has none);
/// only the keys of `details` are kept.
pub(crate) fn internal(j: &Journal, project: Option<&str>, call: &str, e: &Value) {
    if j.gate().is_none() {
        return;
    }
    let Some(c) = e["code"].as_str().filter(|c| INTERNAL_CODES.contains(c)) else {
        return;
    };
    let mut details: Vec<Name> = e["details"]
        .as_object()
        .into_iter()
        .flat_map(|d| d.keys())
        .map(|k| DETAIL_KEYS.get_or_other(k))
        .collect();
    details.sort();
    details.dedup();
    let p = ErrorInternal {
        call: RPC_METHODS.get_or_other(call),
        code: code(c),
        details,
    };
    let ctx = project.and_then(|p| {
        let raw = RawCtx {
            project: Some(p.to_owned()),
            ..RawCtx::default()
        };
        ctx_of(j, raw).ok()
    });
    j.record(Source::FinNode, ctx, p.into());
}

/// `storage.state` when a project's read-only reason changes (`was` → `now`, the
/// `ReadOnlyReason` codes): `readOnly` with the new reason, or `writable` with the one lifted.
pub(crate) fn storage_state(j: &Journal, project: &str, was: Option<&str>, now: Option<&str>) {
    if was == now || j.gate().is_none() {
        return;
    }
    let (state, reason) = match (now, was) {
        (Some(r), _) => (StorageMode::ReadOnly, r),
        (None, Some(r)) => (StorageMode::Writable, r),
        (None, None) => return,
    };
    let raw = RawCtx {
        project: Some(project.to_owned()),
        ..RawCtx::default()
    };
    let p = StorageState {
        state,
        reason: code(reason),
    };
    j.record(Source::FinNode, ctx_of(j, raw).ok(), p.into());
}
