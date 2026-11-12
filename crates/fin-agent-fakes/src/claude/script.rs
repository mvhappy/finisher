//! The script file: what the fake plays after its handshake.

use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Script {
    pub version: Option<String>,
    pub model: Option<String>,
    pub auth: Option<Value>,
    pub initialize: Map<String, Value>,
    pub init: Map<String, Value>,
    pub observed: Option<String>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Expect {
    User,
    Control,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Step {
    pub exit_on: Option<String>,
    pub expect: Option<Expect>,
    pub subtype: Option<String>,
    pub sleep: Option<u64>,
    pub write_file: Option<WriteFile>,
    pub spawn: Option<Spawn>,
    pub emit: Vec<Value>,
    pub unknown_line: bool,
    pub ask: Option<Ask>,
    pub on_allow: Vec<Value>,
    pub on_deny: Vec<Value>,
    pub not_logged_in: bool,
    pub cumulative_usage: Option<Usage>,
    pub text: Option<String>,
    pub crash: Option<Crash>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteFile {
    pub path: String,
    #[serde(default)]
    pub contents: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spawn {
    pub argv: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ask {
    pub tool: String,
    #[serde(default)]
    pub input: Value,
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crash {
    pub signal: Option<String>,
    pub code: Option<i32>,
}

pub fn load(path: &str) -> Result<Script, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("fake-claude: cannot read script {path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("fake-claude: bad script {path}: {e}"))
}
