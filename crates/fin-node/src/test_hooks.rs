//! Test-only exports (feature `test-hooks`, enabled by the vitest build). Never shipped.

use std::path::Path;

use fin_probe::fixtures::{FixtureOpts, write_fixture_mp4};
use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::{Call, Engine, StoreTask, guarded};

#[napi]
impl Engine {
    /// Panics on the thread pool while holding the store, to test panic containment.
    #[napi(catch_unwind)]
    pub fn panic_for_test(&self) -> AsyncTask<StoreTask> {
        self.task(Call::Panic)
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
