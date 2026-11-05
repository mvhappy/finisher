//! `xhost`: one node of the cross-host storage harness (plan 4A Task 19,
//! `tests/storage-xhost/`). Not shipped.
//!
//! Subcommands (each prints JSONL events on stdout, see `emit`):
//! - `create --data S --dir D [--free]`: makes the project. `--free` then removes the
//!   creator's lease files, so racers start on a project nobody holds (the creator would
//!   otherwise hold it until stale, about 70 s);
//! - `holder --data S --dir D --secs N --commit-every-ms M`: opens, ticks until it holds, then
//!   commits a small op every M ms and ticks. `contender` is the same loop (it is told apart
//!   only by its role in the events);
//! - `follower --data S --dir D --secs N`: read-only; logs every applied record;
//! - `verify --data S --dir D`: opens (normally with fresh server data, so the database is
//!   rebuilt from the folder), then reports `Store::verify`, the resolved chain, the
//!   quarantine and the folder's segments and lease files.
//!
//! Common options: `--label L` (default: the subcommand), `--start-at-ms T` (Unix ms: wait
//! for it, to start nodes together), `--tick-ms` (default 250), `--clock-every-ms` (default
//! 5000), `--wall-skew-ms K` (emulated wall-clock skew of the event stamps only), `--skew-ms K`
//! (emulated server-time skew: this store's lease sees the share's clock K ms off, as with a
//! client-set mtime on a skewed client).
//!
//! Stalls (holder/contender): `--stall-at-ms T --stall-for-ms U [--stall-mode M]`, T from the
//! start of the loop. Modes:
//! - `freeze` (default): one last commit, then no ticks, no commits and every heartbeat fails
//!   (a hung share or lost network, the process alive);
//! - `ticks`: no ticks or commits; the heartbeat goes on (a stuck main thread);
//! - `beats`: every heartbeat fails; ticks and commits go on;
//! - `check`: every heartbeat fails and the next full fence check stops after reading the
//!   record (`HangPoint::CheckAfterVerify`) until U has passed: a write in flight.

mod db;
mod emit;
mod node;
mod verify;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use fin_core::Rational;
use fin_store::Store;
use serde_json::json;

use emit::Emitter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallMode {
    Freeze,
    Ticks,
    Beats,
    Check,
}

#[derive(Debug, Clone)]
pub struct Args {
    pub cmd: String,
    pub data: PathBuf,
    pub dir: PathBuf,
    pub label: String,
    pub secs: f64,
    pub commit_every_ms: u64,
    pub tick_ms: u64,
    pub clock_every_ms: u64,
    pub stall_at_ms: Option<u64>,
    pub stall_for_ms: u64,
    pub stall_mode: StallMode,
    pub skew_ms: i64,
    pub wall_skew_ms: i64,
    pub start_at_ms: Option<i64>,
    pub free: bool,
    /// Follower: may acquire the lease (default: passive, never acquires).
    pub active: bool,
    pub settle_ms: u64,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let cmd = argv.first().cloned().ok_or("missing subcommand")?;
    let mut a = Args {
        label: cmd.clone(),
        cmd,
        data: PathBuf::new(),
        dir: PathBuf::new(),
        secs: 10.0,
        commit_every_ms: 500,
        tick_ms: 250,
        clock_every_ms: 5000,
        stall_at_ms: None,
        stall_for_ms: 0,
        stall_mode: StallMode::Freeze,
        skew_ms: 0,
        wall_skew_ms: 0,
        start_at_ms: None,
        free: false,
        active: false,
        settle_ms: 3000,
    };
    let mut it = argv[1..].iter();
    while let Some(flag) = it.next() {
        let mut val = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        let num = |v: String| v.parse::<i64>().map_err(|e| format!("{flag}: {e}"));
        match flag.as_str() {
            "--data" => a.data = PathBuf::from(val()?),
            "--dir" => a.dir = PathBuf::from(val()?),
            "--label" => a.label = val()?,
            "--secs" => a.secs = val()?.parse().map_err(|e| format!("--secs: {e}"))?,
            "--commit-every-ms" => a.commit_every_ms = num(val()?)?.max(1) as u64,
            "--tick-ms" => a.tick_ms = num(val()?)?.max(10) as u64,
            "--clock-every-ms" => a.clock_every_ms = num(val()?)?.max(100) as u64,
            "--stall-at-ms" => a.stall_at_ms = Some(num(val()?)?.max(0) as u64),
            "--stall-for-ms" => a.stall_for_ms = num(val()?)?.max(0) as u64,
            "--stall-mode" => {
                a.stall_mode = match val()?.as_str() {
                    "freeze" => StallMode::Freeze,
                    "ticks" => StallMode::Ticks,
                    "beats" => StallMode::Beats,
                    "check" => StallMode::Check,
                    m => return Err(format!("unknown --stall-mode {m}")),
                }
            }
            "--skew-ms" => a.skew_ms = num(val()?)?,
            "--wall-skew-ms" => a.wall_skew_ms = num(val()?)?,
            "--start-at-ms" => a.start_at_ms = Some(num(val()?)?),
            "--settle-ms" => a.settle_ms = num(val()?)?.max(0) as u64,
            "--free" => a.free = true,
            "--active" => a.active = true,
            f => return Err(format!("unknown option {f}")),
        }
    }
    if a.data.as_os_str().is_empty() || a.dir.as_os_str().is_empty() {
        return Err("--data and --dir are required".into());
    }
    Ok(a)
}

