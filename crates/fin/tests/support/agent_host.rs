//! A real `finisherd` that can also open agent sessions for `fin`'s tests (plan 7A Task 8b):
//! `apps/finisherd/test/support/agent-session-host.ts`, driven through request files.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Run, Server, fin_cmd, finish, ok};

pub struct AgentHost {
    pub server: Server,
    requests: PathBuf,
    next: Cell<u32>,
    /// Project id → folder, for the `FIN_PROJECT_DIR` the runtime would set.
    dirs: RefCell<HashMap<String, PathBuf>>,
}

/// An agent session: its id, credential file and root branch.
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub credential: PathBuf,
    pub root: Option<String>,
    /// The session's project folder (`FIN_PROJECT_DIR`, as the runtime sets it).
    pub project_dir: Option<PathBuf>,
}

impl AgentHost {
    pub fn start(test: &str) -> Option<AgentHost> {
        let server = Server::start_script(
            test,
            "apps/finisherd/test/support/agent-session-host.ts",
            &["--requests", "{root}/agent-host"],
        );
        // `start_script` makes the folder's parent; the host polls the folder itself.
        AgentHost::with(server?)
    }

    /// A host whose sessions may run on the scripted `fake` harness (plan 7B Task 6): the
    /// test-hooks fin-node binary with `FIN_TEST_HOOKS=1`; `scripts` are the fake's scripts in
    /// open order (`apps/finisherd/src/agent/fake.ts`). Skipped without that binary.
    pub fn start_fake(test: &str, scripts: &Value) -> Option<AgentHost> {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("fake-scripts.json");
        std::fs::write(&file, scripts.to_string()).unwrap();
        let env = vec![
            (
                "FIN_NODE_BINARY_DIR".to_string(),
                super::fontserver::test_native_dir().into(),
            ),
            ("FIN_TEST_HOOKS".to_string(), "1".into()),
        ];
        let file = file.display().to_string();
        let server = Server::start_script_with(
            test,
            "apps/finisherd/test/support/agent-session-host.ts",
            &["--requests", "{root}/agent-host", "--fake-scripts", &file],
            env,
            super::fontserver::why_no_test_hooks(),
        )?;
        let host = AgentHost::with(server);
        // The scripts file is read at start; the folder may go.
        drop(dir);
        host
    }

    fn with(server: Server) -> Option<AgentHost> {
        let requests = server.root.join("agent-host");
        std::fs::create_dir_all(&requests).unwrap();
        Some(AgentHost {
            server,
            requests,
            next: Cell::new(0),
            dirs: RefCell::new(HashMap::new()),
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
        self.open_with(json!({ "op": "open", "projectId": project_id, "mode": mode }))
    }

    /// A session on the `fake` harness (a host from [`AgentHost::start_fake`]).
    pub fn open_fake(&self, project_id: &str, mode: &str) -> Session {
        self.open_with(
            json!({ "op": "open", "projectId": project_id, "mode": mode, "harness": "fake" }),
        )
    }

    fn open_with(&self, req: Value) -> Session {
        let project_id = req["projectId"].as_str().unwrap().to_string();
        let v = self.request(req);
        Session {
            id: v["sessionId"].as_str().unwrap().to_string(),
            credential: PathBuf::from(v["credential"].as_str().unwrap()),
            root: v["rootBranch"].as_str().map(str::to_string),
            project_dir: self.dirs.borrow().get(&project_id).cloned(),
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

    /// Starts the agent listener (plan 7C Task 6); its `tcp:127.0.0.1:<port>` endpoint.
    pub fn listener(&self) -> String {
        self.request(json!({ "op": "listener" }))["endpoint"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// `fin` as a person (the local key).
    pub fn fin(&self, cwd: &Path, args: &[&str]) -> Run {
        super::fin(&self.server.sd, cwd, args)
    }

    /// `fin` as the session: `FIN_CREDENTIAL`, `FIN_SERVER` and `FIN_PROJECT_DIR` set (as the
    /// runtime sets them), plus `env`. Server data is still given (`FIN_SERVER_DATA`): under a
    /// credential it must be ignored.
    pub fn agent_env(&self, s: &Session, cwd: &Path, args: &[&str], env: &[(&str, &OsStr)]) -> Run {
        let mut cmd = fin_cmd(&self.server.sd, cwd, args);
        cmd.env("FIN_CREDENTIAL", &s.credential)
            .env("FIN_SERVER", &self.server.socket);
        if let Some(dir) = &s.project_dir {
            cmd.env("FIN_PROJECT_DIR", dir);
        }
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
        self.dirs.borrow_mut().insert(id.clone(), dir.clone());
        (dir, id)
    }

    /// The session's workspace, created.
    pub fn workspace(&self, dir: &Path, s: &Session) -> PathBuf {
        let w = dir.join("workspace").join(&s.id);
        std::fs::create_dir_all(&w).unwrap();
        w
    }
}
