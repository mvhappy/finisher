//! `account/read`, `model/list` and `experimentalFeature/list`.

use serde_json::{Value, json};

use super::Fake;
use crate::codex::config;

const MODELS: &str = include_str!("../data/models.json");

/// Page `limit` entries from `cursor` (an index as a string); the next cursor when more remain.
fn page(all: Vec<Value>, p: &Value, default_limit: usize) -> (Vec<Value>, Value) {
    let start = p["cursor"]
        .as_str()
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(0)
        .min(all.len());
    let limit = p["limit"]
        .as_u64()
        .map_or(default_limit, |l| l.max(1) as usize);
    let end = (start + limit).min(all.len());
    let next = if end < all.len() {
        json!(end.to_string())
    } else {
        Value::Null
    };
    (all[start..end].to_vec(), next)
}

impl Fake {
    pub(super) fn models(&self) -> Vec<Value> {
        self.script
            .models
            .clone()
            .unwrap_or_else(|| serde_json::from_str(MODELS).expect("data/models.json"))
    }

    pub(super) fn default_model(&self) -> String {
        let m = self.models();
        m.iter()
            .find(|x| x["isDefault"] == true)
            .or(m.first())
            .and_then(|x| x["id"].as_str())
            .unwrap_or("gpt-6.1-sol")
            .to_string()
    }

    pub(super) fn model_list(&self, p: &Value) -> Value {
        let all: Vec<Value> = self
            .models()
            .into_iter()
            .filter(|m| p["includeHidden"] == true || m["hidden"] != true)
            .collect();
        let (data, next) = page(all, p, 100);
        json!({"data": data, "nextCursor": next})
    }

    pub(super) fn feature_list(&self, p: &Value) -> Value {
        let all: Vec<Value> = config::features()
            .iter()
            .map(|f| {
                let beta = f.stage == "beta";
                json!({"name": f.name, "stage": f.stage,
                    "displayName": if beta { json!(f.name) } else { Value::Null },
                    "description": if beta { json!("") } else { Value::Null },
                    "announcement": if beta { json!("") } else { Value::Null },
                    "enabled": self.eff.feature(f.name), "defaultEnabled": f.default})
            })
            .collect();
        let (data, next) = page(all, p, 100);
        json!({"data": data, "nextCursor": next})
    }

    /// The signed-in account: only with an `auth.json`; its fields from, in order, the
    /// `fake-account.json` beside it (what `accountChange` writes), an `account` object inside
    /// `auth.json`, the script's `account`, else a ChatGPT Plus person.
    pub(super) fn account(&self) -> Option<Value> {
        let auth = self.home.join("auth.json");
        if !auth.exists() {
            return None;
        }
        let read = |p: std::path::PathBuf| {
            std::fs::read_to_string(p)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        };
        let given = read(self.home.join("fake-account.json"))
            .or_else(|| read(auth).and_then(|a| a.get("account").cloned()))
            .or_else(|| self.script.account.clone())
            .unwrap_or(json!({}));
        Some(match given["type"].as_str() {
            Some("apiKey") => json!({"type": "apiKey"}),
            _ => json!({"type": "chatgpt",
                "email": given["email"].as_str().unwrap_or("person@example.com"),
                "planType": given["planType"].as_str().unwrap_or("plus")}),
        })
    }

    pub(super) fn account_read(&self) -> Value {
        match self.account() {
            Some(a) => json!({"account": a, "requiresOpenaiAuth": true}),
            None => json!({"account": null, "requiresOpenaiAuth": true, "workspaceRouting": null}),
        }
    }
}
