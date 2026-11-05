//! JSONL events on stdout, each stamped with this host's monotonic and wall clocks.
//!
//! Every line: `{"ev", "mono", "wall", "label", "server", "epoch", "lease", ...fields}`.
//! - `mono`: nanoseconds of this host's monotonic clock counting sleep (`CLOCK_BOOTTIME` on
//!   Linux, `CLOCK_MONOTONIC` on macOS, `QueryInterruptTimePrecise` on Windows). Comparable
//!   between processes on one host only.
//! - `wall`: Unix nanoseconds of the wall clock, plus `--wall-skew-ms` (emulated skew).
//! - `epoch`, `lease`: the lease as this store sees it, where one is open.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use fin_store::lease::LeaseState;
use serde_json::{Map, Value, json};

/// Nanoseconds of the host's monotonic clock (counting sleep); 0 if unreadable.
#[cfg(unix)]
pub fn mono_ns() -> u64 {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let id = libc::CLOCK_BOOTTIME;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let id = libc::CLOCK_MONOTONIC;
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec for the duration of the call.
    if unsafe { libc::clock_gettime(id, &mut ts) } != 0 {
        return 0;
    }
    let s = u64::try_from(ts.tv_sec).unwrap_or(0);
    let n = u64::try_from(ts.tv_nsec).unwrap_or(0);
    s.saturating_mul(1_000_000_000).saturating_add(n)
}

#[cfg(windows)]
pub fn mono_ns() -> u64 {
    let mut t = 0u64;
    // SAFETY: `t` is a valid, writable u64 for the duration of the call.
    unsafe { windows_sys::Win32::System::WindowsProgramming::QueryInterruptTimePrecise(&mut t) };
    t.saturating_mul(100)
}

#[cfg(not(any(unix, windows)))]
pub fn mono_ns() -> u64 {
    0
}

/// The name of the monotonic source, for the `start` event.
pub fn mono_source() -> &'static str {
    if cfg!(any(target_os = "linux", target_os = "android")) {
        "CLOCK_BOOTTIME"
    } else if cfg!(windows) {
        "QueryInterruptTimePrecise"
    } else if cfg!(unix) {
        "CLOCK_MONOTONIC"
    } else {
        "none"
    }
}

fn unix_ns(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_nanos()).unwrap_or(i64::MAX),
    }
}

/// `(state, epoch)` of a lease state, for the common fields.
pub fn lease_parts(s: &LeaseState) -> (&'static str, Option<u64>) {
    match s {
        LeaseState::Held { epoch } => ("held", Some(*epoch)),
        LeaseState::HeldElsewhere { epoch, .. } => ("heldElsewhere", Some(*epoch)),
        LeaseState::Stale { epoch, .. } => ("stale", Some(*epoch)),
        LeaseState::Lost { .. } => ("lost", None),
    }
}

pub struct Emitter {
    label: String,
    wall_skew_ns: i64,
    pub server: Option<String>,
    clock_seq: u64,
}

impl Emitter {
    pub fn new(label: &str, wall_skew_ms: i64) -> Emitter {
        Emitter {
            label: label.to_string(),
            wall_skew_ns: wall_skew_ms.saturating_mul(1_000_000),
            server: None,
            clock_seq: 0,
        }
    }

    /// This host's wall clock in Unix ns, with the emulated skew.
    pub fn wall_ns(&self) -> i64 {
        unix_ns(SystemTime::now()).saturating_add(self.wall_skew_ns)
    }

    /// One event. `fields` must be a JSON object (anything else is put under `data`).
    pub fn emit(&self, ev: &str, lease: Option<&LeaseState>, fields: Value) {
        let mut m = Map::new();
        m.insert("ev".into(), json!(ev));
        m.insert("mono".into(), json!(mono_ns()));
        m.insert("wall".into(), json!(self.wall_ns()));
        m.insert("label".into(), json!(self.label));
        m.insert("server".into(), json!(self.server));
        if let Some(l) = lease {
            let (state, epoch) = lease_parts(l);
            m.insert("lease".into(), json!(state));
            m.insert("epoch".into(), json!(epoch));
        }
        match fields {
            Value::Object(f) => m.extend(f),
            Value::Null => {}
            other => {
                m.insert("data".into(), other);
            }
        }
        let mut out = io::stdout().lock();
        let _ = serde_json::to_writer(&mut out, &Value::Object(m));
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }

    /// Measures this host's wall clock against the share's: rewrites
    /// `<dir>/xhost-clock-<label>`, syncs it, and reads its mtime through a new handle.
    /// `offsetNs` = mtime − midpoint of the wall clock around the write; `rttNs` bounds the
    /// error (plus the server's mtime granularity, and any delay in setting it).
    pub fn clock_sample(&mut self, dir: &Path, lease: Option<&LeaseState>) {
        self.clock_seq += 1;
        let path = dir.join(format!("xhost-clock-{}", self.label));
        let before = self.wall_ns();
        let m0 = mono_ns();
        let r = (|| -> io::Result<SystemTime> {
            let mut f = File::create(&path)?;
            f.write_all(format!("{} {}\n", self.label, self.clock_seq).as_bytes())?;
            f.sync_all()?;
            drop(f);
            File::open(&path)?.metadata()?.modified()
        })();
        let after = self.wall_ns();
        let rtt = mono_ns().saturating_sub(m0);
        match r {
            Ok(mtime) => {
                let mid = before + (after - before) / 2;
                let mtime = unix_ns(mtime);
                self.emit(
                    "clock",
                    lease,
                    json!({ "mtimeNs": mtime, "offsetNs": mtime - mid, "rttNs": rtt }),
                );
            }
            Err(e) => self.emit("clock", lease, json!({ "error": e.to_string() })),
        }
    }
}
