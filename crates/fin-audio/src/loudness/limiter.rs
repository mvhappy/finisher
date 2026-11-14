//! The look-ahead true-peak limiter (plan 6C Decision 18, spec §8.6). A0: scalar `f32` in a
//! fixed order, so its output is bit-identical native and on wasm32.
//!
//! **Detection.** The signal is oversampled 4× by the resampler's windowed-sinc kernel at
//! ratio 4/1 (instants `j + p/4`, `p = 0..4`). For each input frame `j`, the peak is the largest
//! magnitude, over both channels (the gain is linked), of the frame itself, its four instants
//! and, at each instant that is a local maximum of the magnitude, the vertex of the parabola
//! through it and its two neighbours (instants of frames `j − 1` and `j + 1` included): a 4×
//! grid alone can miss up to 0.7 dB of a crest near Nyquist, the vertex recovers nearly all of
//! it. Detection therefore reads up to `REACH` frames ahead. The frame's required gain `r[j]`
//! is `ceiling / peak`, or 1 at or below the ceiling.
//!
//! **Gain.** `m[n]` is the minimum of `r` over the look-ahead window `[n, n + LOOKAHEAD)`, then
//! released: `m[n] = min(m_raw[n], m[n−1] + (1 − m[n−1])·k)`, a one-pole return towards 1 with
//! a 50 ms time constant. The applied gain is the mean of the last `LOOKAHEAD` values of `m`, so
//! a reduction ramps in linearly over the look-ahead and is complete when the peak arrives:
//! every `m[n−i]` averaged at frame `n` covers `n`, so the gain at `n` never exceeds `r[n]`.
//! The pipeline starts `LOOKAHEAD − 1` frames before the stream (frames that are not output,
//! with `r = 1`), so the first frames ramp in like any others. Below the ceiling every term is
//! exactly 1 and the output is the input, bit for bit.
//!
//! **Latency.** Output frame `n` needs input up to `n + LOOKAHEAD − 1 + REACH`, so the limiter
//! holds [`Limiter::LATENCY`] frames: `process` hands back only the frames that are ready (the
//! first call returns that many fewer), and `finish` appends the rest, so the stream comes out
//! aligned with and as long as the input.

use std::collections::VecDeque;

use crate::Stereo;
use crate::resample::kernel::{Kernel, dot};

/// The look-ahead: 5 ms at 48 kHz.
const LOOKAHEAD: usize = 240;
/// The oversampling factor of the peak detector.
const OVERSAMPLE: u64 = 4;
/// The 4/1 kernel's half-length in input frames (the resampler's 70 at `fs_min`).
const HALF: usize = 70;
/// How far ahead of frame `j` its detection reads: frame `j + 1`'s first instant.
const REACH: usize = HALF + 1;
/// The release time constant: 50 ms at 48 kHz, in frames.
const RELEASE_FRAMES: f64 = 2_400.0;

/// A streaming stereo true-peak limiter.
pub struct Limiter {
    ceiling: f32,
    /// `1 − exp(−1 / RELEASE_FRAMES)`.
    release: f32,
    kernel: Kernel,
    /// Input history, both channels: `l[0]` is input frame `base` (negative frames are zeros).
    l: Vec<f32>,
    r: Vec<f32>,
    base: i64,
    /// Real input frames received.
    received: u64,
    /// The next frame whose required gain is computed.
    next_r: u64,
    /// The sliding minimum of `r`: `(frame, gain)`, gains increasing from the front.
    window: VecDeque<(u64, f32)>,
    /// The next frame through the gain pipeline (negative: the lead-in, not output).
    next_out: i64,
    /// The last `LOOKAHEAD` released minima, oldest first.
    held: VecDeque<f32>,
    /// Oversampled frames `os_base ..`: `[channel][p]` is the instant `frame + p/4`.
    os: VecDeque<[[f32; 4]; 2]>,
    os_base: i64,
}

impl Limiter {
    /// Frames held back: the look-ahead plus the detector's reach.
    pub const LATENCY: usize = LOOKAHEAD - 1 + REACH;

    /// A limiter holding the true peak at `ceiling_dbtp`.
    pub fn new(ceiling_dbtp: f64) -> Limiter {
        let kernel = Kernel::design(OVERSAMPLE, 1);
        debug_assert_eq!(kernel.half, HALF);
        let mut lim = Limiter {
            ceiling: libm::pow(10.0, ceiling_dbtp / 20.0) as f32,
            release: (1.0 - libm::exp(-1.0 / RELEASE_FRAMES)) as f32,
            kernel,
            l: Vec::new(),
            r: Vec::new(),
            base: 0,
            received: 0,
            next_r: 0,
            window: VecDeque::new(),
            next_out: 0,
            held: VecDeque::new(),
            os: VecDeque::new(),
            os_base: 0,
        };
        lim.reset();
        lim
    }

    fn reset(&mut self) {
        // Frame −1's instants read back to frame −HALF.
        let lead = HALF;
        for c in [&mut self.l, &mut self.r] {
            c.clear();
            c.resize(lead, 0.0);
        }
        self.base = -(lead as i64);
        self.received = 0;
        self.next_r = 0;
        self.window.clear();
        self.next_out = 1 - LOOKAHEAD as i64;
        self.held.clear();
        self.held.resize(LOOKAHEAD, 1.0);
        self.os.clear();
        self.os_base = -1;
    }

