//! `asset.probe` of a catalogued library file (plan 5D Task 13, Decisions 20 and 47): the
//! glue over [`fin_library::probe_asset`], which opens the file once through `open_for_read`
//! with its catalogued raw path and `expect` and probes that handle. The RPC layer calls it
//! only for a path that found exactly one catalogued row; `probe(path)` is never used for a
//! library file. Resolves to `Probe` JSON; rejects `FILE_CLOUD_ONLY`, `FILE_CHANGED`,
//! `FILE_NOT_FOUND`, `PATH_NOT_ALLOWED` or the probe's own error.

// The napi export is registered only outside `cargo test` (`test = false` for this crate).
#![cfg_attr(test, allow(dead_code))]

use std::path::PathBuf;

use fin_core::CoreError;
use fin_library::{Expect, FileRef, RawRel, probe_asset};
use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::{encode, guarded, reject};

pub struct LibraryProbeFileTask {
    root: PathBuf,
    rel_json: String,
    expect_json: String,
    hydrate: bool,
    display: PathBuf,
}

fn bad(what: &str, e: &serde_json::Error) -> CoreError {
    CoreError::new(
        "INVALID_REQUEST",
        format!("the library file's {what} is not valid: {e}"),
    )
}

impl Task for LibraryProbeFileTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<String> {
        guarded(
            || {
                let rel: RawRel = serde_json::from_str(&self.rel_json)
                    .map_err(|e| reject(bad("raw path", &e)))?;
                let expect: Expect = serde_json::from_str(&self.expect_json)
                    .map_err(|e| reject(bad("expectation", &e)))?;
                let file = FileRef {
                    file: 0,
                    root: &self.root,
                    rel: &rel,
                    expect: &expect,
                    hydrate: self.hydrate,
                    display: &self.display,
                };
                encode(&probe_asset(&file).map_err(reject)?)
            },
            || {},
        )
    }

    fn resolve(&mut self, _env: Env, output: String) -> Result<String> {
        Ok(output)
    }
}

/// Probes the catalogued file at `rel` (a `RawRel` JSON) below `root`, opened with `expect`
/// (an `Expect` JSON) and `hydrate`; `display` names the file in errors only.
#[napi(catch_unwind)]
pub fn library_probe_file(
    root: String,
    rel_json: String,
    expect_json: String,
    hydrate: bool,
    display: String,
) -> AsyncTask<LibraryProbeFileTask> {
    AsyncTask::new(LibraryProbeFileTask {
        root: root.into(),
        rel_json,
        expect_json,
        hydrate,
        display: display.into(),
    })
}
