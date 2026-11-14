//! The test kinds (params in `fin_jobs::test_kinds`). Each answers its result type as JSON.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use fin_jobs::test_kinds::*;
use fin_jobs::worker::JobCtx;
use fin_jobs::{FileMode, JobError, Progress, ProgressUnit, Retry, StartJob, WorkerMsg};
use serde::Serialize;
use serde_json::{Value, json};

use super::{params, sleep_or_cancel, spawn_grandchild};

fn out<T: Serialize>(v: T) -> Result<Value, JobError> {
    serde_json::to_value(v).map_err(|e| JobError::new("INTERNAL", e.to_string()))
}

fn io(e: std::io::Error) -> JobError {
    JobError::new("IO_ERROR", e.to_string()).with_retry(Retry::Backoff)
}

/// Runs one job of the `test`, `testbad` or `testplugin` family.
pub(crate) fn run(job: &StartJob, ctx: &mut JobCtx<'_>, served: u64) -> Result<Value, JobError> {
    match job.kind.as_str() {
        "test.sleep" => sleep(job, ctx),
        "test.echo" => echo(job),
        "test.progressFlood" => {
            let p: ProgressFloodParams = params(job)?;
            if let Some(bytes) = p.stderr_bytes {
                let mut err = std::io::stderr().lock();
                for i in 1..=p.n {
                    let head = format!("flood {i} ");
                    let pad = (bytes as usize).saturating_sub(head.len());
                    writeln!(err, "{head}{}", "x".repeat(pad)).map_err(io)?;
                }
            } else {
                for i in 1..=p.n {
                    ctx.progress(Progress::new(i as f64, ProgressUnit::Items).of(p.n as f64));
                }
            }
            out(CountResult { sent: p.n })
        }
        "test.warm" => warm(job, ctx, served),
        "test.crash" => super::batch::crash(job, ctx),
        "test.hang" => {
            ctx.stop_alive_for_test();
            ctx.stop_reading_for_test();
            // Tells the host the hang is in effect (a cancel sent before this is answered, not ridden out).
            ctx.progress(Progress::new(0.0, ProgressUnit::Items));
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        "test.stall" => loop {
            sleep_or_cancel(&ctx.cancel_token(), 50)?;
        },
        "test.spawn" => spawn(job, ctx),
        "test.files" => files(job, ctx),
        "test.checkpoint" => checkpoint(job, ctx),
        "test.oom" => oom(job, ctx),
        "test.fail" => {
            let p: FailParams = params(job)?;
            if p.fail_until.is_none_or(|n| job.attempt <= n) {
                return Err(JobError::new(
                    p.code,
                    format!("test.fail fails attempt {}", job.attempt),
                )
                .with_retry(p.retry));
            }
            out(FailResult {
                attempt: job.attempt,
            })
        }
        "test.call" => {
            let p: CallParams = params(job)?;
            ctx.call(&p.method, p.params)
        }
        "test.diag" => {
            ctx.diag("test.diag", json!({ "n": 1 }));
            out(DiagResult {
                sent: ctx.diagnostics(),
            })
        }
        "test.garbage" => garbage(job, ctx),
        "test.exitAfterResult" => {
            ctx.send_for_test(&WorkerMsg::Result {
                job: job.id.clone(),
                value: json!({ "exit": 0 }),
            });
            std::process::exit(0)
        }
        "test.helper" => {
            let p: HelperParams = params(job)?;
            sleep_or_cancel(&ctx.cancel_token(), p.delay_ms.unwrap_or(0))?;
            out(HelperResult {
                helpers: ctx.helpers().iter().map(|h| h.id.clone()).collect(),
            })
        }
        "test.badHello" => Ok(job.params.clone()),
        "test.pluginMinor" => {
            let p: PluginMinorParams = params(job)?;
            let reply = match ctx.call_unknown_for_test("futureThing") {
                Ok(v) => json!({ "value": v }),
                Err(e) => json!({ "error": e }),
            };
            out(PluginMinorResult {
                reply,
                pid: std::process::id(),
                served,
                key: p.key,
            })
        }
        other => Err(JobError::new(
            "JOB_KIND_UNKNOWN",
            format!("no kind {other}"),
        )),
    }
}

fn sleep(job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
    let p: SleepParams = params(job)?;
    let token = ctx.cancel_token();
    let t0 = Instant::now();
    let every = p.progress_every_ms.unwrap_or(0);
    let mut slept = 0;
    while slept < p.ms {
        let step = if every > 0 {
            every.min(p.ms - slept)
        } else {
            p.ms - slept
        };
        sleep_or_cancel(&token, step)?;
        slept += step;
        if every > 0 {
            ctx.progress(
                Progress::new(slept as f64 / 1000.0, ProgressUnit::Seconds)
                    .of(p.ms as f64 / 1000.0),
            );
        }
    }
    out(SleepResult {
        slept_ms: t0.elapsed().as_millis() as u64,
    })
}

fn echo(job: &StartJob) -> Result<Value, JobError> {
    let p: EchoParams = params(job)?;
    let mut r = EchoResult {
        value: p.value,
        pid: std::process::id(),
        env: None,
        args: None,
        fds: None,
        cwd: None,
        pdeathsig: None,
        nice: None,
        cgroup: None,
    };
    for what in p.report {
        match what {
            EchoReport::Pid => {}
            EchoReport::Env => {
                r.env = Some(
                    std::env::vars_os()
                        .map(|(k, v)| {
                            (
                                k.to_string_lossy().into_owned(),
                                v.to_string_lossy().into_owned(),
                            )
                        })
                        .collect::<BTreeMap<_, _>>(),
                )
            }
            EchoReport::Args => r.args = Some(std::env::args().collect()),
            EchoReport::Fds => r.fds = open_fds(),
            EchoReport::Cwd => {
                r.cwd = std::env::current_dir()
                    .ok()
                    .map(|d| d.display().to_string())
            }
            EchoReport::Pdeathsig => r.pdeathsig = pdeathsig(),
            EchoReport::Nice => r.nice = nice(),
            EchoReport::Cgroup => r.cgroup = super::start_cgroup(),
        }
    }
    out(r)
}

/// The open descriptors, probed with `fcntl(F_GETFD)` (which opens none itself).
#[cfg(unix)]
fn open_fds() -> Option<Vec<u32>> {
    // SAFETY: F_GETFD only reads a descriptor's flags.
    Some(
        (0..4096)
            .filter(|&fd| unsafe { libc::fcntl(fd, libc::F_GETFD) } != -1)
            .map(|fd| fd as u32)
            .collect(),
    )
}

#[cfg(not(unix))]
fn open_fds() -> Option<Vec<u32>> {
    None
}

#[cfg(target_os = "linux")]
fn pdeathsig() -> Option<i32> {
    let mut sig: libc::c_int = 0;
    // SAFETY: PR_GET_PDEATHSIG writes one int.
    let r = unsafe { libc::prctl(libc::PR_GET_PDEATHSIG, &mut sig as *mut libc::c_int) };
    (r == 0).then_some(sig)
}

#[cfg(not(target_os = "linux"))]
fn pdeathsig() -> Option<i32> {
    None
}

#[cfg(unix)]
fn nice() -> Option<i32> {
    // SAFETY: reads this process's priority.
    Some(unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) })
}

