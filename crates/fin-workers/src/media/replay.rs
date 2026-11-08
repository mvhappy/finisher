//! `fin-workers media --replay <jobdir> [--params <file>]` (spec §3.3): runs a
//! `media.look` job directory without a host, for bug reports and tests. The params are
//! `--params <file>`, else `<jobdir>/in/params.json`; the inputs are the directory's
//! `in/snapshot.json` and `in/manifest.json`, the PNGs go to its `out/`. The family runs over
//! an in-memory envelope exactly as under a host; the job's answer is printed as one JSON line,
//! `{"result": …}` (exit 0) or `{"error": …}` (exit 1). Exit 2 for usage errors.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use fin_jobs::codec::{self, Decoded};
use fin_jobs::worker::{Channel, run_on};
use fin_jobs::{Dirs, FileGrant, FileMode, HostMsg, Limits, Start, StartJob, WorkerMsg};
use serde_json::{Value, json};

use super::MediaFamily;

/// The largest params file read.
const PARAMS_CAP: u64 = 1 << 20;

/// Gives the host's lines, then blocks for ever: the job never sees its host go.
struct Feed(std::io::Cursor<Vec<u8>>);

impl Read for Feed {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.0.read(buf)?;
        if n == 0 {
            loop {
                std::thread::park();
            }
        }
        Ok(n)
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn usage(msg: &str) -> ! {
    eprintln!("fin-workers: {msg}");
    eprintln!("usage: fin-workers media --replay <jobdir> [--params <file>]");
    std::process::exit(2)
}

fn absolute(p: &str) -> PathBuf {
    let p = Path::new(p);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|e| usage(&format!("no current directory: {e}")))
            .join(p)
    }
}

/// The `start` a host would send for `params` over the directory `root`.
pub fn start(root: &Path, params: Value) -> Start {
    let grant = |name: &str, mode| FileGrant {
        name: name.into(),
        path: root.join(name.trim_end_matches('/')),
        mode,
        bytes: None,
    };
    Start {
        job: StartJob {
            id: "replay".into(),
            kind: "media.look".into(),
            kind_version: 1,
            params,
            checkpoint: None,
            attempt: 1,
            extra: None,
        },
        limits: Limits::default(),
        files: vec![
            grant("in/snapshot.json", FileMode::Read),
            grant("in/manifest.json", FileMode::Read),
            grant("out/", FileMode::Write),
        ],
        helpers: vec![],
        dirs: Dirs {
            job: root.to_path_buf(),
            tmp: root.join("tmp"),
        },
        diagnostics: false,
    }
}

/// Runs the replay; returns the exit code.
pub fn main(args: &[String]) -> i32 {
    let mut dir = None;
    let mut params_file = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--params" => {
                params_file = Some(it.next().unwrap_or_else(|| usage("--params needs a file")))
            }
            other if dir.is_none() && !other.starts_with("--") => dir = Some(other),
            other => usage(&format!("unknown argument {other:?}")),
        }
    }
    let dir = dir.unwrap_or_else(|| usage("--replay needs a job directory"));
    let root = match fin_shm::JobDir::resolve(&absolute(dir)) {
        Ok(d) => d.root,
        Err(e) => usage(&format!("{dir}: {e}")),
    };
    let params_path = params_file
        .map(|p| absolute(p))
        .unwrap_or_else(|| root.join("in/params.json"));
    let params = read_params(&params_path).unwrap_or_else(|e| usage(&e));
    let line = codec::encode(1, &HostMsg::Start(start(&root, params)))
        .unwrap_or_else(|e| usage(&format!("the params do not fit a start: {e}")));
    let out = Capture::default();
    let chan = Channel {
        input: Box::new(Feed(std::io::Cursor::new(line))),
        output: Box::new(out.clone()),
        hard_exit: false,
        self_sandbox: None,
        pid: std::process::id(),
    };
    run_on(MediaFamily::new(false), chan);
    let bytes = out.0.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let (answer, code) = answer(&bytes);
    println!("{answer}");
    code
}

fn read_params(path: &Path) -> Result<Value, String> {
    let f =
        fin_shm::checked_open(path, PARAMS_CAP).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut text = Vec::new();
    f.take(PARAMS_CAP)
        .read_to_end(&mut text)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The job's answer among the envelope lines the family wrote.
fn answer(bytes: &[u8]) -> (Value, i32) {
    for (i, line) in bytes.split_inclusive(|&b| b == b'\n').enumerate() {
        match codec::decode_worker(line, i as u64 + 1, false) {
            Ok(Decoded::Msg(WorkerMsg::Result { value, .. })) => {
                return (json!({ "result": value }), 0);
            }
            Ok(Decoded::Msg(WorkerMsg::Error { error, .. })) => {
                return (json!({ "error": error }), 1);
            }
            Ok(_) => {}
            Err(e) => {
                return (
                    json!({ "error": { "code": "WORKER_PROTOCOL", "message": e.to_string() } }),
                    1,
                );
            }
        }
    }
    (
        json!({ "error": { "code": "WORKER_CRASHED", "message": "the job ended without an answer" } }),
        1,
    )
}
