//! `test <dir> [--profile command|hook] [--fixture <snapshot.json>] [--exec <path>] [--node <path>]`:
//! runs the plugin in `dir` through a conformance profile on `fin-plugin-test`'s fake host
//! (spec §7.4). A pass prints the report; a failed profile is `PLUGIN_TEST_FAILED` (exit 3)
//! with the report, its findings first, in `details`.

use std::path::PathBuf;

use fin_plugin_test::{HarnessError, Options, Profile};
use serde_json::{Value, json};

use crate::{Args, CliError};

impl From<HarnessError> for CliError {
    fn from(e: HarnessError) -> CliError {
        CliError {
            code: e.code,
            message: e.message,
            details: e.details,
            fix: e.fix,
        }
    }
}

pub fn run(raw: &[String]) -> Result<Value, CliError> {
    let a = Args::parse(raw, &["--profile", "--fixture", "--exec", "--node"], &[])?;
    let dir = &a.positionals(1, "test <dir>")?[0];
    let opts = Options {
        profile: Profile::parse(a.opt("--profile").unwrap_or("command"))?,
        fixture: a.opt("--fixture").map(PathBuf::from),
        exec: a.opt("--exec").map(PathBuf::from),
        node: a.opt("--node").map(PathBuf::from),
        ..Options::new(dir)
    };
    let report = fin_plugin_test::run(&opts)?;
    let value = serde_json::to_value(&report).unwrap_or(Value::Null);
    if report.ok {
        return Ok(value);
    }
    let first = &report.findings[0];
    let mut e = CliError::new(
        "PLUGIN_TEST_FAILED",
        format!(
            "{} failed the {} profile: {} finding(s); first: {}",
            report.plugin,
            report.profile,
            report.findings.len(),
            first.message
        ),
    )
    .fix("fix each finding in `details.findings`, then run `fin-plugin test` again");
    e.details.insert("findings".into(), json!(report.findings));
    e.details.insert("report".into(), value);
    Err(e)
}
