//! A real `finisherd` for `fin`'s online tests, and a `fin` runner.
//!
//! `Server::start()` spawns `node apps/finisherd/src/main.ts --server-data <tmp>` and waits for
//! `finisherd.json`. When Node 26+, `pnpm install` or the fin-node binary is missing it returns
//! `None` (after printing why) — but never silently: each test binary also has
//! `server_tests_ran_or_explicitly_skipped`, which fails in that case unless
//! `FIN_SKIP_SERVER_TESTS=1` says the skip is intended. With `FIN_REQUIRE_SERVER=1` (CI) every
//! server test panics instead of skipping. The server is killed when the guard drops,
//! including when a test panics.

#![allow(dead_code)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// A temp dir with symlinks resolved (macOS `/var` is `/private/var`), so it compares equal to
/// what the child sees as its working directory.
pub fn real_tempdir() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let real = if cfg!(windows) {
        dir.path().to_path_buf()
    } else {
        dir.path().canonicalize().unwrap()
    };
    (dir, real)
}

fn why_unavailable(root: &Path) -> Option<String> {
    let version = match Command::new("node").arg("--version").output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => return Some("`node` is not on PATH (source .superpowers/env.sh)".into()),
    };
    let major: u32 = version
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .unwrap_or(0);
    if major < 26 {
        return Some(format!(
            "finisherd needs Node 26+, found {version} (source .superpowers/env.sh)"
        ));
    }
    if !root.join("apps/finisherd/node_modules").is_dir() {
        return Some("apps/finisherd/node_modules is missing (run `pnpm install`)".into());
    }
    let binary = std::fs::read_dir(root.join("packages/fin-node"))
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".node"))
        })
        .unwrap_or(false);
    if !binary {
        return Some("no fin-node binary (run `pnpm --filter @finisher/fin-node build`)".into());
    }
    None
}

/// Set (to `1`) in CI: a missing Node or fin-node binary fails every server test.
pub const REQUIRE_ENV: &str = "FIN_REQUIRE_SERVER";
/// Set (to `1`) to skip the server tests on purpose on a machine without the JS side.
pub const SKIP_ENV: &str = "FIN_SKIP_SERVER_TESTS";

fn env_on(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

/// The body of each test binary's `server_tests_ran_or_explicitly_skipped`: passes when the
/// server tests can run, or when `FIN_SKIP_SERVER_TESTS=1` asks to skip them.
pub fn server_tests_ran_or_explicitly_skipped() {
    let Some(reason) = why_unavailable(&repo_root()) else {
        return;
    };
    if env_on(SKIP_ENV) && !env_on(REQUIRE_ENV) {
        eprintln!("SKIPPED fin server tests ({SKIP_ENV}=1): {reason}");
        return;
    }
    panic!(
        "fin's server tests cannot run: {reason}. Fix that, or set {SKIP_ENV}=1 to skip them on purpose"
    );
}

/// A running finisherd with its own server-data directory.
pub struct Server {
    child: Option<Child>,
    _tmp: TempDir,
    /// The temp root (symlinks resolved); `sd` lives inside it, tests may use the rest.
    pub root: PathBuf,
    pub sd: PathBuf,
    pub socket: String,
}

impl Server {
    /// Starts a server, or prints why it cannot and returns `None`.
    pub fn start(test: &str) -> Option<Server> {
        let repo = repo_root();
        if let Some(reason) = why_unavailable(&repo) {
            assert!(!env_on(REQUIRE_ENV), "{test}: {REQUIRE_ENV}=1 but {reason}");
            eprintln!("SKIPPED {test}: {reason}");
            return None;
        }
        let (tmp, root) = real_tempdir();
        let sd = root.join("server-data");
        let log = std::fs::File::create(root.join("finisherd.log")).unwrap();
        let child = Command::new("node")
            .arg(repo.join("apps/finisherd/src/main.ts"))
            .arg("--server-data")
            .arg(&sd)
            .env_remove("FIN_SERVER")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("spawn finisherd");
        let mut server = Server {
            child: Some(child),
            _tmp: tmp,
            root,
            sd,
            socket: String::new(),
        };
        let info = server.sd.join("finisherd.json");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(text) = std::fs::read_to_string(&info)
                && let Ok(json) = serde_json::from_str::<Value>(&text)
            {
                server.socket = json["socket"].as_str().unwrap().to_string();
                return Some(server);
            }
            if let Some(status) = server.child.as_mut().unwrap().try_wait().unwrap() {
                panic!("finisherd exited with {status}: {}", server.log());
            }
            assert!(
                Instant::now() < deadline,
                "finisherd did not start: {}",
                server.log()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.root.join("finisherd.log")).unwrap_or_default()
    }

    /// Stops the server now (the guard would otherwise do it on drop).
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct Run {
    pub code: i32,
    pub json: Value,
    pub stdout: String,
    pub stderr: String,
}

/// A `fin` invocation with a clean environment for the variables fin reads.
pub fn fin_cmd(sd: &Path, cwd: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_fin"));
    cmd.args(args)
        .current_dir(cwd)
        .env("FIN_SERVER_DATA", sd)
        .env_remove("FIN_SERVER")
        .env_remove("FIN_OFFLINE_SNAPSHOT")
        .env_remove("FIN_TEST_COMMIT_GATE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

pub fn finish(args: &[&str], out: Output) -> Run {
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(
        stdout.lines().count(),
        1,
        "exactly one JSON document for {args:?}: {stdout}"
    );
    let json = serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout}"));
    let code = out.status.code().unwrap();
    if code == 0 {
        assert!(
            stderr.is_empty(),
            "stderr on success for {args:?}: {stderr}"
        );
    }
    Run {
        code,
        json,
        stdout,
        stderr,
    }
}

pub fn fin_env(sd: &Path, cwd: &Path, args: &[&str], env: &[(&str, &OsStr)]) -> Run {
    let mut cmd = fin_cmd(sd, cwd, args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    finish(args, cmd.output().unwrap())
}

pub fn fin(sd: &Path, cwd: &Path, args: &[&str]) -> Run {
    fin_env(sd, cwd, args, &[])
}

/// Asserts success and returns the document.
pub fn ok(run: Run) -> Value {
    assert_eq!(run.code, 0, "{}", run.stdout);
    run.json
}

pub fn assert_error(run: &Run, code: i32, error: &str) {
    assert_eq!(run.code, code, "{}", run.stdout);
    assert_eq!(run.json["error"]["code"], error, "{}", run.stdout);
    assert!(
        run.json["error"]["fix"]
            .as_str()
            .is_some_and(|f| !f.is_empty()),
        "no fix: {}",
        run.stdout
    );
}

/// Writes a small MP4 (H.264 video + AAC audio) the server can probe.
pub fn media(path: &Path, seconds: u32) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    fin_probe::fixtures::write_fixture_mp4(
        path,
        fin_probe::fixtures::FixtureOpts {
            seconds,
            ..Default::default()
        },
    );
}
