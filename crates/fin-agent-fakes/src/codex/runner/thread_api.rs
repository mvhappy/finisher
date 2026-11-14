//! `thread/start`, `thread/resume`, `thread/read`, `thread/revert`, `turn/start`, `turn/steer`
//! and `turn/interrupt`, with 0.167.0's checks and error texts (`codex::params`).

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Fake, OnInterrupt, Reply, RpcError, Steer, Task, rpc};
use crate::codex::{params, threads};

fn not_found(id: &str) -> RpcError {
    rpc(format!("thread not found: {id}"))
}

/// The script's `revertFails`. UNOBSERVED(7E Task 0 Step 3): how 0.167.0 reports a revert that
/// fails (message and `data.codexErrorInfo: "threadRollbackFailed"`).
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

    /// A loaded thread's status: `active` while a turn plays, `systemError` after a failed one.
    pub(super) fn status_of(&self, id: &str) -> Value {
        if self.active.as_ref().is_some_and(|a| a.thread == id) {
            json!({"type": "active", "activeFlags": []})
        } else if self.failed.contains(id) {
            json!({"type": "systemError"})
        } else {
            json!({"type": "idle"})
        }
    }

    fn view(&self, rec: &Value, with_turns: bool) -> Value {
        let id = rec["id"].as_str().unwrap_or("");
        threads::view(
            &self.home,
            rec,
            &self.version,
            with_turns,
            self.status_of(id),
        )
    }

    /// The part `thread/start` and `thread/resume` answer alike.
    fn thread_response(&self, rec: &Value, with_turns: bool) -> Value {
        let cwd = rec["cwd"].as_str().unwrap_or("");
        let mut r = json!({"thread": self.view(rec, with_turns),
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

    /// The params' settings, else the effective config's (`approval_policy`, `sandbox_mode`,
    /// `approvals_reviewer`: a user `config.toml` decides them, as on 0.167.0), else defaults.
    fn apply_settings(&self, rec: &mut Value, p: &Value) {
        for (param, field, key, default) in [
            (
                "approvalPolicy",
                "approvalPolicy",
                "approval_policy",
                "on-request",
            ),
            (
                "approvalsReviewer",
                "approvalsReviewer",
                "approvals_reviewer",
                "user",
            ),
            ("sandbox", "sandboxMode", "sandbox_mode", "read-only"),
        ] {
            if !p[param].is_null() {
                rec[field] = p[param].clone();
            } else if rec[field].is_null() {
                rec[field] = json!(self.eff.str(key).unwrap_or_else(|| default.into()));
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

    /// `thread/started`, then each enabled MCP server's start, which fails: the fake runs no
    /// server. UNOBSERVED(7E Task 0 Step 3): `thread/started` after a `thread/resume`; a
    /// server that starts (0.167.0 failed `command = "ls"` this way).
    fn announce(&mut self, thread: &Value) {
        let id = thread["id"].clone();
        self.tasks
            .push(Task::Notify("thread/started", json!({"thread": thread})));
        for (name, s) in self.eff.mcp_servers() {
            if s["enabled"] == false {
                continue;
            }
            for (status, error) in [
                ("starting", Value::Null),
                (
                    "failed",
                    json!(format!(
                        "MCP client for `{name}` failed to start: {}",
                        super::lists::MCP_FAILED
                    )),
                ),
            ] {
                self.tasks.push(Task::Notify(
                    "mcpServer/startupStatus/updated",
                    json!({"threadId": id, "name": name, "status": status, "error": error, "failureReason": null}),
                ));
            }
        }
    }

    pub(super) fn thread_start(&mut self, p: &Value) -> Reply {
        if self.script.refuse_dynamic_tools && !p["dynamicTools"].is_null() {
            return Err(rpc("Invalid request: unknown field `dynamicTools`"));
        }
        params::thread_start(p, self.experimental).map_err(rpc)?;
        let tools = p["dynamicTools"].as_array().cloned().unwrap_or_default();
        let id = threads::new_id();
        let model = p["model"]
            .as_str()
            .map(str::to_string)
            .or_else(|| self.eff.str("model"))
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
        self.announce(&reply["thread"]);
        self.tools.insert(id.clone(), Self::tool_names(&rec));
        self.loaded.insert(id, rec);
        Ok(reply)
    }

    pub(super) fn thread_resume(&mut self, p: &Value) -> Reply {
        let id = params::thread_resume(p).map_err(rpc)?;
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
        self.tools.insert(id.clone(), restored);
        self.loaded.insert(id, rec.clone());
        let reply = self.thread_response(&rec, true);
        self.announce(&reply["thread"]);
        Ok(reply)
    }

    /// A loaded thread, else a stored one shown `notLoaded` (0.167.0 reads it without loading
    /// it), else `thread not loaded: X`.
    pub(super) fn thread_read(&mut self, p: &Value) -> Reply {
        let id = params::thread_read(p).map_err(rpc)?;
        let with_turns = p["includeTurns"] == true;
        if let Some(r) = self.loaded.get(&id) {
            return Ok(json!({"thread": self.view(r, with_turns)}));
        }
        let rec = threads::load(&self.home, &id)
            .ok_or_else(|| rpc(format!("thread not loaded: {id}")))?;
        Ok(
            json!({"thread": threads::view(&self.home, &rec, &self.version, with_turns, json!({"type": "notLoaded"}))}),
        )
    }

    /// A revert of a thread whose turn is playing interrupts that turn first and is answered
    /// after its `turn/completed` (as 0.167.0); `true` when it was put off.
    pub(super) fn revert_waits(&mut self, id: &Value, p: &Value) -> bool {
        let Ok((thread, before)) = params::thread_revert(p) else {
            return false;
        };
        let known = self.loaded.get(&thread).is_some_and(|r| {
            r["turns"]
                .as_array()
                .is_some_and(|t| t.iter().any(|t| t["id"] == before.as_str()))
        });
        let Some(a) = self.active.as_mut().filter(|a| a.thread == thread) else {
            return false;
        };
        if !known || self.script.revert_fails {
            return false;
        }
        a.interrupted = true;
        self.deferred.push((id.clone(), p.clone()));
        true
    }

    /// Answers a put-off revert (the turn has ended), with the notifications that go with it.
    pub(super) fn revert_now(&mut self, id: &Value, p: &Value) {
        let reply = self.thread_revert(p);
        self.answer(id, reply);
        for task in std::mem::take(&mut self.tasks) {
            if let Task::Notify(m, p) = task {
                self.notify(m, p);
            }
        }
    }

    pub(super) fn thread_revert(&mut self, p: &Value) -> Reply {
        let (id, before) = params::thread_revert(p).map_err(rpc)?;
        if !self.loaded.contains_key(&id) {
            return Err(not_found(&id));
        }
        if self.script.revert_fails {
            return Err(rollback_failed("thread rollback failed".into()));
        }
        // 0.167.0 unloads and reloads the thread around a revert, failed or not.
        for status in ["notLoaded", "idle"] {
            self.notify(
                "thread/status/changed",
                json!({"threadId": id, "status": {"type": status}}),
            );
        }
        let rec = self.loaded.get_mut(&id).unwrap();
        let turns = rec["turns"].as_array().cloned().unwrap_or_default();
        let Some(at) = turns.iter().position(|t| t["id"] == before.as_str()) else {
            return Err(rpc(format!("turn not found: {before}")));
        };
        rec["turns"] = Value::Array(turns[..at].to_vec());
        rec["updatedAt"] = json!(threads::now_ms() / 1000);
        let rec = rec.clone();
        threads::save(&self.home, &rec);
        self.tasks
            .push(Task::Notify("thread/reverted", json!({"threadId": id})));
        // "`turns` is always empty" (ThreadRevertResponse).
        Ok(
            json!({"thread": self.view(&rec, false), "turnsBackwardsCursor": null, "itemsBackwardsCursor": null}),
        )
    }

    fn user_item(input: &Value) -> Value {
        let now = threads::now_ms();
        json!({"type": "userMessage", "id": threads::new_id(), "clientId": null,
            "content": input, "startedAtMs": now, "completedAtMs": now})
    }

    /// Adds a user message to the playing turn (after the response, as 0.167.0).
    fn join_active(&mut self, thread: &str, input: &Value) -> String {
        let item = Self::user_item(input);
        let a = self.active.as_mut().expect("an active turn");
        a.steers_seen += 1;
        a.items.push(item.clone());
        let turn = a.turn.clone();
        let mut started = item.clone();
        started["completedAtMs"] = Value::Null;
        self.tasks.push(Task::Notify(
            "item/started",
            json!({"item": started, "threadId": thread, "turnId": turn, "startedAtMs": item["startedAtMs"]}),
        ));
        self.tasks.push(Task::Notify(
            "item/completed",
            json!({"item": item, "threadId": thread, "turnId": turn, "completedAtMs": item["completedAtMs"]}),
        ));
        turn
    }

    pub(super) fn turn_start(&mut self, p: &Value) -> Reply {
        let id = params::turn_start(p).map_err(rpc)?;
        if !self.loaded.contains_key(&id) {
            return Err(not_found(&id));
        }
        let open = |turn: &str| json!({"turn": threads::turn_view(turn, "inProgress", None, Value::Null, None, 0)});
        match &self.active {
            // 0.167.0 takes a `turn/start` during a turn as more input to that turn.
            Some(a) if a.thread == id => {
                let turn = self.join_active(&id, &p["input"]);
                self.obs.push(
                    "steers",
                    json!({"turnId": turn, "input": p["input"], "joined": true, "via": "turn/start"}),
                );
                return Ok(open(&turn));
            }
            // UNOBSERVED(7E Task 0 Step 3): turns of two threads at once (the fake plays one).
            Some(_) => return Err(rpc("fake-codex plays one turn at a time")),
            None => {}
        }
        let turn = threads::new_id();
        self.tasks.push(Task::PlayTurn {
            thread: id,
            turn: turn.clone(),
            user: Self::user_item(&p["input"]),
        });
        Ok(open(&turn))
    }

    pub(super) fn turn_steer(&mut self, p: &Value) -> Reply {
        let (id, expected) = params::turn_steer(p).map_err(rpc)?;
        if !self.loaded.contains_key(&id) {
            return Err(not_found(&id));
        }
        let Some(a) = self.active.as_ref().filter(|a| a.thread == id) else {
            return Err(rpc("no active turn to steer"));
        };
        if a.turn != expected {
            return Err(rpc(format!(
                "expected active turn id `{expected}` but found `{}`",
                a.turn
            )));
        }
        let (turn, joined) = (a.turn.clone(), a.steer == Steer::Join);
        self.obs.push(
            "steers",
            json!({"turnId": turn, "input": p["input"], "joined": joined}),
        );
        if !joined {
            // UNOBSERVED(7E Task 0 Step 3): the refusal's message and data.
            return Err(RpcError {
                code: -32600,
                message: "the active turn cannot be steered".into(),
                data: Some(
                    json!({"codexErrorInfo": {"activeTurnNotSteerable": {"turnKind": "review"}}}),
                ),
            });
        }
        self.join_active(&id, &p["input"]);
        Ok(json!({"turnId": turn}))
    }

    pub(super) fn turn_interrupt(&mut self, p: &Value) -> Reply {
        let (id, turn) = params::turn_interrupt(p).map_err(rpc)?;
        if !self.loaded.contains_key(&id) {
            return Err(not_found(&id));
        }
        let Some(a) = self.active.as_mut().filter(|a| a.thread == id) else {
            return Err(rpc("no active turn to interrupt"));
        };
        if a.turn != turn {
            return Err(rpc(format!(
                "expected active turn id {turn} but found {}",
                a.turn
            )));
        }
        a.interrupt_seen = true;
        if a.on_interrupt != OnInterrupt::Ignore {
            a.interrupted = true;
        }
        Ok(json!({}))
    }
}
