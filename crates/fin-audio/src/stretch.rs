//! Pitch-preserving time-stretch by WSOLA (waveform-similarity overlap-add; plan 6C Decision 6,
//! spec §8.2).
//!
//! **Geometry.** Frames of [`FRAME`] = 1,024 samples, periodic Hann window, 50 % overlap, so the
//! synthesis hop is [`HOP`] = 512 output frames. Frame `j` covers output frames
//! `[(j − 1)·HOP, (j + 1)·HOP)` and reads source frames `[pos_j, pos_j + FRAME)`, with
//! `pos_j = c_j − HOP + δ_j`, `c_j = round_half_up(j·HOP·speed)` and `δ_0 = 0`. Output frame
//! `k = j·HOP + r` (`0 ≤ r < HOP`, "region `j`") is
//! `w[HOP + r]·x[pos_j + HOP + r] + w[r]·x[pos_{j+1} + r]`, the two products added in that order.
//!
//! **Driven by source position.** `c_j` is computed from `j` and the exact rational speed, never
//! by adding hops, so frame `j`'s centre sits on the source frame Decision 4 assigns to output
//! frame `j·HOP` ([`Wsola::source_at`]) and the offset `δ_j ∈ [−SEARCH, SEARCH]` is the only
//! deviation: the stretcher cannot drift.
//!
//! **Search.** `δ_{j+1}` maximises the normalised cross-correlation `Σ t·y / sqrt(Σ y²)` over the
//! `HOP` samples where frame `j + 1` overlaps frame `j`: the template `t` is frame `j`'s natural
//! continuation `a[pos_j + HOP ..][..HOP]`, the candidate `y` is `a[c_{j+1} − HOP + δ ..][..HOP]`,
//! and `a = L + R` (mono analysis on the channel sum; both channels are then synthesised with
//! the same offset, so there is no phasiness between channels). `δ` is scanned `0, −1, 1, −2, 2,
//! …, −SEARCH, SEARCH` and replaced only by a strictly greater score, so ties go to the smaller
//! `|δ|`, then to the negative one; a silent candidate scores 0.
//!
//! **Determinism.** Scalar `f32` in a fixed order, no `mul_add`; the window is designed in `f64`
//! with `libm::cos` and cast once; `libm::sqrtf` for the norm. The output depends only on the
//! input samples, never on how they are chunked into `process` calls. Source frames before 0
//! read as zero; after `finish`, so do frames at or past the end of the input.
//!
//! **Speed 1** bypasses all of this: the input is copied bit-exactly. Speeds WSOLA handles badly
//! (below 1/4, above 4) still produce exactly `output_len` frames (`fin listen` warns
//! `STRETCH_EXTREME`, not this module).
//!
//! **Mono sources.** Feed the mono signal as both channels and read `l`: the analysis sum `2x`
//! is an exact scaling, so the offsets (and `l`) are bit-identical to a dedicated mono stretch.
//!
//! **Driving it from the mixer (Task 7).** All positions are frames relative to the item: output
//! frame 0 is the item's first output sample `sample_at(from)`, and source frame 0 is the
//! integer anchor `sample_at(sourceFrom)`; output sample `n` of the timeline plays absolute
//! source frame `sample_at(sourceFrom) + round_half_up((n − sample_at(from)) × speed)`
//! ([`Wsola::source_at`] of the relative index). The anchor is rounded to a whole sample before
//! the speed term is added, and the speed-1 path rounds it the same way, so the two agree. To
//! produce output frames `[0, k)` the mixer pushes source frames
//! `[0, input_needed(k))`, in any chunking, reading zeros past the source's end as `PcmSource`
//! does; `process` has then emitted at least `k` frames (it emits whole regions of `HOP`, so up
//! to `HOP − 1` more, which the mixer keeps for its next block). `input_needed` is monotone and
//! a pure function of `k` and the speed, so the mixer knows which source frames to read before
//! reading them (`input_needed(k) ≈ source_at(k) + 1,024 + HOP·speed`). WSOLA's offsets chain
//! from frame to frame, so output frame `k` depends on every frame before it: rendering from the
//! middle of an item means running the stretcher from the item's first output frame. `finish` is
//! for a source that has simply ended: it pads with zeros and emits up to `output_len(pushed)`.

use fin_core::Rational;

use crate::Stereo;

/// The analysis/synthesis frame (21.3 ms at 48 kHz).
pub const FRAME: usize = 1_024;
/// The synthesis hop: 50 % overlap.
pub const HOP: usize = FRAME / 2;
/// The offset search radius, `δ ∈ [−SEARCH, SEARCH]`.
pub const SEARCH: usize = 512;

const HOP_I: i64 = HOP as i64;
const SEARCH_I: i64 = SEARCH as i64;

/// `round_half_up(a / b)` for `b > 0`, exact and overflow-free: the remainder is `< b`, so
/// doubling it cannot overflow.
fn round_div(a: i128, b: i128) -> i128 {
    let (q, r) = (a.div_euclid(b), a.rem_euclid(b));
    if 2 * r >= b { q + 1 } else { q }
}