#[cfg(not(unix))]
fn nice() -> Option<i32> {
    None
}

fn warm(job: &StartJob, ctx: &mut JobCtx<'_>, served: u64) -> Result<Value, JobError> {
    let p: WarmParams = params(job)?;
    if p.delay_ms.is_some_and(|ms| ms > 0) {
        // The job is running (a cancel sent before this reaches a job that never started).
        ctx.progress(Progress::new(0.0, ProgressUnit::Items));
    }
    sleep_or_cancel(&ctx.cancel_token(), p.delay_ms.unwrap_or(0))?;
    if let Some(name) = &p.hold_file {
        // Breaks the warm contract on purpose: the file stays open past `ready`.
        let file = File::open(grant_path(ctx, name)).map_err(io)?;
        std::mem::forget(file);
    }
    // A directory is listed, a file read: either way a denial is the errno.
    let probes = p
        .probe
        .iter()
        .map(|path| {
            if let Some(path) = path.strip_prefix('!') {
                let moved = format!("{path}.moved");
                let r = std::fs::rename(path, &moved);
                if r.is_ok() {
                    let _ = std::fs::rename(&moved, path);
                }
                return r.err().map(|e| e.raw_os_error().unwrap_or(-1));
            }
            let r = match std::fs::metadata(path) {
                Ok(m) if m.is_dir() => std::fs::read_dir(path).map(|_| ()),
                _ => std::fs::read(path).map(|_| ()),
            };
            r.err().map(|e| e.raw_os_error().unwrap_or(-1))
        })
        .collect();
    out(WarmResult {
        value: p.value,
        pid: std::process::id(),
        served,
        probes,
    })
}

