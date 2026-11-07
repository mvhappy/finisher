//! The test helper (`fin-workers --helper test:<behaviour>`): a codec helper with id `test`.
//! - `ok`: attaches and waits for `shutdown`;
//! - `exitEarly`: exits 3 right after `attach`;
//! - `hang`: stops `alive`, ignores stdin and blocks;
//! - `spawn`: double-forks a sleeper (as `test.spawn`) and writes the pids to its first
//!   writable file, then waits for `shutdown`;
//! - `call`: calls `test.echo {from: "helper"}`; an error reply fails it.

use std::fs::OpenOptions;
use std::io::Write;
use std::time::Duration;

use fin_jobs::helper::{Helper, HelperCtx};
use fin_jobs::test_kinds::{Detach, HelperBehaviour, SpawnPids};
use fin_jobs::{Attach, FileMode, HelperKind, JobError, Retry};
use serde_json::json;

use super::spawn_grandchild;

pub struct TestHelper {
    behaviour: HelperBehaviour,
}

impl TestHelper {
    /// The helper for a variant name, or `None`.
    pub fn parse(variant: &str) -> Option<TestHelper> {
        let behaviour = serde_json::from_value(json!(variant)).ok()?;
        Some(TestHelper { behaviour })
    }
}

fn io(e: std::io::Error) -> JobError {
    JobError::new("IO_ERROR", e.to_string()).with_retry(Retry::Backoff)
}

impl Helper for TestHelper {
    fn kind(&self) -> HelperKind {
        HelperKind::Codec
    }

    fn id(&self) -> &str {
        "test"
    }

    fn run(&mut self, attach: &Attach, ctx: &mut HelperCtx<'_>) -> Result<(), JobError> {
        match self.behaviour {
            HelperBehaviour::Ok => Ok(()),
            HelperBehaviour::ExitEarly => std::process::exit(3),
            HelperBehaviour::Hang => {
                ctx.stop_alive_for_test();
                ctx.ignore_eof_for_test();
                loop {
                    std::thread::sleep(Duration::from_secs(3600));
                }
            }
            HelperBehaviour::Spawn => {
                let (mid, grandchild) = spawn_grandchild(Detach::None, 600_000).map_err(io)?;
                let pids = SpawnPids {
                    worker: std::process::id(),
                    mid,
                    grandchild,
                };
                if let Some(f) = attach
                    .files
                    .iter()
                    .find(|f| f.mode != FileMode::Read && !f.name.ends_with('/'))
                {
                    let mut file = OpenOptions::new().write(true).open(&f.path).map_err(io)?;
                    let text = serde_json::to_vec(&pids)
                        .map_err(|e| JobError::new("INTERNAL", e.to_string()))?;
                    file.write_all(&text)
                        .and_then(|()| file.sync_all())
                        .map_err(io)?;
                }
                Ok(())
            }
            HelperBehaviour::Call => ctx
                .call("test.echo", json!({ "from": "helper" }))
                .map(|_| ()),
        }
    }
}
