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
//! - `follower --data S --dir D --secs N [--until-kind K]`: read-only; logs every applied
//!   record, and the heads and hashes (`hashes`) after each import that applied some. With
//!   `--until-kind K` it stops `--settle-ms` after applying a record of kind K, after one
//!   more follow cycle (`followed`);
//! - `brancher`, `merger` (the `history` row, see `history`): `brancher --data S --dir D
//!   --secs N [--edits E] [--plain-ms T] [--branch x]`; `merger --data S --dir D --secs N
//!   [--branch x] [--expect id=seq,…]`;
//! - `upgrade --data S --dir D` (the `upgrade4c` row, see `upgrade`): a 4C server on the
//!   server data a 4B `brancher` left: verify, its heads, an undo of a 4B record, export;
//! - `editor --data S --dir D --secs N [--done-marker]` (the `edit4c` row, see `edit4c`): the
//!   caption and font-pin edit script, then the hashes and frame plans (`plans4c`);
//! - `holder --hold-secs H [--done-marker]`: holds and commits for H s from the moment it may
//!   write, then exports and closes (`edit4c`); `follower --until-done L [--fonts
//!   fetch|offline]`: follows until node L's done marker's heads, then fetches the pinned
//!   fonts itself (`fetch`) or not (`offline`) and reports `plans4c` (`edit4c`);
//! - `verify --data S --dir D`: opens (normally with fresh server data, so the database is
//!   rebuilt from the folder), then reports `Store::verify`, the resolved chain, the
//!   quarantine and the folder's segments and lease files, and the heads and hashes of every
//!   branch (`hashes`, see `hashes`).
//!
//! Common options: `--open-retry-ms R` (retry an open refused with `PROJECT_NOT_FOUND` for R
//! ms, logging each refusal as `openRetry`), `--label L` (default: the subcommand), `--start-at-ms T` (Unix ms: wait
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
mod edit4c;
mod emit;
mod fonts4c;
mod hashes;
mod history;
mod node;
mod ops;
mod upgrade;
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

/// `--fonts`: what a 4C follower does about the pinned fonts (`edit4c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontsMode {
    None,
    Fetch,
    Offline,
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
    /// Follower: stop after applying a record of this kind (`--until-kind`).
    pub until_kind: Option<String>,
    /// Brancher: edits after the branch (`--edits`); edit-only phase before it (`--plain-ms`).
    pub edits: u64,
    pub plain_ms: u64,
    /// Brancher, merger: the branch's display name (`--branch`).
    pub branch: String,
    /// Merger: heads to wait for before merging (`--expect id=seq,…`).
    pub expect: Vec<(String, u64)>,
    /// Retry an open refused with `PROJECT_NOT_FOUND` for this long (`--open-retry-ms`).
    pub open_retry_ms: u64,
    /// Follower: stop once node L's done marker's heads are reached (`--until-done L`).
    pub until_done: Option<String>,
    /// Follower: fetch the pinned fonts itself, or not (`--fonts fetch|offline`).
    pub fonts: FontsMode,
    /// Holder: write for this long from the moment it may (`--hold-secs`).
    pub hold_secs: Option<f64>,
    /// Holder, editor: write the done marker once exported (`--done-marker`).
    pub done_marker: bool,
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
        until_kind: None,
        edits: 12,
        plain_ms: 0,
        branch: "x".into(),
        expect: Vec::new(),
        open_retry_ms: 0,
        until_done: None,
        fonts: FontsMode::None,
        hold_secs: None,
        done_marker: false,
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
            "--until-kind" => a.until_kind = Some(val()?),
            "--edits" => a.edits = num(val()?)?.max(4) as u64,
            "--plain-ms" => a.plain_ms = num(val()?)?.max(0) as u64,
            "--branch" => a.branch = val()?,
            "--expect" => {
                for kv in val()?.split(',').filter(|x| !x.is_empty()) {
                    let (k, v) = kv.split_once('=').ok_or("--expect: id=seq,…")?;
                    a.expect
                        .push((k.to_string(), num(v.to_string())?.max(0) as u64));
                }
            }
            "--open-retry-ms" => a.open_retry_ms = num(val()?)?.max(0) as u64,
            "--until-done" => a.until_done = Some(val()?),
            "--fonts" => {
                a.fonts = match val()?.as_str() {
                    "fetch" => FontsMode::Fetch,
                    "offline" => FontsMode::Offline,
                    m => return Err(format!("unknown --fonts {m}")),
                }
            }
            "--hold-secs" => {
                a.hold_secs = Some(val()?.parse().map_err(|e| format!("--hold-secs: {e}"))?)
            }
            "--done-marker" => a.done_marker = true,
            "--free" => a.free = true,
            "--active" => a.active = true,
            f => return Err(format!("unknown option {f}")),
        }
    }
    if a.fonts != FontsMode::None && a.until_done.is_none() {
        return Err("--fonts needs --until-done".into());
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
                "usage: xhost create|holder|contender|follower|brancher|merger|upgrade|editor|verify --data S --dir D [options]"
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
        "holder" if a.hold_secs.is_some() => edit4c::holder(&a, &mut em),
        "follower" if a.until_done.is_some() => edit4c::follow(&a, &mut em),
        "holder" | "contender" | "follower" => node::run(&a, &mut em),
        "editor" => edit4c::editor(&a, &mut em),
        "upgrade" => upgrade::run(&a, &mut em),
        "brancher" => history::brancher(&a, &mut em),
        "merger" => history::merger(&a, &mut em),
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