fn to_i64(v: i128) -> i64 {
    i64::try_from(v).expect("a stretch position fits an i64")
}

/// The periodic Hann window, `0.5 − 0.5·cos(2πm/FRAME)`, designed in `f64` and cast once.
fn hann() -> Vec<f32> {
    let tau = 2.0 * core::f64::consts::PI;
    (0..FRAME)
        .map(|m| (0.5 - 0.5 * libm::cos(tau * m as f64 / FRAME as f64)) as f32)
        .collect()
}

/// The source frames still needed, from source frame `base`.
#[derive(Default)]
struct Held {
    base: i64,
    l: Vec<f32>,
    r: Vec<f32>,
    /// The analysis signal, `l + r`.
    a: Vec<f32>,
}

impl Held {
    /// Copies source frames `[from, from + dst.len())` of `ch` into `dst`, zeros outside
    /// `[0, end)`.
    fn copy(&self, ch: &[f32], from: i64, end: i64, dst: &mut [f32]) {
        for (i, d) in dst.iter_mut().enumerate() {
            let at = from + i as i64;
            *d = if at < 0 || at >= end {
                0.0
            } else {
                debug_assert!(at >= self.base, "a dropped source frame was read");
                ch[(at - self.base) as usize]
            };
        }
    }

    /// Forgets source frames before `keep` (clamped to what is held), in amortised batches.
    fn drop_before(&mut self, keep: i64) {
        let n = (keep - self.base).clamp(0, self.a.len() as i64) as usize;
        if n >= 8 * FRAME && 2 * n >= self.a.len() {
            self.l.drain(..n);
            self.r.drain(..n);
            self.a.drain(..n);
            self.base += n as i64;
        }
    }
}

/// A streaming WSOLA time-stretcher for one item at one constant speed (see the module docs).
pub struct Wsola {
    speed: Rational,
    /// `speed = p/q`, both positive.
    p: i128,
    q: i128,
    window: Vec<f32>,
    held: Held,
    pushed: i64,
    emitted: i64,
    finished: bool,
    /// The next region to emit, `j`, and its first frame's source start, `pos_j`.
    region: i64,
    pos: i64,
    /// Scratch: the template, the candidate span, and two half frames.
    t: Vec<f32>,
    span: Vec<f32>,
    halves: Vec<f32>,
}

impl Wsola {
    /// A stretcher playing the source at `speed` (source seconds per output second).
    ///
    /// # Panics
    /// When `speed` is not positive (the model refuses such speeds).
    pub fn new(speed: Rational) -> Wsola {
        assert!(speed.is_positive(), "a stretch speed must be positive");
        Wsola {
            speed,
            p: i128::from(speed.num()),
            q: i128::from(speed.den()),
            window: hann(),
            held: Held::default(),
            pushed: 0,
            emitted: 0,
            finished: false,
            region: 0,
            pos: -HOP_I,
            t: vec![0.0; HOP],
            span: vec![0.0; HOP + 2 * SEARCH],
            halves: vec![0.0; FRAME],
        }
    }

    pub fn speed(&self) -> Rational {
        self.speed
    }

    fn bypass(&self) -> bool {
        self.p == self.q
    }

    /// The source frame output frame `k` of the item plays, relative to the item's integer
    /// source anchor `sample_at(sourceFrom)`: `round_half_up(k × speed)`, which equals
    /// `sample_at(k/48000 × speed)`. The absolute source frame is
    /// `sample_at(sourceFrom) + source_at(n − sample_at(from))` (Decision 4 as implemented: the
    /// anchor is rounded to a whole sample first, on the speed-1 path too, so both paths agree).
    ///
    /// # Panics
    /// When the source frame does not fit an `i64`.
    pub fn source_at(&self, out_frame: u64) -> i64 {
        // u64 × (p < 2^63) < 2^127: no i128 overflow.
        to_i64(round_div(i128::from(out_frame) * self.p, self.q))
    }

    /// Frame `j`'s nominal centre, `c_j = round_half_up(j·HOP·speed)`.
    fn centre(&self, j: i64) -> i64 {
        let a = (i128::from(j) * i128::from(HOP_I))
            .checked_mul(self.p)
            .expect("a stretch position fits an i64");
        to_i64(round_div(a, self.q))
    }

    /// The source frames (from 0) region `j` needs, whatever the offsets: frame `j`'s second
    /// half reads below `pos_j + FRAME ≤ c_j − HOP + SEARCH + FRAME`; the search for frame
    /// `j + 1` (candidates' first halves) and that frame's first half read below
    /// `c_{j+1} − HOP + SEARCH + HOP = c_{j+1} + SEARCH`.
    fn region_needs(&self, j: i64) -> i64 {
        (self.centre(j) - HOP_I + SEARCH_I + FRAME as i64).max(self.centre(j + 1) + SEARCH_I)
    }

