//! `account/read`, `model/list` and `experimentalFeature/list`.

use serde_json::{Value, json};

use super::{Fake, Reply, rpc};
use crate::codex::config;

const MODELS: &str = include_str!("../data/models.json");

/// Page `limit` entries from `cursor` (an index as a string); the next cursor when more remain.
/// A cursor that is not one is `invalid cursor: X`; past the end `cursor N exceeds total
/// <what> M` (both as 0.167.0's `model/list`).
fn page(
    all: Vec<Value>,
    p: &Value,
    default_limit: usize,
    what: &str,
) -> Result<(Vec<Value>, Value), String> {
    let start = match &p["cursor"] {
        Value::Null => 0,
        c => {
            let raw = c.as_str().map_or_else(|| c.to_string(), str::to_string);
            let n: usize = raw.parse().map_err(|_| format!("invalid cursor: {raw}"))?;
            if n > all.len() {
                return Err(format!("cursor {n} exceeds total {what} {}", all.len()));
            }
            n
        }
    };
    let limit = p["limit"]
        .as_u64()
        .map_or(default_limit, |l| l.max(1) as usize);
    let end = (start + limit).min(all.len());
    let next = if end < all.len() {
        json!(end.to_string())
    } else {
        Value::Null
    };
    Ok((all[start..end].to_vec(), next))
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

    pub(super) fn model_list(&self, p: &Value) -> Reply {
        let all: Vec<Value> = self
            .models()
            .into_iter()
            .filter(|m| p["includeHidden"] == true || m["hidden"] != true)
            .collect();
        let (data, next) = page(all, p, 100, "models").map_err(rpc)?;
        Ok(json!({"data": data, "nextCursor": next}))
    }

    /// UNOBSERVED(7E Task 0 Step 3): the cursor errors of `experimentalFeature/list`.
    pub(super) fn feature_list(&self, p: &Value) -> Reply {
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
        let (data, next) = page(all, p, 100, "features").map_err(rpc)?;
        Ok(json!({"data": data, "nextCursor": next}))
    }

    /// The signed-in account: only with an `auth.json` (or the script's `signedInElsewhere`); its fields from, in order, the
    /// `fake-account.json` beside it (what `accountChange` writes), an `account` object inside
    /// `auth.json`, the script's `account`, else a ChatGPT Plus person.
    pub(super) fn account(&self) -> Option<Value> {
        let auth = self.home.join("auth.json");
        if !auth.exists() && !self.script.signed_in_elsewhere {
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
            Some("amazonBedrock") => json!({"type": "amazonBedrock"}),
            Some("gcp") => json!({"type": "gcp"}),
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
