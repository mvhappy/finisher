//! The audio engine (sub-project 6 spec §8, plan 6C): rational time to sample index, planar
//! stereo buffers and, in later tasks, the mixer, resampler, time-stretch, effects and loudness.
//! No I/O, no clocks, no randomness; builds for `wasm32-unknown-unknown`, and every hashed
//! output is bit-identical native and on wasm32 (`scripts/check-eval-floats.mjs` holds the float
//! rules, `libm::` for transcendentals).

#![forbid(unsafe_code)]

pub mod buf;
pub mod channels;
pub mod effects;
pub mod loudness;
pub mod mix;
pub mod peaks;
pub mod resample;
pub mod stretch;
#[cfg(feature = "testing")]
pub mod testing;
pub mod time;

pub use buf::Stereo;
pub use loudness::{GainDecision, Limiter, Measured, Meter, normalise};
pub use mix::{Mixer, PcmSource, SourceLayout};
pub use peaks::{Peaks, peaks};
pub use resample::{Resampler, resampled_len};
pub use stretch::Wsola;
pub use time::sample_at;

/// The engine's sample rate: every mixed or cached feed is 48 kHz.
pub const RATE: u32 = 48_000;

/// Bumped when any hashed audio output changes (the A0 golden hashes are re-blessed with it).
/// `fin_media::AUDIO_VERSION` is bumped with it (the build fails while they differ): audio cache
/// keys carry that copy on hosts that never link this crate (6C plan Decision 7).
pub const AUDIO_VERSION: u32 = 1;
const _: () = assert!(
    AUDIO_VERSION == fin_media::AUDIO_VERSION,
    "bump fin_media::AUDIO_VERSION with fin_audio::AUDIO_VERSION"
);
