//! `test.crash`: a batch kind. `begin`/`item` per item; it dies inside a poison item (before
//! that item's `begin` with `between`), or inside the second item of any run of two or more
//! with `crashOnSecond` (a crash no single item explains). Stateless: a run sees only the
//! items its params name (the engine's `restrict`).

use fin_jobs::test_kinds::{CrashParams, CrashResult};
use fin_jobs::worker::JobCtx;
use fin_jobs::{JobError, StartJob};
use serde_json::{Value, json};

use super::{die, params, sleep_or_cancel};

pub(crate) fn crash(job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
    let p: CrashParams = params(job)?;
    let token = ctx.cancel_token();
    let mut done = Vec::new();
    for (i, item) in p.items.iter().enumerate() {
        let poison = p.poison.contains(item);
        if poison && p.between {
            die(p.how);
        }
        ctx.begin(item);
        sleep_or_cancel(&token, p.item_ms.unwrap_or(0))?;
        if poison || (p.crash_on_second && p.items.len() >= 2 && i == 1) {
            die(p.how);
        }
        ctx.item(item, Some(json!({ "item": item })));
        done.push(item.clone());
    }
    serde_json::to_value(CrashResult { done }).map_err(|e| JobError::new("INTERNAL", e.to_string()))
}
