//! `library.hash` (Decision 38): the whole-file BLAKE3 of each catalogued file, through
//! `open_for_read` and the read budget. Each item's value is `blake3:<hex>`; the kind's
//! `onSucceeded` writes the hashes to the catalog.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use fin_jobs::worker::JobCtx;
use fin_jobs::{JobError, ReadBudget, StartJob};
use fin_library::params::{FILES_FILE, LibraryHashParams, LibraryHashResult};
use fin_library::probe::{CANCELLED, CHANGED, FileRef, MISSING, PRESENT, UNREADABLE};
use fin_library::process::allow_dataless_materialisation;
use fin_library::{Throttle, hash_file};
use serde_json::Value;

use super::common::{
    CancelBridge, RootWatch, decode, grant, params_invalid, policy_name, root_path,
};
use super::jobfiles;
use super::probe::parse_items;

pub fn run(job: &StartJob, ctx: &mut JobCtx<'_>, warm: bool) -> Result<Value, JobError> {
    let p: LibraryHashParams = decode(job)?;
    let root = root_path(&p.root)?;
    if p.files != FILES_FILE {
        return Err(params_invalid("params.files: not a granted file"));
    }
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

    let mut states: BTreeMap<String, u64> = BTreeMap::new();
    let mut changed = 0u64;
    for (item, id) in &items {
        if throttle.is_cancelled() {
            return Err(JobError::cancelled());
        }
        ctx.begin(item);
        let Some(line) = lines.iter().find(|l| l.id == *id) else {
            ctx.item_failed(
                item,
                JobError::new("FILE_NOT_LISTED", "no line for this file"),
            );
            continue;
        };
        let display = root.join(line.rel.display());
        let outcome = hash_file(
            &FileRef {
                file: *id,
                root: &root,
                rel: &line.rel,
                expect: &line.expect,
                hydrate: p.hydrate,
                display: Path::new(&display),
            },
            &throttle,
        );
        match outcome.state.as_str() {
            CANCELLED => return Err(JobError::cancelled()),
            MISSING | UNREADABLE => watch.recheck()?,
            CHANGED => changed += 1,
            _ => {}
        }
        *states.entry(outcome.state.clone()).or_default() += 1;
        match (outcome.state.as_str(), outcome.hash) {
            (PRESENT, Some(hash)) => ctx.item(item, Some(Value::String(hash))),
            _ => {
                let code = outcome.code.unwrap_or_else(|| outcome.state.clone());
                ctx.item_failed(
                    item,
                    JobError::new(code, format!("not hashed: {}", outcome.state)),
                );
            }
        }
    }
    serde_json::to_value(LibraryHashResult {
        states,
        changed,
        dataless_policy: policy_name(),
    })
    .map_err(|e| JobError::new("INTERNAL", e.to_string()))
}
