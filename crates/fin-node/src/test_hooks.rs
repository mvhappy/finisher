//! Test-only exports (feature `test-hooks`, enabled by the vitest build). Never shipped.

// napi-derive registers a free `#[napi] fn` only under `#[cfg(not(test))]`, so in the cfg(test)
// build that `cargo clippy --all-targets` compiles (despite `test = false`) the exported
// functions below have no caller. They are live in the real cdylib build; allow it for tests only.
#![cfg_attr(test, allow(dead_code))]

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use fin_core::CoreError;
use fin_probe::fixtures::{self, FixtureOpts, write_fixture_mp4};
use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::engine::{Engine, lock};
use crate::tasks::{Call, StoreTask};
use crate::{guarded, reject};

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

    /// Automatic work starts once the engine has been quiet for `ms` (instead of 30 s).
    #[napi(catch_unwind)]
    pub fn set_auto_idle_for_test(&self, ms: u32) {
        self.inner.auto.set_idle_for_test(u64::from(ms));
    }

    /// Each automatic backup from now on waits `ms` between its copy and its checks (cut short
    /// when cancelled).
    #[napi(catch_unwind)]
    pub fn pause_auto_backup_for_test(&self, ms: u32) {
        self.inner.auto.set_backup_pause_for_test(u64::from(ms));
    }

    /// Whether an automatic backup is running.
    #[napi(catch_unwind)]
    pub fn auto_backup_running_for_test(&self) -> bool {
        self.inner.auto.backup_running()
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

    /// Forces two status refreshes to interleave: one computes the summary of the pins now
    /// and pauses (300 ms) before comparing and publishing it while `tx_json` is committed
    /// (as `Engine.commit` would, rescanning the pins), whose refresh runs on the jobs thread.
    /// The newer summary must be the last one published. Rejects with the commit's error.
    #[napi(catch_unwind)]
    pub fn font_publish_race_for_test(&self, tx_json: String) -> Result<()> {
        let tx: fin_core::Tx = serde_json::from_str(&tx_json)
            .map_err(|e| crate::reject(fin_core::CoreError::new("INVALID_TX", e.to_string())))?;
        let inner = self.inner.clone();
        let mut outcome = Ok(());
        self.inner.fonts.publish_race(|| {
            outcome = crate::engine::with_store(&inner, |s| {
                let c = s.commit(tx).map_err(crate::reject)?;
                inner.fonts.committed(s, &c);
                Ok(())
            });
        });
        outcome
    }

    /// The next tick stops in its folder I/O, outside the store mutex: `import` (after the
    /// scan, before the apply), `export` (a segment encoded, before its write) or a point in
    /// the automatic compaction step (`compactList`, `compactRead`, `compactWrite`,
    /// `compactDelete`), until `releaseTickForTest`.
    #[napi(catch_unwind)]
    pub fn hang_next_tick_for_test(&self, point: String) -> Result<()> {
        let point = tick_point(&point)?;
        self.tick_hooks()?.hang(point);
        Ok(())
    }

    /// The next tick panics in its folder I/O, outside the store mutex (a point as for
    /// `hangNextTickForTest`).
    #[napi(catch_unwind)]
    pub fn panic_next_tick_io_for_test(&self, point: String) -> Result<()> {
        let point = tick_point(&point)?;
        self.tick_hooks()?.panic_at(point);
        Ok(())
    }

    /// Whether a tick is stopped at the point `hangNextTickForTest` armed.
    #[napi(catch_unwind)]
    pub fn tick_hung_for_test(&self) -> Result<bool> {
        Ok(self.tick_hooks()?.is_hung())
    }

    /// Lets the stopped tick go on.
    #[napi(catch_unwind)]
    pub fn release_tick_for_test(&self) -> Result<()> {
        self.tick_hooks()?.release();
        Ok(())
    }

    /// The grace period between compaction's phases is `ms` (instead of 10 min or 24 h).
    #[napi(catch_unwind)]
    pub fn set_compaction_grace_for_test(&self, ms: u32) -> Result<()> {
        match &mut *lock(&self.inner.state) {
            crate::engine::State::Open(s) => {
                s.set_compaction_grace_for_test(Duration::from_millis(u64::from(ms)));
                Ok(())
            }
            other => Err(reject(other.unavailable())),
        }
    }

    /// Whether the maintenance thread runs.
    #[napi(catch_unwind)]
    pub fn maintenance_running_for_test(&self) -> bool {
        lock(&self.inner.maintenance)
            .as_ref()
            .is_some_and(|m| m.running())
    }
}

/// `import`, `export`, or a compaction step's `compactList`, `compactRead`, `compactWrite`,
/// `compactDelete` (`fin_store::TickPoint`).
fn tick_point(point: &str) -> Result<fin_store::TickPoint> {
    match point {
        "import" => Ok(fin_store::TickPoint::ImportRead),
        "export" => Ok(fin_store::TickPoint::ExportWrite),
        "compactList" => Ok(fin_store::TickPoint::CompactList),
        "compactRead" => Ok(fin_store::TickPoint::CompactRead),
        "compactWrite" => Ok(fin_store::TickPoint::CompactWrite),
        "compactDelete" => Ok(fin_store::TickPoint::CompactDelete),
        other => Err(reject(CoreError::new(
            "INVALID_OP",
            format!("no tick point {other:?}"),
        ))),
    }
}

impl Engine {
    /// The store's tick hooks (the store is never held across a tick's folder I/O).
    fn tick_hooks(&self) -> Result<fin_store::TickHooks> {
        match &*lock(&self.inner.state) {
            crate::engine::State::Open(s) => Ok(s.tick_hooks()),
            other => Err(reject(other.unavailable())),
        }
    }
}

