//! The lines the fake builds itself: the `initialize` response, `system/init`, ids, and the
//! defaults copied from the real CLI (`../defaults.json`).

use std::sync::OnceLock;

use serde_json::{Map, Value, json};

use super::Runner;
use crate::claude::transcript;

/// A script `init`/`initialize` key with this value removes the key.
pub const REMOVE: &str = "$remove";

pub(super) fn defaults() -> &'static Value {
    static D: OnceLock<Value> = OnceLock::new();
    D.get_or_init(|| {
        serde_json::from_str(include_str!("../defaults.json")).expect("defaults.json is JSON")
    })
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn fresh_uuid() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    // splitmix64 over the time, the pid and a counter: distinct within and across processes.
    let mix = |mut z: u64| {
        z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let seed = nanos ^ (u64::from(std::process::id()) << 40) ^ n;
    let h = format!(
        "{:016x}{:016x}",
        mix(seed),
        mix(seed ^ 0x5555_5555_5555_5555)
    );
    format!(
        "{}-{}-4{}-a{}-{}",
        &h[0..8],
        &h[8..12],
        &h[13..16],
        &h[17..20],
        &h[20..32]
    )
}

/// Shallow merge; a value of `"$remove"` deletes the key.
pub(super) fn merge(target: &mut Value, over: &Map<String, Value>) {
    if let Value::Object(t) = target {
        for (k, v) in over {
            if v.as_str() == Some(REMOVE) {
                t.remove(k);
            } else {
                t.insert(k.clone(), v.clone());
            }
        }
    }
}

/// `--disallowedTools` removes a bare name, or every name with the prefix before a trailing `*`.
fn denied(tool: &str, deny: &[String]) -> bool {
    deny.iter().any(|d| match d.strip_suffix('*') {
        Some(prefix) => tool.starts_with(prefix),
        None => d == tool,
    })
}

impl Runner {
    fn output_style(&self) -> String {
        self.settings
            .str("outputStyle")
            .unwrap_or("default")
            .to_string()
    }

    pub(super) fn initialize_body(&self) -> Value {
        let mut body = defaults()["initialize"].clone();
        let user_styles = std::env::var_os("CLAUDE_CONFIG_DIR").map(|d| {
            std::path::PathBuf::from(d)
                .join("output-styles")
                .to_string_lossy()
                .into_owned()
        });
        merge(
            &mut body,
            json!({
                "output_style": self.output_style(),
                "user_output_styles_dir": user_styles,
                "pid": std::process::id(),
                "claude_code_version": self.version,
                "current_permission_mode": self.mode,
            })
            .as_object()
            .expect("an object"),
        );
        merge(&mut body, &self.script.initialize);
        body
    }

    /// The model `--model` names, resolved through the `initialize` models (else as given).
    pub(super) fn model(&self) -> String {
        if let Some(m) = &self.script.model {
            return m.clone();
        }
        let want = self.args.str("--model").unwrap_or("default");
        self.initialize_body()["models"]
            .as_array()
            .and_then(|ms| ms.iter().find(|m| m["value"] == want))
            .and_then(|m| m["resolvedModel"].as_str())
            .unwrap_or(want)
            .to_string()
    }

    pub(super) fn init_tools(&self) -> Vec<String> {
        let d = defaults();
        let mut tools: Vec<String> = match self.args.str("--tools") {
            None | Some("default") => {
                let drops = if self.args.flags.contains_key("--restricted") {
                    strs(&d["restrictedDrops"])
                } else {
                    Vec::new()
                };
                strs(&d["defaultTools"])
                    .into_iter()
                    .filter(|t| !drops.contains(t))
                    .collect()
            }
            Some(list) => {
                let mut named: Vec<String> = list
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                // The CLI lists them in its own order: `Task` first, then by name.
                named.sort_by_key(|t| (t.as_str() != "Task", t.clone()));
                named.dedup();
                named
            }
        };
        tools.extend(self.mcp_tools());
        let deny: Vec<String> = self
            .args
            .str("--disallowedTools")
            .map(|v| {
                v.split([',', ' '])
                    .filter(|s| !s.is_empty() && !s.contains('('))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        tools.retain(|t| !denied(t, &deny));
        tools
    }

    pub(super) fn init_line(&self) -> Value {
        let mut line = defaults()["init"].clone();
        let cwd = std::env::current_dir()
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_default();
        merge(
            &mut line,
            json!({
                "type": "system", "subtype": "init",
                "cwd": cwd, "original_cwd": cwd,
                "session_id": self.session_id,
                "tools": self.init_tools(),
                "mcp_servers": self.mcp_servers_json(),
                "model": self.model(),
                "permissionMode": self.mode,
                "claude_code_version": self.version,
                "output_style": self.output_style(),
                "uuid": fresh_uuid(),
            })
            .as_object()
            .expect("an object"),
        );
        if let Value::Object(o) = &mut line {
            if let Some(m) = self.memory_paths() {
                o.insert("memory_paths".into(), m);
            }
            if let Some(sock) = self.socket_path() {
                o.insert("messaging_socket_path".into(), json!(sock));
            }
        }
        merge(&mut line, &self.script.init);
        line
    }

    /// `{auto: <project>/memory/}` only when the settings turn auto memory on and the
    /// environment does not turn it off (Task 0); absent otherwise.
    fn memory_paths(&self) -> Option<Value> {
        let on = self.settings.values.get("autoMemoryEnabled") == Some(&Value::Bool(true));
        let env_off = std::env::var("CLAUDE_CODE_DISABLE_AUTO_MEMORY").is_ok_and(|v| v == "1");
        if !on || env_off {
            return None;
        }
        let dir = transcript::project_dir()?.join("memory");
        Some(json!({"auto": format!("{}/", dir.to_string_lossy())}))
    }

    /// The inbox socket the CLI reports (not bound by the fake); none under `--restricted`.
    fn socket_path(&self) -> Option<String> {
        if self.args.flags.contains_key("--restricted") {
            return None;
        }
        #[cfg(unix)]
        {
            // SAFETY: getuid has no preconditions and cannot fail.
            let uid = unsafe { libc::getuid() };
            Some(format!("/tmp/cc-socks-{uid}/{}.sock", std::process::id()))
        }
        #[cfg(not(unix))]
        None
    }

    /// Takes every queued user line for one turn. Verified on 2.1.321 (7D Task 0, s1d): lines
    /// queued during one turn run as one next turn.
    pub(super) fn take_queued(&mut self) -> bool {
        !std::mem::take(&mut self.pending).is_empty()
    }

    pub(super) fn ui_invalidate(&self) -> Value {
        json!({"type": "system", "subtype": "ui_invalidate", "event": "ui.render",
            "uuid": fresh_uuid(), "session_id": self.session_id})
    }

    pub(super) fn status_requesting(&self) -> Value {
        json!({"type": "system", "subtype": "status", "status": "requesting",
            "session_id": self.session_id, "uuid": fresh_uuid()})
    }
}
