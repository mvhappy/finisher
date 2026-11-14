//! `test.stage` (feature `test-hooks`; plan 5D Task 11): a library stage kind for the stage
//! framework's tests ([`fin_library::params_test::TestStageParams`]). Its items are
//! fingerprints; each names one `StageFile` in `params.files`, which the worker opens **only**
//! through [`open_for_read`] with that file's raw chain and `Expect`, as every stage worker
//! must (Decision 39). It reads up to `readBytes` and answers `{fileId, bytes}` per item.

use std::io::Read;

use fin_jobs::worker::JobCtx;
use fin_jobs::{JobError, StartJob};
use fin_library::params_test::TestStageParams;
use fin_library::{OpenError, open_for_read};
use serde_json::{Value, json};

use super::common::{decode, params_invalid, root_path};

fn refusal(e: &OpenError) -> &'static str {
    match e {
        OpenError::CloudOnly => "FILE_CLOUD_ONLY",
        OpenError::Changed => "FILE_CHANGED",
        OpenError::NotFound => "FILE_NOT_FOUND",
        OpenError::Escapes | OpenError::NameSurrogate => "INVALID_PATH",
        OpenError::Io(e) if e.kind() == std::io::ErrorKind::NotFound => "FILE_NOT_FOUND",
        OpenError::Io(_) => "ASSET_UNREADABLE",
    }
}

pub fn run(job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
    let p: TestStageParams = decode(job)?;
    let root = root_path(&p.root)?;
    let token = ctx.cancel_token();
    if let Some(code) = &p.job_error {
        return Err(JobError::new(
            code.clone(),
            "test.stage: the job fails as asked",
        ));
    }
    let mut read = 0u64;
    for item in &p.items {
        if let Some(ms) = p.delay_ms {
            crate::test_family::sleep_or_cancel(&token, ms)?;
        }
        if token.is_cancelled() {
            return Err(JobError::cancelled());
        }
        ctx.begin(item);
        if p.crash_on.contains(item) {
            std::process::abort();
        }
        if let Some(code) = p.fail_items.get(item) {
            ctx.item_failed(
                item,
                JobError::new(code.clone(), "test.stage: the item fails as asked"),
            );
            continue;
        }
        let Some(f) = p.files.iter().find(|f| &f.fp == item) else {
            ctx.item_failed(
                item,
                params_invalid("test.stage: no file for this fingerprint"),
            );
            continue;
        };
        match open_for_read(&root, &f.rel, &f.expect, p.hydrate) {
            Ok(file) => {
                let mut buf = Vec::new();
                match file
                    .take(p.read_bytes.unwrap_or(4096))
                    .read_to_end(&mut buf)
                {
                    Ok(n) => {
                        read += n as u64;
                        ctx.item(item, Some(json!({ "fileId": f.file_id, "bytes": n })));
                    }
                    Err(e) => {
                        ctx.item_failed(item, JobError::new("ASSET_UNREADABLE", e.to_string()))
                    }
                }
            }
            Err(e) => ctx.item_failed(item, JobError::new(refusal(&e), e.to_string())),
        }
    }
    Ok(json!({ "read": read }))
}
