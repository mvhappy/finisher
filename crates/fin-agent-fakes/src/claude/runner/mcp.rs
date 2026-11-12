//! The `--mcp-config` servers the host serves (`type: "sdk"`), reached through `mcp_message`
//! control requests. As the real CLI does (Task 0): the MCP `initialize` goes out before the
//! host's `initialize` is answered; once it is answered, `notifications/initialized` and
//! `tools/list` follow; the first turn waits (up to the script's `mcpTimeoutMs`) for every
//! server to finish, and one that has not is `failed`, its tools absent.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Ev, Runner};
use crate::claude::settings::McpServer;

pub const DEFAULT_TIMEOUT_MS: u64 = 1000;
const PROTOCOL: &str = "2025-11-25";

#[derive(Debug, Clone, PartialEq)]
enum State {
    Idle,
    Initializing(String),
    Listing(String),
    Connected,
    Failed,
}

#[derive(Debug)]
pub(super) struct Server {
    name: String,
    sdk: bool,
    state: State,
    tools: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct Mcp {
    servers: Vec<Server>,
    started: bool,
    /// Requests whose answer only needs consuming (`notifications/initialized`).
    quiet: Vec<String>,
}

impl Mcp {
    pub fn new(servers: Vec<McpServer>) -> Mcp {
        Mcp {
            servers: servers
                .into_iter()
                .map(|s| Server {
                    name: s.name,
                    sdk: s.sdk,
                    state: State::Idle,
                    tools: Vec::new(),
                })
                .collect(),
            started: false,
            quiet: Vec::new(),
        }
    }

    fn pending(&self) -> bool {
        self.servers
            .iter()
            .any(|s| matches!(s.state, State::Initializing(_) | State::Listing(_)))
    }
}

/// The JSON-RPC `result` of an `mcp_message` answer, if it is a success.
fn rpc_result(v: &Value) -> Option<&Value> {
    let r = &v["response"];
    if r["subtype"] != "success" {
        return None;
    }
    let rpc = &r["response"]["mcp_response"];
    (rpc["error"].is_null() && !rpc["result"].is_null()).then_some(&rpc["result"])
}

impl Runner {
    /// Sends an `mcp_message` control request; returns its request id.
    pub(super) fn mcp_send(&mut self, server: &str, message: Value) -> String {
        let rid = self.next_rid();
        self.emit(&json!({"type": "control_request", "request_id": rid,
            "request": {"subtype": "mcp_message", "server_name": server, "message": message}}));
        rid
    }

    /// Starts every SDK server's handshake (once).
    pub(super) fn mcp_start(&mut self) {
        if self.mcp.started {
            return;
        }
        self.mcp.started = true;
        let version = self.version.clone();
        for i in 0..self.mcp.servers.len() {
            if !self.mcp.servers[i].sdk {
                continue;
            }
            let name = self.mcp.servers[i].name.clone();
            let rid = self.mcp_send(
                &name,
                json!({"method": "initialize", "params": {"protocolVersion": PROTOCOL, "capabilities": {},
                    "clientInfo": {"name": "claude-code", "title": "Claude Code", "version": version}},
                    "jsonrpc": "2.0", "id": 0}),
            );
            self.mcp.servers[i].state = State::Initializing(rid);
        }
    }

    /// Advances a handshake on its answer; true when `v` belonged to one.
    pub(super) fn mcp_answer(&mut self, v: &Value) -> bool {
        let rid = v["response"]["request_id"].as_str().unwrap_or_default();
        if let Some(i) = self.mcp.quiet.iter().position(|q| q == rid) {
            self.mcp.quiet.remove(i);
            return true;
        }
        let Some(i) = self.mcp.servers.iter().position(
            |s| matches!(&s.state, State::Initializing(r) | State::Listing(r) if r == rid),
        ) else {
            return false;
        };
        let name = self.mcp.servers[i].name.clone();
        let result = rpc_result(v).cloned();
        let state = self.mcp.servers[i].state.clone();
        self.mcp.servers[i].state = match (state, result) {
            (State::Initializing(_), Some(_)) => {
                let note = self.mcp_send(
                    &name,
                    json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
                );
                self.mcp.quiet.push(note);
                let list = self.mcp_send(
                    &name,
                    json!({"method": "tools/list", "jsonrpc": "2.0", "id": 1}),
                );
                State::Listing(list)
            }
            (State::Listing(_), Some(r)) => {
                self.mcp.servers[i].tools = r["tools"]
                    .as_array()
                    .map(|ts| {
                        ts.iter()
                            .filter_map(|t| t["name"].as_str())
                            .map(|t| format!("mcp__{name}__{t}"))
                            .collect()
                    })
                    .unwrap_or_default();
                State::Connected
            }
            _ => State::Failed,
        };
        true
    }

    /// Before the first `system/init`: waits for every started handshake, up to the timeout.
    pub(super) fn mcp_wait(&mut self) {
        self.mcp_start();
        let timeout =
            Duration::from_millis(self.script.mcp_timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
        let end = Instant::now() + timeout;
        while self.mcp.pending() && !self.eof {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match self.next(Some(left)) {
                Ev::User(u) => self.pending.push_back(u),
                Ev::Eof | Ev::Timeout => break,
                _ => {}
            }
        }
        for s in &mut self.mcp.servers {
            if s.state != State::Connected {
                s.state = State::Failed;
            }
        }
    }

    pub(super) fn mcp_servers_json(&self) -> Value {
        Value::Array(
            self.mcp
                .servers
                .iter()
                .map(|s| {
                    let status = if s.state == State::Connected {
                        "connected"
                    } else {
                        "failed"
                    };
                    if s.sdk {
                        json!({"name": s.name, "status": status, "source": "sdk"})
                    } else {
                        json!({"name": s.name, "status": status})
                    }
                })
                .collect(),
        )
    }

    pub(super) fn mcp_tools(&self) -> Vec<String> {
        self.mcp
            .servers
            .iter()
            .filter(|s| s.state == State::Connected)
            .flat_map(|s| s.tools.iter().cloned())
            .collect()
    }
}
