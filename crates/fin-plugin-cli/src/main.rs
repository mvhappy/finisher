//! `fin-plugin`: publisher tooling (sub-project 8 spec §5.6, §6.1, §6.5). Scaffolds, validates
//! and packs plugins, and builds and signs local-folder registries with
//! `fin_registry::editor`. Success is one JSON document on stdout; a failure is one JSON object
//! `{code, message, details, fix?}` on stderr with the exit code of its code (spec §13).
//! Private keys are never printed.

mod cmd;
mod keys;

use std::collections::BTreeMap;
use std::process::ExitCode;

use serde_json::{Map, Value, json};

const HELP: &str = "fin-plugin: scaffold, validate, pack and publish Finisher plugins

USAGE:
  fin-plugin new content|code <dir>
  fin-plugin validate <dir>
  fin-plugin pack <dir> -o <file.finpkg>
  fin-plugin keygen <private.pem> [--public <public.pem>]
  fin-plugin registry <command> ...        (see `fin-plugin registry --help`)

Output is JSON on stdout. Errors are {code, message, details, fix?} on stderr with the exit
code: 2 usage, 3 invalid or not found, 4 conflict, 5 unavailable, 7 trust.
";

pub(crate) const REGISTRY_HELP: &str = "fin-plugin registry: build and sign a local-folder registry

Note: a git registry is a clone of a repository holding this folder; commit and push it, and users `git pull`.
Private keys live in --keys, a folder outside <dir>, and are never published.

USAGE:
  registry init <dir> --keys <keys-dir> --root-keys <k.pem>... --threshold <n> [--max-stale-days <d>]
  registry delegate <dir> --keys <kd> --publisher <name> --key <public-or-private.pem>
  registry publish <dir> <pkg.finpkg> --keys <kd> --key <publisher-key.pem>
        (a publisher's first publish also signs its delegation into `targets`: the targets
        key `registry init` made must be in --keys)
  registry yank <dir> <id>@<version> --keys <kd> --key <publisher-key.pem>
  registry advise <dir> --keys <kd> --plugin <id> --versions <range> --action warn|disable
        [--severity low|medium|high|critical] --summary <text> [--url <u>] [--id <id>] --key <k.pem>
  registry rotate-key <dir> --keys <kd> --role root|targets|snapshot|timestamp|<publisher>
        --new <k.pem>... [--threshold <n>]
  registry resign-timestamp <dir> --keys <kd> [--expires <days>]
  registry trust reset-clock --plugins <server-data>/plugins --id <registry-id>
        (on the server, by its owner: after fixing a clock that ran fast, clears the time
        of the last refresh that makes every refresh CLOCK_SUSPECT; nothing else changes)

`init` prints the SHA-256 of 1.root.json: users pin it when they add the registry.
";

/// A failure `{code, message, details, fix?}`.
#[derive(Debug)]
pub struct CliError {
    pub code: String,
    pub message: String,
    pub details: Map<String, Value>,
    pub fix: Option<String>,
}

impl CliError {
    pub fn new(code: &str, message: impl Into<String>) -> CliError {
        CliError {
            code: code.into(),
            message: message.into(),
            details: Map::new(),
            fix: None,
        }
    }

    pub fn usage(message: impl Into<String>) -> CliError {
        CliError::new("USAGE", message)
    }

    pub fn io(op: &str, path: &std::path::Path, e: impl std::fmt::Display) -> CliError {
        let mut err = CliError::new("IO_ERROR", format!("{op} {}: {e}", path.display()));
        err.details.insert("op".into(), op.into());
        err.details
            .insert("path".into(), path.display().to_string().into());
        err
    }

    pub fn fix(mut self, fix: &str) -> CliError {
        self.fix = Some(fix.into());
        self
    }

    /// The exit code of this error's code (a copy of `fin`'s table, spec §13).
    pub fn exit_code(&self) -> u8 {
        match self.code.as_str() {
            "USAGE" | "MANIFEST_TOO_LARGE" => 2,
            "PLUGIN_VERSION_EXISTS"
            | "REGISTRY_EXISTS"
            | "PUBLISHER_EXISTS"
            | "DIR_NOT_EMPTY"
            | "KEY_EXISTS" => 4,
            "IO_ERROR" | "REGISTRY_UNREACHABLE" | "CLOCK_SUSPECT" => 5,
            "REGISTRY_UNTRUSTED" => 7,
            c if c.starts_with("TUF_") => 7,
            _ => 3,
        }
    }

    fn to_json(&self) -> Value {
        let mut v = json!({
            "code": self.code,
            "message": self.message,
            "details": self.details,
        });
        if let Some(f) = &self.fix {
            v["fix"] = f.clone().into();
        }
        v
    }
}

