//! What the fake reads from `--settings` and `--mcp-config` (a file path or inline JSON).
//!
//! Settings follow the CLI's `-p` rule (Task 0): a file in which a known key has the wrong
//! type is ignored as a whole; an unknown key is tolerated; an unknown `crossSessionInbound`
//! value drops that key alone.

use serde_json::{Map, Value};

/// Inline JSON (`{...}`) or the contents of the file the flag names.
fn source(value: &str) -> Option<Value> {
    let text = if value.trim_start().starts_with('{') {
        value.to_string()
    } else {
        std::fs::read_to_string(value).ok()?
    };
    serde_json::from_str(&text).ok()
}

#[derive(Debug, Default)]
pub struct Settings {
    /// `applied`, `ignored` (failed validation) or `absent`.
    pub state: &'static str,
    pub values: Map<String, Value>,
}

impl Settings {
    pub fn load(flag: Option<&str>) -> Settings {
        let Some(flag) = flag else {
            return Settings {
                state: "absent",
                values: Map::new(),
            };
        };
        match source(flag) {
            Some(Value::Object(mut values)) if valid(&values) => {
                if !values
                    .get("crossSessionInbound")
                    .and_then(Value::as_str)
                    .is_none_or(|v| ["accept", "hold", "refuse"].contains(&v))
                {
                    values.remove("crossSessionInbound");
                }
                Settings {
                    state: "applied",
                    values,
                }
            }
            _ => Settings {
                state: "ignored",
                values: Map::new(),
            },
        }
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.values.get(key).and_then(Value::as_str)
    }
}

fn is_strs(v: &Value) -> bool {
    v.as_array().is_some_and(|a| a.iter().all(Value::is_string))
}

/// The known keys the fake checks, with their types.
fn valid(s: &Map<String, Value>) -> bool {
    s.iter().all(|(k, v)| match k.as_str() {
        "disableAllHooks" | "autoMemoryEnabled" => v.is_boolean(),
        "cleanupPeriodDays" => v.is_number(),
        "outputStyle" | "model" | "crossSessionInbound" => v.is_string(),
        "env" => v
            .as_object()
            .is_some_and(|o| o.values().all(Value::is_string)),
        "attribution" => v
            .as_object()
            .is_some_and(|o| o.values().all(Value::is_string)),
        "permissions" => v.as_object().is_some_and(|p| {
            p.iter().all(|(k, v)| match k.as_str() {
                "allow" | "ask" | "deny" | "additionalDirectories" => is_strs(v),
                "defaultMode" => v.is_string(),
                "blockWritesOutsideWorkingDirectories" | "disableBypassPermissionsMode" => {
                    v.is_boolean()
                }
                _ => true,
            })
        }),
        _ => true,
    })
}

/// One `--mcp-config` server: its name and whether the host serves it (`type: "sdk"`).
#[derive(Debug, Clone)]
pub struct McpServer {
    pub name: String,
    pub sdk: bool,
}

/// The servers of `--mcp-config` (`{"mcpServers": {name: {type, ...}}}`), in name order.
pub fn mcp_servers(flag: Option<&str>) -> Vec<McpServer> {
    let Some(Value::Object(cfg)) = flag.and_then(source) else {
        return Vec::new();
    };
    let Some(Value::Object(servers)) = cfg.get("mcpServers") else {
        return Vec::new();
    };
    servers
        .iter()
        .map(|(name, s)| McpServer {
            name: name.clone(),
            sdk: s["type"] == "sdk",
        })
        .collect()
}
