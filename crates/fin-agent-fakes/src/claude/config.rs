//! Where the fake's configuration comes from: `FAKE_CLAUDE_*` variables, else the sidecar
//! `<directory of the executable>/fake-claude.json` (`{script, observed, version}`), else the
//! script itself (`observed`, `version`). `account` is the label a signed-in CLI reports in
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
}

#[derive(Debug, Default)]
pub struct Config {
    pub script: Option<String>,
    pub observed: Option<String>,
    pub version: Option<String>,
    pub account: Option<String>,
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
    })
}

/// `@config/<name>` is `<$CLAUDE_CONFIG_DIR>/<name>` (None when that variable is unset);
/// anything else is a path as given.
pub fn observed_path(spec: &str) -> Option<PathBuf> {
    match spec.strip_prefix("@config/") {
        Some(name) => std::env::var_os("CLAUDE_CONFIG_DIR").map(|d| PathBuf::from(d).join(name)),
        None => Some(PathBuf::from(spec)),
    }
}
