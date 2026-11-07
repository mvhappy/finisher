//! Test-only exports (feature `test-hooks`, enabled by the vitest build). Never shipped.

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

    /// Pins `fonts_json` (the model's `Fonts`: `{pins: {key: FontPin}, fallback}`) for this
    /// engine as if main's head held them (asset pins resolve through main's assets), until
    /// the model's `font.*` ops exist (Task 18). Triggers what a commit pinning them would.
    #[napi(catch_unwind)]
    pub fn inject_font_pins_for_test(&self, fonts_json: String) -> Result<()> {
        let fonts: fin_core::Fonts = serde_json::from_str(&fonts_json)
            .map_err(|e| crate::reject(fin_core::CoreError::new("INVALID_OP", e.to_string())))?;
        let scanned = crate::engine::with_store(&self.inner, |s| {
            let (_, mut project) = s.snapshot("main").map_err(crate::reject)?;
            project.fonts = fonts;
            Ok(crate::fonts::scan::of_project(&project, s.project_dir()))
        })?;
        self.inner.fonts.inject(scanned);
        Ok(())
    }

    /// Forces two status refreshes to interleave: one computes the summary of the pins now
    /// and pauses (300 ms) before comparing and publishing it while `fonts_json` is injected
    /// as `injectFontPinsForTest` would, whose refresh runs on the jobs thread. The newer
    /// summary must be the last one published.
    #[napi(catch_unwind)]
    pub fn font_publish_race_for_test(&self, fonts_json: String) -> Result<()> {
        let fonts: fin_core::Fonts = serde_json::from_str(&fonts_json)
            .map_err(|e| crate::reject(fin_core::CoreError::new("INVALID_OP", e.to_string())))?;
        let scanned = crate::engine::with_store(&self.inner, |s| {
            let (_, mut project) = s.snapshot("main").map_err(crate::reject)?;
            project.fonts = fonts;
            Ok(crate::fonts::scan::of_project(&project, s.project_dir()))
        })?;
        self.inner.fonts.publish_race(scanned);
        Ok(())
    }

    /// Whether the maintenance thread runs.
    #[napi(catch_unwind)]
    pub fn maintenance_running_for_test(&self) -> bool {
        lock(&self.inner.maintenance)
            .as_ref()
            .is_some_and(|m| m.running())
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
/// the `Measured` JSON. Caption measurement in eval arrives with Task 20.
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
/// (1 s of 48 kHz stereo PCM) or `m4a` (2 s of 48 kHz stereo AAC, no video).
#[napi(catch_unwind)]
pub fn write_fixture_for_test(kind: String, path: String) -> Result<()> {
    let path = Path::new(&path);
    guarded(
        || {
            match kind.as_str() {
                "png" => fixtures::write_png(path, 64, 48),
                "jpeg" => fixtures::write_jpeg(path, 64, 48),
                "webp" => fixtures::write_webp(path, 64, 48),
                "svg" => fixtures::write_svg(path, 640, 360),
                "dotlottie" => fixtures::write_dotlottie(path, &fixtures::sample_lottie(), true),
                "cube" => fixtures::write_cube(path, 4),
                "wav" => fixtures::write_wav(path, 48_000, 2, 48_000),
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
