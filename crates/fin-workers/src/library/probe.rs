//! `library.probe` (Decisions 20, 34, 36, 37): opens each catalogued file once through
//! `open_for_read`, computes `fp1` and the shallow probe from that handle, and reports the
//! outcomes in groups of up to 32 through `call library.probed`, acknowledging each item
//! only after the reply.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use fin_jobs::worker::JobCtx;
use fin_jobs::{JobError, ReadBudget, StartJob};
use fin_library::params::{
    CALL_PROBED, KIND_FILTER, LibraryProbeParams, LibraryProbeResult, PROBED_MAX, ProbedCall,
};
use fin_library::probe::{CANCELLED, CHANGED, FileRef, MISSING, UNREADABLE};
use fin_library::process::allow_dataless_materialisation;
use fin_library::{ProbeOutcome, Throttle, probe_file};
use serde_json::Value;

use super::common::{
    CancelBridge, RootWatch, decode, extension_listed, grant, params_invalid, policy_name,
    root_path,
};
use super::jobfiles;

/// A group of finished files waiting for their `library.probed` call.
struct Group {
    ids: Vec<String>,
    results: Vec<ProbeOutcome>,
}

impl Group {
    /// Reports the group and acknowledges its items; an error leaves them unacknowledged.
    fn flush(&mut self, ctx: &mut JobCtx<'_>) -> Result<(), JobError> {
        if self.ids.is_empty() {
            return Ok(());
        }
        let params = serde_json::to_value(ProbedCall {
            results: std::mem::take(&mut self.results),
        })
        .map_err(|e| JobError::new("INTERNAL", e.to_string()))?;
        ctx.call(CALL_PROBED, params)?;
        for id in self.ids.drain(..) {
            ctx.item(&id, None);
        }
        Ok(())
    }
}

/// Item ids as numbers: `(item, file id)`; an item that is no id is returned apart.
pub fn parse_items(items: &[String]) -> (Vec<(String, i64)>, Vec<String>) {
    let (mut good, mut bad) = (Vec::new(), Vec::new());
    for item in items {
        match item.parse::<i64>() {
            Ok(id) => good.push((item.clone(), id)),
            Err(_) => bad.push(item.clone()),
        }
    }
    (good, bad)
}

pub fn run(job: &StartJob, ctx: &mut JobCtx<'_>, warm: bool) -> Result<Value, JobError> {
    let p: LibraryProbeParams = decode(job)?;
    let root = root_path(&p.root)?;
    if p.files != fin_library::params::FILES_FILE {
        return Err(params_invalid("params.files: not a granted file"));
    }
    // Only a cold job of a hydrating library lets the OS download (Decision 6).
    if p.hydrate && !warm {
        allow_dataless_materialisation()
            .map_err(|e| JobError::new("IO_ERROR", format!("dataless policy: {e}")))?;
    }
    let mut watch = RootWatch::new(&root, &p.volume);
    watch.start()?;
    let (items, bad) = parse_items(&p.items);
    for item in &bad {
        ctx.item_failed(
            item,
            JobError::new("JOB_ITEM_INVALID", "an item is a file id"),
        );
    }
    let wanted: HashSet<i64> = items.iter().map(|(_, id)| *id).collect();
    let lines = jobfiles::file_lines(&grant(ctx, &p.files)?, &wanted)?;
    let bridge = CancelBridge::new(ctx.cancel_token());
    let budget = p
        .read_bps
        .map(ReadBudget::new)
        .or_else(|| ctx.read_budget());
    let throttle = Throttle::new(budget, Some(bridge.token()));

    let mut group = Group {
        ids: Vec::new(),
        results: Vec::new(),
    };
    let mut states: BTreeMap<String, u64> = BTreeMap::new();
    let mut changed = 0u64;
    let mut stopped = false;
    for (item, id) in &items {
        if throttle.is_cancelled() {
            stopped = true;
            break;
        }
        ctx.begin(item);
        let Some(line) = lines.iter().find(|l| l.id == *id) else {
            ctx.item_failed(
                item,
                JobError::new("FILE_NOT_LISTED", "no line for this file"),
            );
            continue;
        };
        let outcome =
            if extension_listed(line.rel.name().unwrap_or_default(), p.extensions.as_deref()) {
                let display = root.join(line.rel.display());
                probe_file(
                    &FileRef {
                        file: *id,
                        root: &root,
                        rel: &line.rel,
                        expect: &line.expect,
                        hydrate: p.hydrate,
                        display: Path::new(&display),
                    },
                    &throttle,
                )
            } else {
                ProbeOutcome::skipped(*id, KIND_FILTER)
            };
        match outcome.state.as_str() {
            // Never stored and never acknowledged: the job is ending.
            CANCELLED => {
                stopped = true;
                break;
            }
            MISSING | UNREADABLE => {
                // A dead mount looks like missing and unreadable files; nothing is recorded
                // against an offline root (Decision 18).
                if let Err(offline) = watch.recheck() {
                    group.flush(ctx)?;
                    return Err(offline);
                }
            }
            CHANGED => changed += 1,
            _ => {}
        }
        *states.entry(outcome.state.clone()).or_default() += 1;
        group.ids.push(item.clone());
        group.results.push(outcome);
        if group.ids.len() >= PROBED_MAX {
            group.flush(ctx)?;
        }
    }
    group.flush(ctx)?;
    if stopped || bridge.token().is_cancelled() {
        return Err(JobError::cancelled());
    }
    serde_json::to_value(LibraryProbeResult {
        states,
        changed,
        dataless_policy: policy_name(),
    })
    .map_err(|e| JobError::new("INTERNAL", e.to_string()))
}
