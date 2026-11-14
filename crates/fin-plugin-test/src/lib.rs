//! The plugin test harness (sub-project 8 spec §7.4; plan 8B Task 14).
//!
//! A fake host ([`fake_host`]) that starts a plugin as the real host does (its runtime's
//! program, a trailing `--warm` for a warm process, the environment allow-list, a private temp
//! directory as its working directory) and speaks the host side of the `fin-jobs` envelope:
//! `hello`, `start` with a [`PluginContext`](fin_plugin_api::protocol::PluginContext) in
//! `extra`, `call` → `reply`, `cancel`, `ready`, `shutdown`. It serves `project.snapshot` and
//! `project.view` from a fixture snapshot ([`fixtures`]), `net.fetch`/`net.read` from a fake
//! HTTPS API on loopback with a certificate made for the run ([`fake_https`]): the fake host is
//! the broker, so the plugin never connects itself. It records every call.
//!
//! A conformance profile ([`profiles`]) runs a plugin through it and reports findings: the
//! `command` profile in 8B, the `hook` profile in 7F; `analyzer`, `provider`, `codec` and
//! `matte` arrive with their batches (8C, 8G, 8H). `fin-plugin test` is its command line.

pub mod calls;
pub mod fake_host;
pub mod fake_https;
pub mod fixtures;
pub mod plugin;
pub mod process;
pub mod profiles;
pub mod report;
pub mod scan;

use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Map, Value};

pub use report::{Finding, HookReport, Report};

/// A profile `fin-plugin test --profile` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Commands (spec §3.8): every declared command answers in time, with valid output.
    Command,
    /// Hook handlers (7F Task 9): every declared handler answers a sample request of its event
    /// in time with one valid reply (`allow`, `block`, `context`).
    Hook,
}

impl Profile {
    pub fn name(self) -> &'static str {
        match self {
            Profile::Command => "command",
            Profile::Hook => "hook",
        }
    }

    /// `command` or `hook`; a later batch's profile is refused with the batch it arrives in.
    pub fn parse(name: &str) -> Result<Profile, HarnessError> {
        let later = match name {
            "command" => return Ok(Profile::Command),
            "hook" => return Ok(Profile::Hook),
            "analyzer" => "8C",
            "provider" => "8G",
            "codec" | "matte" => "8H",
            _ => {
                return Err(HarnessError::new(
                    "USAGE",
                    format!("unknown profile {name:?}; the profiles are: command, hook"),
                ));
            }
        };
        Err(HarnessError::new(
            "USAGE",
            format!(
                "the {name} profile arrives with sub-project {later}; this harness has: command, hook"
            ),
        )
        .with("profile", name)
        .with("availableFrom", later))
    }
}

/// What to test and how.
#[derive(Debug, Clone)]
pub struct Options {
    /// The plugin's folder (its `finisher-plugin.json`).
    pub dir: PathBuf,
    pub profile: Profile,
    /// A snapshot (a fin-core project as JSON) in place of the built-in fixture.
    pub fixture: Option<PathBuf>,
    /// A native plugin's executable for this host, in place of the manifest's (a source tree
    /// has no build outputs under `bin/`).
    pub exec: Option<PathBuf>,
    /// The Node that runs a `node` plugin (`node` on `PATH` when absent).
    pub node: Option<PathBuf>,
}

impl Options {
    pub fn new(dir: impl Into<PathBuf>) -> Options {
        Options {
            dir: dir.into(),
            profile: Profile::Command,
            fixture: None,
            exec: None,
            node: None,
        }
    }
}

/// Why a run could not happen at all (as opposed to a [`Finding`]): `{code, message, details,
/// fix?}` with the codes `fin-plugin` uses.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HarnessError {
    pub code: String,
    pub message: String,
    pub details: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl HarnessError {
    pub fn new(code: &str, message: impl Into<String>) -> HarnessError {
        HarnessError {
            code: code.into(),
            message: message.into(),
            details: Map::new(),
            fix: None,
        }
    }

    pub fn with(mut self, key: &str, value: impl Into<Value>) -> HarnessError {
        self.details.insert(key.into(), value.into());
        self
    }

    pub fn fix(mut self, fix: &str) -> HarnessError {
        self.fix = Some(fix.into());
        self
    }
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for HarnessError {}

/// Loads the plugin and the fixture and runs the profile. `Err` only when nothing could be
/// tested (the manifest, the runtime, the fixture); a plugin that fails the profile is an
/// `Ok` report with `ok: false` and its findings.
pub fn run(opts: &Options) -> Result<Report, HarnessError> {
    let plugin = plugin::load(&opts.dir, opts.exec.as_deref(), opts.node.as_deref())?;
    let snapshot = match &opts.fixture {
        Some(path) => fixtures::Snapshot::load(path)?,
        None => fixtures::Snapshot::builtin(),
    };
    match opts.profile {
        Profile::Command => profiles::command::run(&plugin, &snapshot),
        Profile::Hook => profiles::hook::run(&plugin, &snapshot),
    }
}
