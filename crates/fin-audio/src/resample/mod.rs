//! The scalar polyphase windowed-sinc resampler (spec §8.1, plan 6C Decision 5).
//!
//! The rate ratio is reduced by its gcd to `L / M` (output / input). Output sample `k` is the
//! source instant `t = k·M / L` input samples, filtered by the prototype of [`kernel`] centred
//! there, so the filter's group delay is removed: output sample 0 is source time 0, and a stream
//! of `n` input samples yields exactly [`resampled_len`]`(n, from, to)` outputs. Inputs before
//! the first sample and after the last are zeros. Streaming: `process` accepts any chunk sizes
//! and `finish` flushes the tail; the output is bit-identical whatever the chunking, since every
//! output is the same fixed-order `f32` dot product over the same samples.
//!
//! **Limits.** Both rates must be 1 Hz to 768 kHz, and downsampling at most 48:1 (so 705.6
//! and 768 kHz sources still reach the 16 kHz feed); upsampling is unbounded within the rates.
//! The tables hold about `140 × max(1, from / to)` taps a phase for at most 1,025 phases, so
//! the limits keep a resampler under about 30 MB. Anything else is `MEDIA_LIMIT` with
//! `details {limit: "sampleRate" | "downsampleRatio", value, max}`.

mod kernel;

use fin_core::CoreError;
use kernel::{Kernel, PHASES, dot};
use serde_json::json;

/// Rates accepted, in Hz.
const MAX_RATE: u32 = 768_000;
/// The largest downsampling ratio accepted (the taps grow with it: 140 × ratio a phase).
const MAX_DOWN: u32 = 48;

/// The number of outputs for `n_in` inputs: `ceil(n_in × to / from)` (saturating; 0 for a
/// zero `from_hz`).
pub fn resampled_len(n_in: u64, from_hz: u32, to_hz: u32) -> u64 {
    if from_hz == 0 {
        return 0;
    }
    let n = (u128::from(n_in) * u128::from(to_hz)).div_ceil(u128::from(from_hz));
    u64::try_from(n).unwrap_or(u64::MAX)
}

fn limit(name: &str, value: f64, max: f64, message: String) -> CoreError {
    CoreError::new("MEDIA_LIMIT", message)
        .with_details(json!({ "limit": name, "value": value, "max": max }))
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// A streaming mono resampler from `from_hz` to `to_hz`.
pub struct Resampler {
    from_hz: u32,
    to_hz: u32,
    l: u64,
    m: u64,
    /// `None` for the identity ratio (a copy).
    kernel: Option<Kernel>,
    /// Input history: `buf[0]` is input sample `base` (negative indices are the leading zeros).
    buf: Vec<f32>,
    base: i64,
    /// Real input samples received.
    received: u64,
    /// Outputs written.
    produced: u64,
    /// The next output's source instant: `i + r / L` input samples.
    i: u64,
    r: u64,
}

impl Resampler {
    /// A resampler for `from_hz → to_hz`; the filter tables are designed here, once.
    /// `MEDIA_LIMIT` outside the limits in the module documentation.
    pub fn new(from_hz: u32, to_hz: u32) -> Result<Resampler, CoreError> {
        for rate in [from_hz, to_hz] {
            if !(1..=MAX_RATE).contains(&rate) {
                return Err(limit(
                    "sampleRate",
                    f64::from(rate),
                    f64::from(MAX_RATE),
                    format!("the sample rate {rate} Hz is outside 1 to {MAX_RATE} Hz"),
                ));
            }
        }
        if u64::from(from_hz) > u64::from(MAX_DOWN) * u64::from(to_hz) {
            return Err(limit(
                "downsampleRatio",
                f64::from(from_hz) / f64::from(to_hz),
                f64::from(MAX_DOWN),
                format!("resampling {from_hz} Hz to {to_hz} Hz is more than {MAX_DOWN}:1 down"),
            ));
        }
        let g = gcd(u64::from(from_hz), u64::from(to_hz));
        let (l, m) = (u64::from(to_hz) / g, u64::from(from_hz) / g);
        let kernel = (l != 1 || m != 1).then(|| Kernel::design(l, m));
        let mut r = Resampler {
            from_hz,
            to_hz,
            l,
            m,
            kernel,
            buf: Vec::new(),
            base: 0,
            received: 0,
            produced: 0,
            i: 0,
            r: 0,
        };
        r.reset();
        Ok(r)
    }

    /// Back to the start of a new stream.
    fn reset(&mut self) {
        let lead = self.half().saturating_sub(1) as usize;
        self.buf.clear();
        self.buf.resize(lead, 0.0);
        self.base = -(lead as i64);
        self.received = 0;
        self.produced = 0;
        self.i = 0;
        self.r = 0;
    }

    /// Feeds `input` and appends every output whose taps it completes.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.kernel.is_none() {
            out.extend_from_slice(input);
            return;
        }
        self.buf.extend_from_slice(input);
        self.received += input.len() as u64;
        // An output reads inputs up to `i + half`.
        while self.i + self.half() < self.received {
            out.push(self.next());
        }
        self.compact();
    }

    /// Ends the stream: appends the remaining outputs (zeros past the last input), so the
    /// stream's total is `resampled_len(n, from, to)`, then resets for a new stream.
    pub fn finish(&mut self, out: &mut Vec<f32>) {
        if self.kernel.is_some() {
            let total = resampled_len(self.received, self.from_hz, self.to_hz);
            // The last output's source instant is before `received`, so `half` zeros suffice.
            self.buf.resize(self.buf.len() + self.half() as usize, 0.0);
            while self.produced < total {
                out.push(self.next());
            }
        }
        self.reset();
    }

    fn half(&self) -> u64 {
        self.kernel.as_ref().map_or(0, |k| k.half as u64)
    }

    /// The output at the current instant; advances by `M / L` input samples.
    fn next(&mut self) -> f32 {
        let Some(k) = self.kernel.as_ref() else {
            unreachable!("the identity ratio copies");
        };
        let first = self.i as i64 - (k.half as i64 - 1);
        let start = (first - self.base) as usize;
        let x = &self.buf[start..start + 2 * k.half];
        let y = if k.phases == self.l {
            dot(&k.tables[self.r as usize], x)
        } else {
            // `L` above the table resolution: the phase quantised to 1/1024, interpolated
            // linearly between the two adjacent tables.
            let pos = self.r * PHASES;
            let (q, rem) = ((pos / self.l) as usize, pos % self.l);
            let a = (rem as f64 / self.l as f64) as f32;
            let y0 = dot(&k.tables[q], x);
            let y1 = dot(&k.tables[q + 1], x);
            y0 + a * (y1 - y0)
        };
        self.r += self.m;
        self.i += self.r / self.l;
        self.r %= self.l;
        self.produced += 1;
        y
    }

    /// Drops the history no later output reads.
    fn compact(&mut self) {
        let keep_from = self.i as i64 - (self.half() as i64 - 1);
        let dead = ((keep_from - self.base).max(0) as usize).min(self.buf.len());
        if dead > 0 {
            self.buf.drain(..dead);
            self.base += dead as i64;
        }
    }
}
