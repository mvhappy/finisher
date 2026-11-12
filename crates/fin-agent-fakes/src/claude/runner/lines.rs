//! The lines the fake builds itself: the `initialize` response, `system/init`, ids.

use serde_json::{Map, Value, json};

use super::{DEFAULT_TOOLS, Runner};

pub(super) fn fresh_uuid() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let h = format!("{:032x}", nanos ^ (u128::from(std::process::id()) << 90));
    format!(
        "{}-{}-4{}-a{}-{}",
        &h[0..8],
        &h[8..12],
        &h[13..16],
        &h[17..20],
        &h[20..32]
    )
}

impl Runner {
    pub(super) fn initialize_body(&self) -> Value {
        let mut body = json!({
            "commands": [],
            "agents": [],
            "output_style": "default",
            "available_output_styles": ["default"],
            "models": [
                {"value": "default", "resolvedModel": "claude-fake-1", "displayName": "Default (fake)", "description": "Fake default model"},
                {"value": "opus", "resolvedModel": "claude-fake-opus-1", "displayName": "Opus (fake)", "description": "Fake large model"},
            ],
            "account": {"tokenSource": "claude.ai", "apiProvider": "firstParty"},
            "claude_code_version": self.version,
            "capabilities": ["interrupt_receipt_v1"],
            "current_permission_mode": self.mode,
            "pid": std::process::id(),
        });
        merge(&mut body, &self.script.initialize);
        body
    }

    pub(super) fn init_line(&self) -> Value {
        let tools: Vec<String> = match self.args.str("--tools") {
            None | Some("default") => DEFAULT_TOOLS.iter().map(|s| (*s).to_string()).collect(),
            Some(list) => list
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        };
        let mut line = json!({
            "type": "system", "subtype": "init",
            "cwd": std::env::current_dir().map(|c| c.to_string_lossy().into_owned()).unwrap_or_default(),
            "session_id": self.session_id,
            "tools": tools,
            "mcp_servers": [],
            "model": self.script.model.clone().unwrap_or_else(|| "claude-fake-1".into()),
            "permissionMode": self.mode,
            "apiKeySource": "none",
            "claude_code_version": self.version,
            "slash_commands": [],
            "skills": [],
            "plugins": [],
            "agents": [],
            "memory_paths": null,
            "capabilities": ["interrupt_receipt_v1"],
            "output_style": "default",
        });
        merge(&mut line, &self.script.init);
        line
    }
}

pub(super) fn merge(target: &mut Value, over: &Map<String, Value>) {
    if let Value::Object(t) = target {
        for (k, v) in over {
            t.insert(k.clone(), v.clone());
        }
    }
}
