//! A real `finisherd` that can also open agent sessions for `fin`'s tests (plan 7A Task 8b):
//! `apps/finisherd/test/support/agent-session-host.ts`, driven through request files.

use std::cell::Cell;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Run, Server, fin_cmd, finish, ok};

pub struct AgentHost {
    pub server: Server,
    requests: PathBuf,
    next: Cell<u32>,
}

/// An agent session: its id, credential file and root branch.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub credential: PathBuf,
    pub root: Option<String>,
}

impl AgentHost {
    pub fn start(test: &str) -> Option<AgentHost> {
        let server = Server::start_script(
            test,
            "apps/finisherd/test/support/agent-session-host.ts",
            &["--requests", "{root}/agent-host"],
        );
        // `start_script` makes the folder's parent; the host polls the folder itself.
        let server = server?;
        let requests = server.root.join("agent-host");
        std::fs::create_dir_all(&requests).unwrap();
        Some(AgentHost {
            server,
            requests,
            next: Cell::new(0),
        })
    }

    /// Sends one request and waits for its answer (an `error` answer panics unless `allow_error`).
    pub fn request(&self, req: Value) -> Value {
        let n = self.next.get() + 1;
        self.next.set(n);
        let name = format!("{n:06}");
        let tmp = self.requests.join(format!("{name}.tmp"));
        std::fs::write(&tmp, req.to_string()).unwrap();
        std::fs::rename(&tmp, self.requests.join(format!("{name}.req.json"))).unwrap();
        let res = self.requests.join(format!("{name}.res.json"));
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(text) = std::fs::read_to_string(&res) {
                let v: Value = serde_json::from_str(&text).unwrap();
                assert!(v.get("error").is_none(), "{req}: {v}");
                return v;
            }
            assert!(
                Instant::now() < deadline,
                "no answer to {req}: {}",
                self.server.log()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn open(&self, project_id: &str, mode: &str) -> Session {
        let v = self.request(json!({ "op": "open", "projectId": project_id, "mode": mode }));
        Session {
            id: v["sessionId"].as_str().unwrap().to_string(),
            credential: PathBuf::from(v["credential"].as_str().unwrap()),
            root: v["rootBranch"].as_str().map(str::to_string),
        }
    }

    pub fn revoke(&self, s: &Session) {
        self.request(json!({ "op": "revoke", "sessionId": s.id }));
    }

    pub fn txs(&self, s: &Session) -> Vec<Value> {
        self.request(json!({ "op": "txs", "sessionId": s.id }))["txs"]
            .as_array()
            .unwrap()
            .clone()
    }

    /// `fin` as a person (the local key).
    pub fn fin(&self, cwd: &Path, args: &[&str]) -> Run {
        super::fin(&self.server.sd, cwd, args)
    }

    /// `fin` as the session: `FIN_CREDENTIAL` and `FIN_SERVER` set, plus `env`. Server data is
    /// still given (`FIN_SERVER_DATA`): under a credential it must be ignored.
    pub fn agent_env(&self, s: &Session, cwd: &Path, args: &[&str], env: &[(&str, &OsStr)]) -> Run {
        let mut cmd = fin_cmd(&self.server.sd, cwd, args);
        cmd.env("FIN_CREDENTIAL", &s.credential)
            .env("FIN_SERVER", &self.server.socket);
        for (k, v) in env {
            cmd.env(k, v);
        }
        finish(args, cmd.output().unwrap())
    }

    pub fn agent(&self, s: &Session, cwd: &Path, args: &[&str]) -> Run {
        self.agent_env(s, cwd, args, &[])
    }

    /// A project `film` (asset a1 = `clip.mp4`, 10 s) made by a person; its id.
    pub fn project(&self) -> (PathBuf, String) {
        let root = &self.server.root;
        let created = ok(self.fin(root, &["project", "create", "film", "--name", "Film"]));
        let dir = root.join("film");
        super::media(&dir.join("clip.mp4"), 10);
        ok(self.fin(&dir, &["asset", "add", "clip.mp4"]));
        let id = created["project"]["projectId"]
            .as_str()
            .unwrap()
            .to_string();
        (dir, id)
    }

    /// The session's workspace, created.
    pub fn workspace(&self, dir: &Path, s: &Session) -> PathBuf {
        let w = dir.join("workspace").join(&s.id);
        std::fs::create_dir_all(&w).unwrap();
        w
    }
}
