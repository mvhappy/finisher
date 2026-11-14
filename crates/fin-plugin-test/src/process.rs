//! A plugin process started as the real host starts it (sp8b Task 4's family): the runtime's
//! program with a trailing `--warm` for a warm process, the environment allow-list plus
//! `FIN_PLUGIN_PROTOCOL` and the private temp as `TMPDIR`/`TEMP`/`TMP` and working directory,
//! and stdio only. Its stdout is decoded strictly as the envelope's worker side (a plugin whose
//! `hello` names a newer minor gets unknown types dropped, Decision 21); its stderr is kept.

use std::ffi::OsString;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fin_jobs::codec::{self, Decoded, Line, MAX_LINE};
use fin_jobs::{HostMsg, WorkerMsg};

use crate::HarnessError;
use crate::plugin::{Plugin, Program};

/// The argument that makes a process warm: the last one.
pub const WARM_ARG: &str = "--warm";
/// The plugin protocol the process speaks (`FIN_PLUGIN_PROTOCOL`).
pub const PLUGIN_PROTOCOL: &str = "1";
/// Lines of stderr kept per process.
const STDERR_LINES: usize = 1000;

/// The directories a run gives its processes.
#[derive(Debug, Clone)]
pub struct RunDirs {
    /// The private temp: the working directory and `TMPDIR`.
    pub tmp: PathBuf,
    /// `pluginData` (when `fs pluginData` is declared) and whether it is writable.
    pub data: Option<(PathBuf, bool)>,
}

/// Variables copied from the harness's environment when present (`childEnv`'s allow-list).
const ALLOWED: &[&str] = &["PATH", "HOME", "LANG", "TMPDIR"];
const ALLOWED_WINDOWS: &[&str] = &[
    "Path",
    "USERPROFILE",
    "SYSTEMROOT",
    "SystemRoot",
    "TEMP",
    "TMP",
    "LOCALAPPDATA",
];

/// The environment a plugin process gets: the allow-list, then the protocol and its temp.
pub fn child_env(tmp: &Path) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(k, _)| {
            let k = k.to_string_lossy();
            ALLOWED.contains(&k.as_ref())
                || k.starts_with("LC_")
                || (cfg!(windows) && ALLOWED_WINDOWS.contains(&k.as_ref()))
        })
        .collect();
    env.retain(|(k, _)| !["TMPDIR", "TEMP", "TMP"].contains(&k.to_string_lossy().as_ref()));
    env.push(("FIN_PLUGIN_PROTOCOL".into(), PLUGIN_PROTOCOL.into()));
    for k in ["TMPDIR", "TEMP", "TMP"] {
        env.push((k.into(), tmp.into()));
    }
    env
}

/// Node's permission flags for a `node` plugin (`node-runtime.ts`'s `nodeArgs`): read its
/// package, read (and write, when granted) `pluginData`, read and write its temp; no child
/// processes, addons, workers or WASI.
pub fn node_args(
    plugin_dir: &Path,
    main: &Path,
    dirs: &RunDirs,
) -> Result<Vec<OsString>, HarnessError> {
    // Node expands `*` in a flag's path and warns on `,` (`node-runtime.ts`'s `flagPath`).
    let mut paths = vec![plugin_dir, main, dirs.tmp.as_path()];
    if let Some((d, _)) = &dirs.data {
        paths.push(d);
    }
    if paths
        .iter()
        .any(|p| p.to_string_lossy().contains(['*', ',']))
    {
        return Err(HarnessError::new(
            "PLUGIN_RUNTIME_UNAVAILABLE",
            "a plugin path holds a character Node's permission flags cannot take",
        )
        .with("runtime", "node")
        .with("reason", "pathCharacter"));
    }
    let flag = |name: &str, p: &Path| {
        let mut s = OsString::from(format!("--allow-fs-{name}="));
        s.push(p);
        s
    };
    let mut args = vec![OsString::from("--permission"), flag("read", plugin_dir)];
    if let Some((data, write)) = &dirs.data {
        args.push(flag("read", data));
        if *write {
            args.push(flag("write", data));
        }
    }
    args.push(flag("read", &dirs.tmp));
    args.push(flag("write", &dirs.tmp));
    args.push(main.into());
    Ok(args)
}

/// What became of a message to the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    Ok,
    /// The process no longer reads.
    Gone,
    /// Over the envelope's 1 MiB line: not sent.
    TooLarge,
}

/// What the process sent, or how its output ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Msg(WorkerMsg),
    /// A type this host does not know from a plugin of a newer minor (dropped).
    Unknown(String),
    /// A line that breaks the envelope (framing, `seq`, `v`, an unknown type, the shape of a
    /// known type, over 1 MiB): the process is killed, as the host kills it.
    Bad(String),
    /// Stdout closed.
    Eof,
}

pub struct PluginProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    seq: u64,
    rx: Receiver<Event>,
    stderr: Arc<Mutex<Vec<String>>>,
    pub warm: bool,
    /// The job kinds its `hello` named (set by [`crate::fake_host::FakeHost::hello`]).
    pub hello_kinds: Vec<fin_jobs::KindVersion>,
    ended: bool,
}