impl From<fin_registry::editor::EditorError> for CliError {
    fn from(e: fin_registry::editor::EditorError) -> CliError {
        let fix = match e.code.as_str() {
            "KEY_MISSING" => Some("pass --keys the key folder `registry init` used"),
            "PLUGIN_VERSION_EXISTS" => {
                Some("versions are immutable: bump the version and pack again")
            }
            "PUBLISHER_NOT_DELEGATED" => Some("run `registry delegate` for this publisher first"),
            _ => None,
        };
        CliError {
            code: e.code,
            message: e.message,
            details: e.details,
            fix: fix.map(String::from),
        }
    }
}

impl From<fin_registry::package::PackageError> for CliError {
    fn from(e: fin_registry::package::PackageError) -> CliError {
        CliError {
            code: e.code.into(),
            message: e.message,
            details: e.details,
            fix: None,
        }
    }
}

/// Parsed arguments: positionals and `--flag value` / `--flag v1 v2 …` options.
pub struct Args {
    pos: Vec<String>,
    flags: BTreeMap<String, Vec<String>>,
}

impl Args {
    /// `single` flags take one value, `multi` flags one or more (up to the next flag).
    pub fn parse(raw: &[String], single: &[&str], multi: &[&str]) -> Result<Args, CliError> {
        let mut a = Args {
            pos: Vec::new(),
            flags: BTreeMap::new(),
        };
        let mut i = 0;
        while i < raw.len() {
            let t = raw[i].as_str();
            i += 1;
            if !t.starts_with('-') || t == "-" {
                a.pos.push(t.to_string());
                continue;
            }
            let is_multi = multi.contains(&t);
            if !is_multi && !single.contains(&t) {
                return Err(CliError::usage(format!("unknown option {t}")));
            }
            let mut vals = Vec::new();
            while i < raw.len() && !raw[i].starts_with("--") && (is_multi || vals.is_empty()) {
                vals.push(raw[i].clone());
                i += 1;
            }
            if vals.is_empty() {
                return Err(CliError::usage(format!("{t} needs a value")));
            }
            if a.flags.contains_key(t) && !is_multi {
                return Err(CliError::usage(format!("{t} given twice")));
            }
            a.flags.entry(t.to_string()).or_default().extend(vals);
        }
        Ok(a)
    }

    /// Exactly `n` positionals.
    pub fn positionals(&self, n: usize, what: &str) -> Result<&[String], CliError> {
        if self.pos.len() != n {
            return Err(CliError::usage(format!("expected {what}")));
        }
        Ok(&self.pos)
    }

    pub fn opt(&self, name: &str) -> Option<&str> {
        self.flags
            .get(name)
            .and_then(|v| v.first())
            .map(String::as_str)
    }

    pub fn req(&self, name: &str) -> Result<&str, CliError> {
        self.opt(name)
            .ok_or_else(|| CliError::usage(format!("{name} is required")))
    }

    pub fn all(&self, name: &str) -> Result<&[String], CliError> {
        match self.flags.get(name) {
            Some(v) => Ok(v),
            None => Err(CliError::usage(format!("{name} is required"))),
        }
    }

    pub fn number<T: std::str::FromStr>(&self, name: &str) -> Result<Option<T>, CliError> {
        self.opt(name)
            .map(|s| {
                s.parse::<T>()
                    .map_err(|_| CliError::usage(format!("{name} needs a number, got {s:?}")))
            })
            .transpose()
    }
}

fn run(args: &[String]) -> Result<Option<Value>, CliError> {
    let wants_help = args.iter().any(|a| a == "--help" || a == "-h");
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    match args.first().map(String::as_str) {
        _ if wants_help => {
            if args.first().map(String::as_str) == Some("registry") {
                print!("{REGISTRY_HELP}");
            } else {
                print!("{HELP}");
            }
            Ok(None)
        }
        Some("new") => cmd::new::run(&rest).map(Some),
        Some("validate") => cmd::validate::run(&rest).map(Some),
        Some("pack") => cmd::pack::run(&rest).map(Some),
        Some("keygen") => keys::keygen(&rest).map(Some),
        Some("registry") => cmd::registry::run(&rest).map(Some),
        Some(other) => Err(CliError::usage(format!(
            "unknown command {other:?}; see --help"
        ))),
        None => Err(CliError::usage("no command; see --help")),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(Some(v)) => {
            println!("{v}");
            ExitCode::SUCCESS
        }
        Ok(None) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", e.to_json());
            ExitCode::from(e.exit_code())
        }
    }
}
