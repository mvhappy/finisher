//! Test-only exports (feature `test-hooks`, enabled by the vitest build). Never shipped.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use fin_probe::fixtures::{FixtureOpts, write_fixture_mp4};
use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::engine::{Engine, lock};
use crate::guarded;
use crate::tasks::{Call, StoreTask};

/// Faults an engine's threads pick up.
#[derive(Default)]
pub(crate) struct Hooks {
    tick_panic: AtomicBool,
    long_delay_ms: AtomicU64,
}

impl Hooks {
    /// Whether the next tick should panic (once).
    pub(crate) fn take_tick_panic(&self) -> bool {
        self.tick_panic.swap(false, Ordering::SeqCst)
    }

    /// Sleeps inside the next long call, while it has the store (once).
    pub(crate) fn long_call_delay(&self) {
        let ms = self.long_delay_ms.swap(0, Ordering::SeqCst);
        if ms > 0 {
            std::thread::sleep(Duration::from_millis(ms));
        }
    }
}

#[napi]
impl Engine {
    /// Panics on the thread pool while holding the store, to test panic containment.
    #[napi(catch_unwind)]
    pub fn panic_for_test(&self) -> AsyncTask<StoreTask> {
        self.task(Call::Panic)
    }

    /// The next maintenance tick panics while holding the store.
    #[napi(catch_unwind)]
    pub fn panic_next_tick_for_test(&self) {
        self.inner.hooks.tick_panic.store(true, Ordering::SeqCst);
    }

    /// The next long call holds the store `ms` longer.
    #[napi(catch_unwind)]
    pub fn delay_next_long_call_for_test(&self, ms: u32) {
        self.inner
            .hooks
            .long_delay_ms
            .store(u64::from(ms), Ordering::SeqCst);
    }

    /// Emits a `rewound` event as the maintenance thread would for an import that took `branch`
    /// back to `from_rev` (the store's real rewinds are tested in fin-store).
    #[napi(catch_unwind)]
    pub fn emit_rewound_for_test(&self, branch: String, from_rev: u32) {
        let incarnation = match &*lock(&self.inner.state) {
            crate::engine::State::Open(s) => s.incarnation(),
            _ => String::new(),
        };
        let event = fin_store::StoreEvent::Rewound {
            branch,
            from_rev: u64::from(from_rev),
            quarantined: 1,
            own: false,
            generation: 1,
        };
        self.inner
            .events
            .emit(crate::maintenance::event_json(&event, Some(&incarnation)));
    }

    /// Whether the maintenance thread runs.
    #[napi(catch_unwind)]
    pub fn maintenance_running_for_test(&self) -> bool {
        lock(&self.inner.maintenance)
            .as_ref()
            .is_some_and(|m| m.running())
    }
}

/// Writes a small MP4 (320x180 video at `fps`, optional stereo AAC audio) to `path`.
#[napi(catch_unwind)]
pub fn write_fixture_mp4_for_test(
    path: String,
    seconds: u32,
    fps: u32,
    audio: bool,
    sample_rate: u32,
) -> Result<()> {
    let opts = FixtureOpts {
        seconds,
        fps,
        audio,
        sample_rate,
    };
    guarded(
        || {
            write_fixture_mp4(Path::new(&path), opts);
            Ok(())
        },
        || {},
    )
}