/// A generated test font (`fin-testfonts`): `sans` (Latin + Cyrillic), `rtl` (Hebrew), `han`,
/// `latinBold`, or `big:<bytes>` (a font padded to that size; `big:<bytes>:<seed>` varies it).
#[napi(catch_unwind)]
pub fn test_font_for_test(name: String) -> Result<Buffer> {
    let mut parts = name.split(':');
    let bytes = match (parts.next(), parts.next(), parts.next()) {
        (Some("sans"), None, _) => fin_testfonts::test_sans(),
        (Some("rtl"), None, _) => fin_testfonts::test_rtl(),
        (Some("han"), None, _) => fin_testfonts::test_han(),
        (Some("latinBold"), None, _) => fin_testfonts::build(&fin_testfonts::LATIN_BOLD),
        (Some("big"), Some(n), seed) => {
            let size: usize = n.parse().map_err(|_| Error::from_reason("bad size"))?;
            let mut b = fin_testfonts::test_big(size);
            if let Some(seed) = seed.and_then(|s| s.parse::<u8>().ok()) {
                let last = b.len() - 1;
                b[last] ^= seed;
            }
            b
        }
        _ => return Err(Error::from_reason(format!("unknown test font {name}"))),
    };
    Ok(bytes.into())
}

/// Measures a `TextRequest` (JSON) with the shared measurer of `server_data`; resolves to
/// the `Measured` JSON.
#[napi(catch_unwind)]
pub fn measure_for_test(server_data: String, request_json: String) -> Result<String> {
    let req: fin_core::TextRequest = serde_json::from_str(&request_json)
        .map_err(|e| crate::reject(fin_core::CoreError::new("INVALID_OP", e.to_string())))?;
    let m = crate::fonts::for_server(Path::new(&server_data)).measurer();
    crate::encode(&m.measure(&req).map_err(crate::reject)?)
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

/// Writes a small fixture of `kind` to `path`: `png`, `jpeg` and `webp` (64x48), `svg`
/// (640x360), `dotlottie` (the fixtures' sample template), `cube` (an identity 4³ LUT), `wav`
/// (1 s of 48 kHz stereo PCM; `wav:<seconds>` for longer), `m4a` (2 s of 48 kHz stereo AAC,
/// no video) or `mp4-r90` (a portrait clip: 320x180 coded, rotated 90°).
#[napi(catch_unwind)]
pub fn write_fixture_for_test(kind: String, path: String) -> Result<()> {
    let path = Path::new(&path);
    guarded(
        || {
            let wav_seconds = kind
                .strip_prefix("wav:")
                .and_then(|n| n.parse::<u32>().ok());
            match kind.as_str() {
                _ if wav_seconds.is_some_and(|s| (1..=600).contains(&s)) => {
                    let seconds = wav_seconds.unwrap_or(1);
                    fixtures::write_wav(path, 48_000, 2, 48_000 * seconds);
                }
                "png" => fixtures::write_png(path, 64, 48),
                "jpeg" => fixtures::write_jpeg(path, 64, 48),
                "webp" => fixtures::write_webp(path, 64, 48),
                "svg" => fixtures::write_svg(path, 640, 360),
                "dotlottie" => fixtures::write_dotlottie(path, &fixtures::sample_lottie(), true),
                "cube" => fixtures::write_cube(path, 4),
                "wav" => fixtures::write_wav(path, 48_000, 2, 48_000),
                // A portrait phone clip: 2 s of 320x180 at 25 fps, a 90° `tkhd` matrix.
                "mp4-r90" => fixtures::write_fixture_mp4_rotated(
                    path,
                    FixtureOpts::default(),
                    fin_media::Rotation::R90,
                ),
                "m4a" => fixtures::write_m4a(
                    path,
                    FixtureOpts {
                        seconds: 2,
                        fps: 25,
                        audio: true,
                        sample_rate: 48_000,
                    },
                ),
                other => {
                    return Err(reject(CoreError::new(
                        "INTERNAL",
                        format!("no fixture kind {other:?}"),
                    )));
                }
            }
            Ok(())
        },
        || {},
    )
}

/// Packs the unpacked plugin in `src` into `out` (`fin-registry`'s reproducible packer, the
/// manifest's executables marked); resolves to the archive's SHA-256.
#[allow(clippy::disallowed_methods)] // a test plugin source directory (test-hooks builds only)
#[napi(catch_unwind)]
pub fn pack_plugin_for_test(src: String, out: String) -> Result<String> {
    let src = Path::new(&src);
    let executables = std::fs::read(src.join(fin_plugin_api::MANIFEST_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<fin_plugin_api::Manifest>(&b).ok())
        .map(|m| m.executables())
        .unwrap_or_default();
    fin_registry::package::pack(src, &executables, Path::new(&out))
        .map(|info| info.sha256)
        .map_err(|e| crate::reject(fin_core::CoreError::new("INVALID_OP", e.to_string())))
}

/// The paths process `pid` holds open or mapped, on macOS (`proc_pidinfo`; the warm workers'
/// held-files check, 5C-1 Task 11 review L3). Throws `IO_ERROR` when they cannot be read, and
/// on other systems.
#[napi(catch_unwind)]
pub fn held_paths_for_test(pid: u32) -> Result<Vec<String>> {
    fin_jobhost::held_paths::held_paths(pid).map_err(|e| {
        reject(CoreError::new(
            "IO_ERROR",
            format!("the open files of process {pid}: {e}"),
        ))
    })
}