/// A mapped file's path, or the name under the job directory.
fn grant_path(ctx: &JobCtx<'_>, name: &str) -> PathBuf {
    ctx.file(name)
        .map(|f| f.path.clone())
        .unwrap_or_else(|| ctx.dirs().job.join(name))
}

fn spawn(job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
    let p: SpawnParams = params(job)?;
    let (mid, grandchild) =
        spawn_grandchild(p.detach, p.grandchild_ms.unwrap_or(600_000)).map_err(io)?;
    let pids = SpawnPids {
        worker: std::process::id(),
        mid,
        grandchild,
    };
    let text = serde_json::to_vec(&pids).map_err(|e| JobError::new("INTERNAL", e.to_string()))?;
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(grant_path(ctx, &p.out))
        .map_err(io)?;
    f.write_all(&text).and_then(|()| f.sync_all()).map_err(io)?;
    loop {
        sleep_or_cancel(&ctx.cancel_token(), 50)?;
    }
}

/// FNV-1a 64 as hex.
fn fnv(bytes: &[u8]) -> String {
    let h = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{h:016x}")
}

fn pattern(len: u64) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

fn files(job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
    let p: FilesParams = params(job)?;
    let mut reports = Vec::new();
    let mut out_empty = None;
    for g in ctx.files() {
        if g.name.ends_with('/') {
            if g.mode != FileMode::Read {
                let empty = std::fs::read_dir(&g.path).map_err(io)?.next().is_none();
                out_empty = Some(out_empty.unwrap_or(true) && empty);
                std::fs::write(g.path.join("attempt.txt"), job.attempt.to_string()).map_err(io)?;
            }
            continue;
        }
        let mut f = OpenOptions::new()
            .read(true)
            .write(g.mode != FileMode::Read)
            .open(&g.path)
            .map_err(io)?;
        let mut content = Vec::new();
        f.read_to_end(&mut content).map_err(io)?;
        if g.mode != FileMode::Read {
            content = pattern(g.bytes.unwrap_or(content.len() as u64));
            f.seek(SeekFrom::Start(0))
                .and_then(|_| f.write_all(&content))
                .map_err(io)?;
        }
        reports.push(FileReport {
            name: g.name.clone(),
            mode: g.mode,
            bytes: content.len() as u64,
            hash: fnv(&content),
        });
    }
    if p.fail_on_attempt == Some(job.attempt) {
        return Err(
            JobError::new("IO_ERROR", "test.files fails this attempt").with_retry(Retry::Backoff)
        );
    }
    out(FilesResult {
        files: reports,
        out_empty,
    })
}

