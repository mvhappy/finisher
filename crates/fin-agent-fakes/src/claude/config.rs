//! Where the fake's configuration comes from: `FAKE_CLAUDE_*` variables, else the sidecar
//! `<directory of the executable>/fake-claude.json` (`{script, observed, version, account,
//! invocations}`), else the script itself (`observed`, `version`). `account` is the label a signed-in CLI reports in
//! `initialize.account.email` (never in `auth status`, which names the account only from its
//! config dir's profile on 2.1.321), so a test can switch accounts without a new script.

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const SIDECAR: &str = "fake-claude.json";

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Sidecar {
    script: Option<String>,
    observed: Option<String>,
    version: Option<String>,
    account: Option<String>,
    invocations: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Config {
    pub script: Option<String>,
    pub observed: Option<String>,
    pub version: Option<String>,
    pub account: Option<String>,
    /// Where each process appends its invocation line (`invocations.rs`): `@store/<name>`,
    /// `@config/<name>` or a path (`FAKE_CLAUDE_INVOCATIONS`: one).
    pub invocations: Vec<String>,
}

/// The sidecar next to the running executable, merged under the environment.
pub fn load() -> Result<Config, String> {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let side = match dir.as_ref().map(|d| d.join(SIDECAR)) {
        Some(p) if p.is_file() => {
            let text = std::fs::read_to_string(&p)
                .map_err(|e| format!("fake-claude: cannot read {SIDECAR}: {e}"))?;
            serde_json::from_str::<Sidecar>(&text)
                .map_err(|e| format!("fake-claude: bad {SIDECAR}: {e}"))?
        }
        _ => Sidecar::default(),
    };
    // A relative script path in the sidecar is relative to the sidecar's directory.
    let side_script = side.script.map(|s| match &dir {
        Some(d) if Path::new(&s).is_relative() => d.join(s).to_string_lossy().into_owned(),
        _ => s,
    });
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    Ok(Config {
        script: env("FAKE_CLAUDE_SCRIPT").or(side_script),
        observed: env("FAKE_CLAUDE_OBSERVED").or(side.observed),
        version: env("FAKE_CLAUDE_VERSION").or(side.version),
        account: env("FAKE_CLAUDE_ACCOUNT").or(side.account),
        invocations: env("FAKE_CLAUDE_INVOCATIONS").map_or(side.invocations, |v| vec![v]),
    })
}

/// `@config/<name>` is `<$CLAUDE_CONFIG_DIR>/<name>`, `@store/<name>` is
/// `<$CLAUDE_SECURESTORAGE_CONFIG_DIR>/<name>` (the sign-in store: writable in detection's and
/// every session's sandbox), None when the variable is unset; anything else is a path as given.
pub fn observed_path(spec: &str) -> Option<PathBuf> {
    let under = |var: &str, name: &str| std::env::var_os(var).map(|d| PathBuf::from(d).join(name));
    if let Some(name) = spec.strip_prefix("@config/") {
        under("CLAUDE_CONFIG_DIR", name)
    } else if let Some(name) = spec.strip_prefix("@store/") {
        under("CLAUDE_SECURESTORAGE_CONFIG_DIR", name)
    } else {
        Some(PathBuf::from(spec))
    }
}

/// Appends this process's invocation (`{pid, argv}`, one JSON line) to each of `specs` it can
/// write (`observed_path`); a spec it cannot resolve or write is skipped. The host's own runs
/// (`--version`, `auth status`) are logged too, so a test can count every CLI process.
pub fn log_invocation(specs: &[String], argv: &[String]) {
    use std::io::Write;
    let line = serde_json::json!({"pid": std::process::id(), "argv": argv}).to_string();
    for path in specs.iter().filter_map(|s| observed_path(s)) {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}
