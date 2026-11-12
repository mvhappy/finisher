//! Session transcripts under `$CLAUDE_CONFIG_DIR/projects/<cwd>/<uuid>.jsonl`, and the
//! cumulative totals a resume restores.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct Totals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cost_usd: f64,
    pub turns: u64,
}

pub struct Transcript {
    path: Option<PathBuf>,
}

pub struct Restored {
    pub lines: usize,
    pub totals: Totals,
}

fn projects() -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")?;
    Some(PathBuf::from(dir).join("projects"))
}

impl Transcript {
    /// The transcript for a fresh session in the current directory.
    pub fn create(uuid: &str) -> Transcript {
        let path = projects().map(|p| {
            let cwd = std::env::current_dir()
                .map(|c| c.to_string_lossy().into_owned())
                .unwrap_or_default();
            let slug: String = cwd
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            p.join(slug).join(format!("{uuid}.jsonl"))
        });
        Transcript { path }
    }

    /// Finds `<uuid>.jsonl` in any project directory of the config dir.
    pub fn find(uuid: &str) -> Option<(Transcript, Restored)> {
        let name = format!("{uuid}.jsonl");
        for entry in fs::read_dir(projects()?).ok()?.flatten() {
            let path = entry.path().join(&name);
            if let Ok(text) = fs::read_to_string(&path) {
                let mut totals = Totals::default();
                for line in text.lines() {
                    if let Ok(v) = serde_json::from_str::<Value>(line)
                        && v["type"] == "fake_totals"
                        && let Ok(t) = serde_json::from_value(v["totals"].clone())
                    {
                        totals = t;
                    }
                }
                return Some((
                    Transcript { path: Some(path) },
                    Restored {
                        lines: text.lines().count(),
                        totals,
                    },
                ));
            }
        }
        None
    }

    pub fn append(&self, line: &Value) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{line}");
        }
    }

    pub fn append_totals(&self, totals: &Totals) {
        self.append(&json!({"type": "fake_totals", "totals": totals}));
    }
}
