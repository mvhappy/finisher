//! A test host over in-memory pipes: drives `worker::run_on` / `helper::run_on` on a thread.

#![allow(dead_code)]

use std::io::{BufReader, PipeWriter, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::Duration;

use fin_jobs::codec::{self, Decoded, Line, MAX_LINE};
use fin_jobs::worker::{Channel, Family};
use fin_jobs::{Dirs, HostMsg, Limits, Start, StartJob, WorkerMsg};
use serde_json::Value;

pub const PID: u32 = 4242;
const WAIT: Duration = Duration::from_secs(10);

pub struct Rig {
    to: Option<PipeWriter>,
    seq: u64,
    from: Receiver<WorkerMsg>,
    run: Option<JoinHandle<i32>>,
}

impl Rig {
    fn start(
        run: impl FnOnce(Channel) -> i32 + Send + 'static,
        self_sandbox: Option<String>,
    ) -> Rig {
        let (in_r, in_w) = std::io::pipe().unwrap();
        let (out_r, out_w) = std::io::pipe().unwrap();
        let chan = Channel {
            input: Box::new(in_r),
            output: Box::new(out_w),
            hard_exit: false,
            self_sandbox,
            pid: PID,
        };
        let run = std::thread::spawn(move || run(chan));
        let (tx, from) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(out_r);
            let mut buf = Vec::new();
            let mut seq = 1;
            while let Ok(Line::Line) = codec::read_line(&mut r, &mut buf, MAX_LINE) {
                match codec::decode_worker(&buf, seq, false) {
                    Ok(Decoded::Msg(m)) => {
                        if tx.send(m).is_err() {
                            return;
                        }
                    }
                    other => panic!(
                        "bad worker line {:?}: {other:?}",
                        String::from_utf8_lossy(&buf)
                    ),
                }
                seq += 1;
            }
        });
        Rig {
            to: Some(in_w),
            seq: 0,
            from,
            run: Some(run),
        }
    }

    pub fn worker(family: impl Family + 'static) -> Rig {
        Rig::start(move |c| fin_jobs::worker::run_on(family, c), None)
    }

    pub fn worker_sandboxed(family: impl Family + 'static, request: &str) -> Rig {
        Rig::start(
            move |c| fin_jobs::worker::run_on(family, c),
            Some(request.to_owned()),
        )
    }

    pub fn helper(helper: impl fin_jobs::helper::Helper + 'static) -> Rig {
        Rig::start(move |c| fin_jobs::helper::run_on(helper, c), None)
    }

    pub fn send(&mut self, msg: &HostMsg) {
        self.seq += 1;
        let line = codec::encode(self.seq, msg).unwrap();
        self.to
            .as_mut()
            .expect("stdin open")
            .write_all(&line)
            .unwrap();
    }

    /// The next message, `alive` included.
    pub fn recv_any(&mut self) -> WorkerMsg {
        self.from.recv_timeout(WAIT).expect("a message within 10 s")
    }

    /// The next message other than `alive`.
    pub fn recv(&mut self) -> WorkerMsg {
        loop {
            match self.recv_any() {
                WorkerMsg::Alive {} => continue,
                m => return m,
            }
        }
    }

    /// Whether the worker sent nothing (but `alive`) for `d`.
    pub fn quiet_for(&mut self, d: Duration) -> bool {
        let until = std::time::Instant::now() + d;
        loop {
            let left = until.saturating_duration_since(std::time::Instant::now());
            match self.from.recv_timeout(left) {
                Ok(WorkerMsg::Alive {}) => continue,
                Ok(m) => panic!("unexpected {m:?}"),
                Err(_) => return true,
            }
        }
    }

    pub fn close_stdin(&mut self) {
        self.to = None;
    }

    /// The exit code (closes stdin first).
    pub fn join(mut self) -> i32 {
        self.close_stdin();
        self.run
            .take()
            .unwrap()
            .join()
            .expect("the runtime does not panic")
    }

    /// The exit code, with stdin still open.
    pub fn join_open(mut self) -> i32 {
        let code = self
            .run
            .take()
            .unwrap()
            .join()
            .expect("the runtime does not panic");
        self.to = None;
        code
    }
}

pub fn dirs() -> Dirs {
    Dirs {
        job: PathBuf::from("/job"),
        tmp: PathBuf::from("/job/tmp"),
    }
}

pub fn start(id: &str, kind: &str, params: Value) -> HostMsg {
    HostMsg::Start(Start {
        job: StartJob {
            id: id.into(),
            kind: kind.into(),
            kind_version: 1,
            params,
            checkpoint: None,
            attempt: 1,
            extra: None,
        },
        limits: Limits::default(),
        files: vec![],
        helpers: vec![],
        dirs: dirs(),
        diagnostics: false,
    })
}

pub fn start_with(id: &str, params: Value, f: impl FnOnce(&mut Start)) -> HostMsg {
    let HostMsg::Start(mut s) = start(id, "toy.run", params) else {
        unreachable!()
    };
    f(&mut s);
    HostMsg::Start(s)
}

type Run = Box<
    dyn FnMut(&StartJob, &mut fin_jobs::worker::JobCtx<'_>) -> Result<Value, fin_jobs::JobError>
        + Send,
>;

/// A family of one kind (`toy.run`) whose `run` is a closure.
pub struct Toy {
    pub warm: bool,
    f: Run,
}

static TOY_KINDS: &[fin_jobs::KindVersion] = &[fin_jobs::KindVersion::new("toy.run", 1)];

impl Family for Toy {
    fn name(&self) -> &'static str {
        "toy"
    }

    fn kinds(&self) -> &'static [fin_jobs::KindVersion] {
        TOY_KINDS
    }

    fn warm(&self) -> bool {
        self.warm
    }

    fn run(
        &mut self,
        job: &StartJob,
        ctx: &mut fin_jobs::worker::JobCtx<'_>,
    ) -> Result<Value, fin_jobs::JobError> {
        (self.f)(job, ctx)
    }
}

pub fn toy(
    f: impl FnMut(&StartJob, &mut fin_jobs::worker::JobCtx<'_>) -> Result<Value, fin_jobs::JobError>
    + Send
    + 'static,
) -> Toy {
    Toy {
        warm: false,
        f: Box::new(f),
    }
}

pub fn warm_toy(
    f: impl FnMut(&StartJob, &mut fin_jobs::worker::JobCtx<'_>) -> Result<Value, fin_jobs::JobError>
    + Send
    + 'static,
) -> Toy {
    Toy {
        warm: true,
        f: Box::new(f),
    }
}

/// The echo toy: returns its params.
pub fn echo() -> Toy {
    toy(|job, _| Ok(job.params.clone()))
}