/// Waits for `--start-at-ms` (Unix ms, by this host's wall clock with its emulated skew).
fn wait_start(a: &Args, em: &Emitter) {
    if let Some(t) = a.start_at_ms {
        let now_ms = em.wall_ns() / 1_000_000;
        if t > now_ms {
            std::thread::sleep(Duration::from_millis((t - now_ms) as u64));
        }
    }
}

fn create(a: &Args, em: &mut Emitter) -> Result<(), String> {
    let s = Store::create(&a.data, &a.dir, "xhost", Rational::from_int(30), None)
        .map_err(|e| format!("{}: {e}", e.code()))?;
    em.server = Some(s.server_id().to_string());
    let lease = s.lease_state();
    em.emit(
        "created",
        Some(&lease),
        json!({ "projectId": s.project_id(), "storage": s.storage_kind() }),
    );
    s.close().map_err(|e| e.to_string())?;
    if a.free {
        let fin = a.dir.join(".finisher");
        let mut removed = Vec::new();
        for e in std::fs::read_dir(&fin).map_err(|e| e.to_string())? {
            let name = e.map_err(|e| e.to_string())?.file_name();
            let name = name.to_string_lossy().to_string();
            if name == "lease.json"
                || name.starts_with("lease.claim-")
                || name.starts_with("lease.probe-")
            {
                std::fs::remove_file(fin.join(&name)).map_err(|e| format!("{name}: {e}"))?;
                removed.push(name);
            }
        }
        em.emit("freed", None, json!({ "removed": removed }));
    }
    Ok(())
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let a = match parse(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("xhost: {e}");
            eprintln!(
                "usage: xhost create|holder|contender|follower|verify --data S --dir D [options]"
            );
            return ExitCode::from(2);
        }
    };
    let mut em = Emitter::new(&a.label, a.wall_skew_ms);
    wait_start(&a, &em);
    em.emit(
        "start",
        None,
        json!({
            "cmd": a.cmd, "pid": std::process::id(), "os": std::env::consts::OS,
            "monoSource": emit::mono_source(), "version": env!("CARGO_PKG_VERSION"),
            "secs": a.secs, "commitEveryMs": a.commit_every_ms, "skewMs": a.skew_ms,
            "wallSkewMs": a.wall_skew_ms, "stallAtMs": a.stall_at_ms, "stallForMs": a.stall_for_ms,
            "stallMode": format!("{:?}", a.stall_mode).to_lowercase(),
            "countsSleep": fin_store::lease::SystemClock::counts_sleep(),
        }),
    );
    let r = match a.cmd.as_str() {
        "create" => create(&a, &mut em),
        "holder" | "contender" | "follower" => node::run(&a, &mut em),
        "verify" => verify::run(&a, &mut em),
        c => Err(format!("unknown subcommand {c}")),
    };
    match r {
        Ok(()) => {
            em.emit("end", None, json!({ "ok": true }));
            ExitCode::SUCCESS
        }
        Err(e) => {
            em.emit("end", None, json!({ "ok": false, "error": e }));
            ExitCode::FAILURE
        }
    }
}