impl PluginProcess {
    pub fn spawn(
        plugin: &Plugin,
        dirs: &RunDirs,
        warm: bool,
    ) -> Result<PluginProcess, HarnessError> {
        let (program, mut args): (&Path, Vec<OsString>) = match &plugin.program {
            Program::Native { exe } => (exe, Vec::new()),
            Program::Node { node, main } => (node, node_args(&plugin.dir, main, dirs)?),
        };
        if warm {
            args.push(WARM_ARG.into());
        }
        let mut child = Command::new(program)
            .args(&args)
            .env_clear()
            .envs(child_env(&dirs.tmp))
            .current_dir(&dirs.tmp)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                HarnessError::new(
                    "PLUGIN_RUNTIME_UNAVAILABLE",
                    format!("{} does not start: {e}", program.display()),
                )
                .with("reason", "spawn")
            })?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || read_worker(stdout, &tx));
        let lines = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&lines);
        std::thread::spawn(move || read_stderr(stderr, &kept));
        Ok(PluginProcess {
            stdin: child.stdin.take(),
            child,
            seq: 0,
            rx,
            stderr: lines,
            warm,
            hello_kinds: Vec::new(),
            ended: false,
        })
    }

    /// Sends `msg`: [`Sent::Gone`] when the process no longer reads (it exited),
    /// [`Sent::TooLarge`] (nothing sent, `seq` unchanged) when it does not fit one line.
    pub fn send(&mut self, msg: &HostMsg) -> Sent {
        let Ok(line) = codec::encode(self.seq + 1, msg) else {
            return Sent::TooLarge;
        };
        self.seq += 1;
        let Some(stdin) = self.stdin.as_mut() else {
            return Sent::Gone;
        };
        match stdin.write_all(&line).and_then(|()| stdin.flush()) {
            Ok(()) => Sent::Ok,
            Err(_) => Sent::Gone,
        }
    }

    /// The next event other than `alive` before `deadline`; `None` when none came. After
    /// [`Event::Eof`] or [`Event::Bad`] every call answers `Eof`.
    pub fn recv(&mut self, deadline: Instant) -> Option<Event> {
        if self.ended {
            return Some(Event::Eof);
        }
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(Event::Msg(WorkerMsg::Alive {})) => continue,
                Ok(e) => {
                    if matches!(e, Event::Eof | Event::Bad(_)) {
                        self.ended = true;
                    }
                    return Some(e);
                }
                Err(RecvTimeoutError::Timeout) => return None,
                Err(RecvTimeoutError::Disconnected) => {
                    self.ended = true;
                    return Some(Event::Eof);
                }
            }
        }
    }

    /// Closes stdin (the host is gone: a runtime exits 70).
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// The exit status once the process ends within `within`; `None` if it is still running.
    pub fn wait_exit(&mut self, within: Duration) -> Option<ExitStatus> {
        let until = Instant::now() + within;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            if Instant::now() >= until {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Kills the process (no-op once it exited) and reaps it.
    pub fn kill(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.ended = true;
    }

    /// The stderr lines so far.
    pub fn stderr_lines(&self) -> Vec<String> {
        self.stderr.lock().map(|l| l.clone()).unwrap_or_default()
    }
}

impl Drop for PluginProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

fn read_worker(stdout: impl Read, tx: &mpsc::Sender<Event>) {
    let mut r = BufReader::new(stdout);
    let mut buf = Vec::new();
    let mut seq = 1;
    // Decided by `hello`: a newer minor's unknown types are dropped.
    let mut lenient = false;
    loop {
        let event = match codec::read_line(&mut r, &mut buf, MAX_LINE) {
            Ok(Line::Line) => match codec::decode_worker(&buf, seq, lenient) {
                Ok(Decoded::Msg(m)) => {
                    if let WorkerMsg::Hello(h) = &m {
                        lenient = h.minor > fin_jobs::MINOR;
                    }
                    Event::Msg(m)
                }
                Ok(Decoded::Unknown { t, .. }) => Event::Unknown(t),
                Err(e) => Event::Bad(e.to_string()),
            },
            Ok(Line::TooLong { len }) => Event::Bad(format!("a line of {len} bytes, over 1 MiB")),
            Ok(Line::Eof) | Err(_) => Event::Eof,
        };
        seq += 1;
        let last = matches!(event, Event::Eof | Event::Bad(_));
        if tx.send(event).is_err() || last {
            return;
        }
    }
}

fn read_stderr(stderr: impl Read, lines: &Mutex<Vec<String>>) {
    let mut r = BufReader::new(stderr);
    let mut buf = Vec::new();
    while let Ok(Line::Line | Line::TooLong { .. }) = codec::read_line(&mut r, &mut buf, 64 * 1024)
    {
        let line = String::from_utf8_lossy(&buf).into_owned();
        if let Ok(mut l) = lines.lock()
            && l.len() < STDERR_LINES
        {
            l.push(line);
        }
    }
}
