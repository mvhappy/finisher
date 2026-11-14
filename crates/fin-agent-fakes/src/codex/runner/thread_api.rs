//! `thread/start`, `thread/resume`, `thread/read`, `thread/revert`, `turn/start`, `turn/steer`
//! and `turn/interrupt`.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Fake, OnInterrupt, Reply, RpcError, Steer, Task, rpc};
use crate::codex::threads;

fn not_found(id: &str) -> RpcError {
    rpc(format!("thread not found: {id}"))
}

fn rollback_failed(message: String) -> RpcError {
    RpcError {
        code: -32600,
        message,
        data: Some(json!({"codexErrorInfo": "threadRollbackFailed"})),
    }
}

impl Fake {
    fn cwd_param(p: &Value) -> String {
        p["cwd"].as_str().map(str::to_string).unwrap_or_else(|| {
            std::env::current_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
    }

    /// The part `thread/start` and `thread/resume` answer alike.
    fn thread_response(&self, rec: &Value, with_turns: bool) -> Value {
        let cwd = rec["cwd"].as_str().unwrap_or("");
        let mut r = json!({"thread": threads::view(&self.home, rec, &self.version, with_turns),
            "model": rec["model"], "modelProvider": "openai", "serviceTier": null,
            "cwd": cwd, "instructionSources": self.instruction_sources(&PathBuf::from(cwd)),
            "approvalPolicy": rec["approvalPolicy"], "approvalsReviewer": rec["approvalsReviewer"],
            "sandbox": threads::sandbox_policy(&rec["sandboxMode"]), "reasoningEffort": null});
        if !with_turns {
            r["disabledPluginIds"] = json!([]);
            r["runtimeWorkspaceRoots"] = json!([cwd]);
            r["activePermissionProfile"] = Value::Null;
            r["multiAgentMode"] = json!("explicitRequestOnly");
        }
        r
    }

    fn apply_settings(&self, rec: &mut Value, p: &Value) {
        for (param, field, default) in [
            ("approvalPolicy", "approvalPolicy", json!("on-request")),
            ("approvalsReviewer", "approvalsReviewer", json!("user")),
            ("sandbox", "sandboxMode", json!("read-only")),
        ] {
            if !p[param].is_null() {
                rec[field] = p[param].clone();
            } else if rec[field].is_null() {
                rec[field] = default;
            }
        }
        if let Some(m) = p["model"].as_str() {
            rec["model"] = json!(m);
        }
        if p["cwd"].is_string() {
            rec["cwd"] = p["cwd"].clone();
        }
    }

    fn tool_names(rec: &Value) -> Vec<String> {
        rec["dynamicTools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect()
    }

    pub(super) fn thread_start(&mut self, p: &Value) -> Reply {
        let tools = p["dynamicTools"].as_array().cloned().unwrap_or_default();
        if !tools.is_empty() && !self.experimental {
            return Err(rpc(
                "thread/start.dynamicTools requires experimentalApi capability",
            ));
        }
        let id = threads::new_id();
        let model = p["model"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| self.default_model());
        let mut rec = threads::create(
            &id,
            &Self::cwd_param(p),
            &model,
            p["ephemeral"] == true,
            tools,
        );
        self.apply_settings(&mut rec, p);
        threads::save(&self.home, &rec);
        let reply = self.thread_response(&rec, false);
        self.tasks.push(Task::Notify(
            "thread/started",
            json!({"thread": reply["thread"]}),
        ));
        self.tools.insert(id.clone(), Self::tool_names(&rec));
        self.loaded.insert(id, rec);
        Ok(reply)
    }

    pub(super) fn thread_resume(&mut self, p: &Value) -> Reply {
        let id = p["threadId"]
            .as_str()
            .ok_or_else(|| rpc("Invalid request: missing field `threadId`"))?
            .to_string();
        let mut rec = match self.loaded.get(&id) {
            Some(r) => r.clone(),
            None => threads::load(&self.home, &id)
                .ok_or_else(|| rpc(format!("no rollout found for thread id {id}")))?,
        };
        self.apply_settings(&mut rec, p);
        let restored = if self.script.resume_restores_dynamic_tools == Some(false) {
            vec![]
        } else {
            Self::tool_names(&rec)
        };
        let reply = self.thread_response(&rec, true);
        self.tasks.push(Task::Notify(
            "thread/started",
            json!({"thread": reply["thread"]}),
        ));
        self.tools.insert(id.clone(), restored);
        self.loaded.insert(id, rec);
        Ok(reply)
    }

    pub(super) fn thread_read(&mut self, p: &Value) -> Reply {
        let id = p["threadId"]
            .as_str()
            .ok_or_else(|| rpc("Invalid request: missing field `threadId`"))?;
        let rec = match self.loaded.get(id) {
            Some(r) => r.clone(),
            None => threads::load(&self.home, id).ok_or_else(|| not_found(id))?,
        };
        Ok(
            json!({"thread": threads::view(&self.home, &rec, &self.version, p["includeTurns"] == true)}),
        )
    }

    pub(super) fn thread_revert(&mut self, p: &Value) -> Reply {
        let id = p["threadId"]
            .as_str()
            .ok_or_else(|| rpc("Invalid request: missing field `threadId`"))?
            .to_string();
        let before = p["beforeTurnId"]
            .as_str()
            .ok_or_else(|| rpc("Invalid request: missing field `beforeTurnId`"))?;
        if !self.loaded.contains_key(&id) {
            return Err(not_found(&id));
        }
        if self.script.revert_fails {
            return Err(rollback_failed("thread rollback failed".into()));
        }
        if self.active.as_ref().is_some_and(|a| a.thread == id) {
            return Err(rollback_failed(
                "cannot revert a thread with an active turn".into(),
            ));
        }
        let rec = self.loaded.get_mut(&id).unwrap();
        let turns = rec["turns"].as_array().cloned().unwrap_or_default();
        let Some(at) = turns.iter().position(|t| t["id"] == before) else {
            return Err(rollback_failed(format!("turn not found: {before}")));
        };
        rec["turns"] = Value::Array(turns[..at].to_vec());
        rec["updatedAt"] = json!(threads::now_ms() / 1000);
        let rec = rec.clone();
        threads::save(&self.home, &rec);
        self.tasks
            .push(Task::Notify("thread/reverted", json!({"threadId": id})));
        Ok(json!({"thread": threads::view(&self.home, &rec, &self.version, true)}))
    }

    pub(super) fn turn_start(&mut self, p: &Value) -> Reply {
        let id = p["threadId"]
            .as_str()
            .ok_or_else(|| rpc("Invalid request: missing field `threadId`"))?
            .to_string();
        if !self.loaded.contains_key(&id) {
            return Err(not_found(&id));
        }
        if self.active.is_some() {
            return Err(rpc("thread already has an active turn"));
        }
        let turn = threads::new_id();
        let now = threads::now_ms();
        let user = json!({"type": "userMessage", "id": threads::new_id(), "clientId": null,
            "content": p["input"].clone(), "startedAtMs": now, "completedAtMs": now});
        self.tasks.push(Task::PlayTurn {
            thread: id,
            turn: turn.clone(),
            user,
        });
        Ok(
            json!({"turn": threads::turn_view(&turn, "inProgress", &[], Value::Null, now / 1000, 0)}),
        )
    }

    pub(super) fn turn_steer(&mut self, p: &Value) -> Reply {
        let id = p["threadId"].as_str().unwrap_or("");
        let Some(a) = self.active.as_mut().filter(|a| a.thread == id) else {
            return Err(rpc("no active turn to steer"));
        };
        let expected = p["expectedTurnId"].as_str().unwrap_or("");
        if a.turn != expected {
            return Err(rpc(format!(
                "expected active turn id `{expected}` but found `{}`",
                a.turn
            )));
        }
        let (turn, steer) = (a.turn.clone(), a.steer);
        let joined = steer == Steer::Join;
        self.obs.push(
            "steers",
            json!({"turnId": turn, "input": p["input"], "joined": joined}),
        );
        if !joined {
            return Err(RpcError {
                code: -32600,
                message: "the active turn cannot be steered".into(),
                data: Some(
                    json!({"codexErrorInfo": {"activeTurnNotSteerable": {"turnKind": "review"}}}),
                ),
            });
        }
        let now = threads::now_ms();
        let item = json!({"type": "userMessage", "id": threads::new_id(), "clientId": null,
            "content": p["input"].clone(), "startedAtMs": now, "completedAtMs": now});
        let a = self.active.as_mut().unwrap();
        a.steers_seen += 1;
        a.items.push(item.clone());
        let mut started = item.clone();
        started["completedAtMs"] = Value::Null;
        self.tasks.push(Task::Notify(
            "item/started",
            json!({"item": started, "threadId": id, "turnId": turn, "startedAtMs": now}),
        ));
        self.tasks.push(Task::Notify(
            "item/completed",
            json!({"item": item, "threadId": id, "turnId": turn, "completedAtMs": now}),
        ));
        Ok(json!({"turnId": turn}))
    }

    pub(super) fn turn_interrupt(&mut self, p: &Value) -> Value {
        if let Some(a) = self.active.as_mut()
            && a.thread == p["threadId"].as_str().unwrap_or("")
            && a.turn == p["turnId"].as_str().unwrap_or("")
        {
            a.interrupt_seen = true;
            if a.on_interrupt != OnInterrupt::Ignore {
                a.interrupted = true;
            }
        }
        json!({})
    }
}