fn checkpoint(job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
    let p: CheckpointParams = params(job)?;
    let from = job
        .checkpoint
        .as_ref()
        .and_then(|c| c["step"].as_u64())
        .unwrap_or(0) as u32;
    let token = ctx.cancel_token();
    for step in from..p.steps {
        if let Some(reason) = ctx.cancelled() {
            if reason == fin_jobs::CancelReason::Pause {
                ctx.checkpoint(json!({ "step": step }))?;
            }
            return Err(JobError::cancelled());
        }
        if p.crash_at == Some(step) && job.attempt == 1 {
            std::process::abort();
        }
        if sleep_or_cancel(&token, p.step_ms.unwrap_or(10)).is_err() {
            if ctx.cancelled() == Some(fin_jobs::CancelReason::Pause) {
                ctx.checkpoint(json!({ "step": step }))?;
            }
            return Err(JobError::cancelled());
        }
        ctx.checkpoint(json!({ "step": step + 1 }))?;
        ctx.progress(
            Progress::new(f64::from(step + 1), ProgressUnit::Items).of(f64::from(p.steps)),
        );
    }
    out(CheckpointResult {
        steps: p.steps,
        resumed_from: from,
    })
}

fn oom(job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
    let p: OomParams = params(job)?;
    let len =
        usize::try_from(p.mib << 20).map_err(|_| JobError::new("JOB_PARAMS_INVALID", "mib"))?;
    let mut held = vec![0u8; len];
    for i in (0..len).step_by(4096) {
        held[i] = 1;
    }
    std::hint::black_box(&held);
    // Unmapped when the job returns.
    let _mapped = match &p.map_file {
        Some(name) => Some(map_and_touch(&grant_path(ctx, name)).map_err(io)?),
        None => None,
    };
    if let Some(name) = &p.ready_file {
        std::fs::write(grant_path(ctx, name), b"1").map_err(io)?;
    }
    sleep_or_cancel(&ctx.cancel_token(), p.hold_ms.unwrap_or(0))?;
    drop(held);
    out(OomResult { mib: p.mib })
}

/// A read-only private mapping of `path` with every page read (file-backed, reclaimable).
#[cfg(unix)]
fn map_and_touch(path: &std::path::Path) -> std::io::Result<Mapped> {
    use std::os::fd::AsRawFd;
    let file = File::open(path)?;
    let len = file.metadata()?.len() as usize;
    // SAFETY: maps a file we opened read-only; the mapping is unmapped by `Mapped`'s drop.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            file.as_raw_fd(),
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(std::io::Error::last_os_error());
    }
    let mut sum = 0u8;
    for i in (0..len).step_by(4096) {
        // SAFETY: inside the mapping just made.
        sum ^= unsafe { std::ptr::read_volatile(ptr.cast::<u8>().add(i)) };
    }
    std::hint::black_box(sum);
    Ok(Mapped(ptr, len))
}

#[cfg(unix)]
struct Mapped(*mut libc::c_void, usize);

#[cfg(unix)]
impl Drop for Mapped {
    fn drop(&mut self) {
        // SAFETY: unmaps the mapping `map_and_touch` made.
        unsafe { libc::munmap(self.0, self.1) };
    }
}

#[cfg(not(unix))]
struct Mapped;

#[cfg(not(unix))]
fn map_and_touch(_path: &std::path::Path) -> std::io::Result<Mapped> {
    Err(std::io::Error::other("mapFile is Unix-only"))
}

fn garbage(job: &StartJob, ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
    let p: GarbageParams = params(job)?;
    match p.what {
        Garbage::Json => ctx.send_raw_for_test(b"{not json\n"),
        Garbage::Seq => {
            ctx.skip_seq_for_test();
            ctx.send_for_test(&WorkerMsg::Alive {});
        }
        Garbage::Long => {
            let mut line = vec![b'x'; fin_jobs::codec::MAX_LINE + 16];
            line.push(b'\n');
            ctx.send_raw_for_test(&line);
        }
        Garbage::OtherJob => ctx.send_for_test(&WorkerMsg::Progress(Progress {
            job: "other-job".into(),
            ..Progress::new(1.0, ProgressUnit::Items)
        })),
        Garbage::UnknownType => ctx.send_type_for_test("bogus"),
    }
    loop {
        sleep_or_cancel(&ctx.cancel_token(), 50)?;
    }
}
