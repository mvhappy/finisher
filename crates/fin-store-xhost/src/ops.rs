//! Small helpers shared by the nodes: opening the store, the record kind of a committed tx,
//! the asset op of a holder commit.

use std::sync::Arc;
use std::time::{Duration, Instant};

use fin_core::{CommittedTx, CoreError, Op, Rational, VideoInfo};
use fin_store::lease::{MonoClock, SystemClock};
use fin_store::{OpenOptions, Store};
use serde_json::json;

use crate::Args;
use crate::emit::Emitter;

/// The system clock, with the share's time seen `shift_ms` off (`--skew-ms`): every probe
/// mtime this store reads is shifted, as a client-set mtime on a skewed client would be.
#[derive(Debug)]
struct SkewClock {
    shift_ms: i64,
}

impl MonoClock for SkewClock {
    fn now(&self) -> Instant {
        SystemClock.now()
    }
    fn server_shift_ms(&self) -> i64 {
        self.shift_ms
    }
}

/// Opens the store of `a` (with `--skew-ms`, through [`SkewClock`]).
pub fn open_store(a: &Args, em: &Emitter) -> Result<Store, String> {
    let opts = || {
        let mut o = OpenOptions::default();
        if a.skew_ms != 0 {
            o.clock = Some(Arc::new(SkewClock {
                shift_ms: a.skew_ms,
            }) as Arc<dyn MonoClock>);
        }
        o
    };
    let t0 = Instant::now();
    // `--open-retry-ms`: a folder another host has just made can be invisible here for a while
    // (a directory cache); each refused open is logged (`openRetry`).
    let mut tries = 0u32;
    let store = loop {
        match Store::open_with(&a.data, &a.dir, opts()) {
            Ok(s) => break s,
            Err(e) => {
                let e: CoreError = e.into();
                let waited = t0.elapsed().as_millis() as u64;
                if e.code != "PROJECT_NOT_FOUND" || waited >= a.open_retry_ms {
                    return Err(format!("open: {}: {}", e.code, e.message));
                }
                tries += 1;
                em.emit(
                    "openRetry",
                    None,
                    json!({ "code": e.code, "try": tries, "waitedMs": waited }),
                );
                std::thread::sleep(Duration::from_millis(1000));
            }
        }
    };
    Ok(store)
}

/// The record kind of `c` (`edit`, `branch`, `merge`, …).
pub fn kind_of(c: &CommittedTx) -> String {
    c.link
        .as_deref()
        .map_or_else(|| "edit".to_string(), |l| fin_core::kind_name(l.kind()))
}

/// A small, unique asset: the op every holder commit carries.
pub fn asset_op(tx_id: &str) -> Op {
    Op::AssetAdd {
        id: "$a".into(),
        path: format!("/xhost/{tx_id}.mov"),
        duration: Rational::from_int(10),
        video: Some(VideoInfo {
            width: 1920,
            height: 1080,
            rate: Rational::from_int(30),
        }),
        audio: None,
        fingerprint: format!("fp-{tx_id}"),
    }
}