    /// How many source frames (from source frame 0) must have been pushed for `process` to have
    /// emitted at least `out_frames` frames. Monotone in `out_frames`; `0` for `0`.
    pub fn input_needed(&self, out_frames: u64) -> u64 {
        if out_frames == 0 {
            return 0;
        }
        if self.bypass() {
            return out_frames;
        }
        let j = i64::try_from((out_frames - 1) / HOP as u64).expect("an output length fits an i64");
        self.region_needs(j) as u64
    }

    /// The output length of a source of `in_frames` frames: `round_half_up(in_frames / speed)`,
    /// i.e. `sample_at(in_frames/48000 / speed)`.
    ///
    /// # Panics
    /// When the length does not fit a `u64`.
    pub fn output_len(&self, in_frames: u64) -> u64 {
        u64::try_from(round_div(i128::from(in_frames) * self.q, self.p))
            .expect("a stretch output length fits a u64")
    }

    /// Source frames pushed so far.
    pub fn pushed(&self) -> u64 {
        self.pushed as u64
    }

    /// Output frames emitted so far.
    pub fn emitted(&self) -> u64 {
        self.emitted as u64
    }

    /// Pushes the next source frames and appends every output frame they complete to `out`.
    ///
    /// # Panics
    /// After `finish`.
    pub fn process(&mut self, input: &Stereo, out: &mut Stereo) {
        assert!(!self.finished, "Wsola::process after finish");
        assert_eq!(
            input.l.len(),
            input.r.len(),
            "both channels of a Wsola input have the same length"
        );
        self.pushed += input.len() as i64;
        if self.bypass() {
            out.l.extend_from_slice(&input.l);
            out.r.extend_from_slice(&input.r);
            self.emitted = self.pushed;
            return;
        }
        let h = &mut self.held;
        h.l.extend_from_slice(&input.l);
        h.r.extend_from_slice(&input.r);
        h.a.extend(input.l.iter().zip(&input.r).map(|(l, r)| l + r));
        while self.pushed >= self.region_needs(self.region) {
            self.emit_region(HOP, out);
        }
    }

    /// Ends the source (frames from `pushed` on read as zero) and appends the remaining output,
    /// so that `output_len(pushed)` frames have been emitted in all.
    pub fn finish(&mut self, out: &mut Stereo) {
        if self.finished {
            return;
        }
        self.finished = true;
        if self.bypass() {
            return;
        }
        let total = self.output_len(self.pushed()) as i64;
        while self.emitted < total {
            let n = (total - self.emitted).min(HOP_I) as usize;
            self.emit_region(n, out);
        }
    }

    /// Finds `pos_{j+1}`, emits the first `n` frames of region `j` and advances to region `j + 1`.
    fn emit_region(&mut self, n: usize, out: &mut Stereo) {
        let next = self.search();
        let end = self.end();
        let (h, w) = (&self.held, &self.window);
        let (a, b) = self.halves.split_at_mut(HOP);
        for (ch, dst) in [(&h.l, &mut out.l), (&h.r, &mut out.r)] {
            h.copy(ch, self.pos + HOP_I, end, a); // frame j's second half
            h.copy(ch, next, end, b); // frame j + 1's first half
            for r in 0..n {
                let x = w[HOP + r] * a[r];
                let y = w[r] * b[r];
                dst.push(x + y);
            }
        }
        self.emitted += n as i64;
        self.region += 1;
        self.pos = next;
        // Later regions read from the template (≥ c_j − HOP) and the next search (≥ c_{j+1} −
        // HOP − SEARCH), both ≥ c_j − HOP − SEARCH for the new j.
        let keep = self.centre(self.region) - (HOP_I + SEARCH_I);
        self.held.drop_before(keep);
    }

    /// The end of the readable source: everything pushed once finished, else unbounded (the
    /// region condition guarantees nothing unpushed is read before then).
    fn end(&self) -> i64 {
        if self.finished { self.pushed } else { i64::MAX }
    }

    /// `pos_{j+1}` for the current region `j`: the best-correlated candidate around `c_{j+1}`.
    fn search(&mut self) -> i64 {
        let lo = self.centre(self.region + 1) - HOP_I - SEARCH_I;
        let end = self.end();
        let h = &self.held;
        h.copy(&h.a, self.pos + HOP_I, end, &mut self.t);
        h.copy(&h.a, lo, end, &mut self.span);
        let (t, span) = (&self.t, &self.span);
        let score = |off: usize| -> f32 {
            let y = &span[off..off + HOP];
            let (mut dot, mut energy) = (0.0f32, 0.0f32);
            for i in 0..HOP {
                dot += t[i] * y[i];
                energy += y[i] * y[i];
            }
            if energy > 0.0 {
                dot / libm::sqrtf(energy)
            } else {
                0.0
            }
        };
        let mut best = (SEARCH, score(SEARCH));
        for d in 1..=SEARCH {
            for off in [SEARCH - d, SEARCH + d] {
                let s = score(off);
                if s > best.1 {
                    best = (off, s);
                }
            }
        }
        lo + best.0 as i64
    }
}