    /// Feeds `block` and replaces its contents with the output frames now ready (input frames
    /// `LATENCY` behind the newest, so fewer than given at the start of a stream).
    pub fn process(&mut self, block: &mut Stereo) {
        self.l.extend_from_slice(&block.l);
        self.r.extend_from_slice(&block.r);
        self.received += block.len() as u64;
        block.clear();
        self.run(self.received, block);
    }

    /// Ends the stream: appends the held frames to `out` (the input is zeros past its end),
    /// then resets for a new stream.
    pub fn finish(&mut self, out: &mut Stereo) {
        let pad = Self::LATENCY;
        self.l.resize(self.l.len() + pad, 0.0);
        self.r.resize(self.r.len() + pad, 0.0);
        self.run(self.received + pad as u64, out);
        self.reset();
    }

    /// Pushes every output frame (up to the real input's end) whose look-ahead the `avail`
    /// buffered frames (real or padding) cover. The required gains are computed one frame
    /// ahead of need, so the sliding minimum never sees past `n + LOOKAHEAD − 1` and the output
    /// does not depend on the block sizes.
    fn run(&mut self, avail: u64, out: &mut Stereo) {
        'out: while self.next_out < self.received as i64 {
            let n = self.next_out;
            // Output `n` needs the required gains up to `n + LOOKAHEAD − 1`; gain `j` reads
            // input up to `j + REACH`.
            while (self.next_r as i64) < n + LOOKAHEAD as i64 {
                if self.next_r + REACH as u64 >= avail {
                    break 'out;
                }
                let j = self.next_r;
                let g = self.required(j as i64);
                while self.window.back().is_some_and(|&(_, v)| v >= g) {
                    self.window.pop_back();
                }
                self.window.push_back((j, g));
                self.next_r += 1;
            }
            while self.window.front().is_some_and(|&(j, _)| (j as i64) < n) {
                self.window.pop_front();
            }
            let m_raw = self.window.front().map_or(1.0, |&(_, v)| v);
            let prev = *self.held.back().unwrap_or(&1.0);
            let m = m_raw.min(prev + (1.0 - prev) * self.release);
            self.held.pop_front();
            self.held.push_back(m);
            let mut sum = 0.0f32;
            for &v in &self.held {
                sum += v;
            }
            let gain = sum / LOOKAHEAD as f32;
            self.next_out += 1;
            if n < 0 {
                continue;
            }
            let at = (n - self.base) as usize;
            let (xl, xr) = (self.l[at], self.r[at]);
            if gain == 1.0 {
                out.push(xl, xr);
            } else {
                out.push(xl * gain, xr * gain);
            }
        }
        self.compact();
    }

    /// Frame `f`'s four oversampled instants, both channels (reads frames `f − HALF + 1` to
    /// `f + HALF`).
    fn oversample(&self, f: i64) -> [[f32; 4]; 2] {
        let first = (f - (HALF as i64 - 1) - self.base) as usize;
        let mut out = [[0.0f32; 4]; 2];
        for (o, ch) in out.iter_mut().zip([&self.l, &self.r]) {
            let x = &ch[first..first + 2 * HALF];
            for (v, table) in o.iter_mut().zip(&self.kernel.tables) {
                *v = dot(table, x);
            }
        }
        out
    }

    /// The gain frame `j` needs: `ceiling / peak`, at most 1 (see the module documentation).
    fn required(&mut self, j: i64) -> f32 {
        while self.os_base < j - 1 {
            self.os.pop_front();
            self.os_base += 1;
        }
        while self.os_base + (self.os.len() as i64) < j + 2 {
            let f = self.os_base + self.os.len() as i64;
            let o = self.oversample(f);
            self.os.push_back(o);
        }
        let at = (j - self.base) as usize;
        let mut peak = self.l[at].abs().max(self.r[at].abs());
        for c in 0..2 {
            let (prev, cur, next) = (self.os[0][c], self.os[1][c], self.os[2][c]);
            let s = [prev[3], cur[0], cur[1], cur[2], cur[3], next[0]];
            for k in 1..5 {
                peak = peak.max(crest(s[k - 1], s[k], s[k + 1]));
            }
        }
        if peak > self.ceiling {
            self.ceiling / peak
        } else {
            1.0
        }
    }

    /// Drops the input no later output or detection reads.
    fn compact(&mut self) {
        let keep = self.next_out.min(self.next_r as i64 - HALF as i64);
        let dead = ((keep - self.base).max(0) as usize).min(self.l.len());
        if dead > 0 {
            self.l.drain(..dead);
            self.r.drain(..dead);
            self.base += dead as i64;
        }
    }
}

/// The magnitude at `b`, or the vertex of the parabola through `a, b, c` when `b` is a local
/// maximum of the magnitude (the vertex then lies within half an instant of `b`).
fn crest(a: f32, b: f32, c: f32) -> f32 {
    let m = b.abs();
    if m < a.abs() || m < c.abs() {
        return m;
    }
    // In `b`'s sign, so `b` is the largest of the three and the curvature is ≤ 0.
    let (a, c) = if b < 0.0 { (-a, -c) } else { (a, c) };
    let curve = 2.0 * m - a - c;
    if curve > 0.0 {
        let d = a - c;
        m + d * d / (8.0 * curve)
    } else {
        m
    }
}
