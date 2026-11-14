//! What `fin listen` reports over a range (plan 6C Decision 21, spec §8.7): per mix, the
//! loudness the [`Meter`] reads, clipping, silences and the loudest short-term moments.
//!
//! - [`Analyser`] takes the mix's 48 kHz blocks in order from the range start and answers a
//!   [`ListenStats`]. Its numbers are reported, never hashed: nothing here is in A0.
//! - **Clipping:** every sample (each channel's) whose magnitude is above 1.0; `firstAt` is the
//!   first such frame's timeline sample ÷ 48,000, exact.
//! - **Silence is its own detector**, not short-term loudness (whose 3 s window cannot resolve a
//!   0.7 s gap): 10 ms blocks of 480 frames aligned to the range start (the last one shorter
//!   when the range ends mid-block); a block is silent when the larger of its two channels'
//!   mean squares is below −60 dBFS (`1e-6`, compared as a power: no logarithm); a silence is a
//!   maximal run of silent blocks lasting 0.5 s or more, from its first block's start to its
//!   last block's end. A run touching an edge of the range is cut there and reported whatever
//!   its length inside (Decision 21: clipped, not dropped).
//! - **Loudest moments:** the meter's short-term (3 s) loudness read every 100 ms from the range
//!   start; up to 5 readings, loudest first (an earlier one on a tie), each window starting at
//!   least 3 s from every one already chosen; `at` is the start of the reading's 3 s window,
//!   clipped to the range start.
//! - [`Pass`] is one mix to measure: a [`Mixer`] over a plan (the whole one, or one track's items:
//!   [`filter_plan`]) and its analyser.

use std::collections::BTreeMap;

use fin_core::eval::{AudioItem, AudioPlan};
use fin_core::{CoreError, Rational};
use fin_media::jobs::{ListenClipped, ListenMoment, ListenSpan, ListenStats};

use crate::loudness::Meter;
use crate::mix::{Mixer, PcmSource};
use crate::{RATE, Stereo, sample_at};

/// A silence block: 10 ms at 48 kHz.
pub const SILENCE_BLOCK: usize = 480;
/// −60 dBFS as a mean square.
pub const SILENCE_POWER: f64 = 1e-6;
/// The shortest silence reported: 0.5 s.
pub const SILENCE_MIN: i64 = RATE as i64 / 2;
/// How many loudest moments are reported.
pub const LOUDEST: usize = 5;
/// The short-term window, and the least distance between two loudest moments: 3 s.
pub const SHORT_TERM: i64 = 3 * RATE as i64;
/// The short-term loudness is read at this hop: 100 ms.
const HOP: usize = RATE as usize / 10;

/// A timeline sample as rational seconds.
fn seconds(sample: i64) -> Rational {
    Rational::new(sample, i64::from(RATE)).expect("a sample index over 48,000 is a rational")
}

/// Measures one mix over timeline samples `[start, end)` (module docs).
pub struct Analyser {
    start: i64,
    end: i64,
    /// The timeline sample of the next frame pushed.
    pos: i64,
    meter: Meter,
    /// Frames not yet given to the meter (fewer than a hop).
    hop: Stereo,
    /// Short-term readings: `(window end, LUFS)`.
    readings: Vec<(i64, f64)>,
    /// The current silence block's sums of squares and length.
    block: (f64, f64, usize),
    /// The first sample of the current run of silent blocks.
    run: Option<i64>,
    silences: Vec<(i64, i64)>,
    clipped: u64,
    first_clip: Option<i64>,
}

impl Analyser {
    pub fn new(start: i64, end: i64) -> Analyser {
        Analyser {
            start,
            end,
            pos: start,
            meter: Meter::new(2),
            hop: Stereo::default(),
            readings: Vec::new(),
            block: (0.0, 0.0, 0),
            run: None,
            silences: Vec::new(),
            clipped: 0,
            first_clip: None,
        }
    }

    /// The next frames of the mix, in order from `start`.
    pub fn push(&mut self, block: &Stereo) {
        for (i, (&l, &r)) in block.l.iter().zip(&block.r).enumerate() {
            let at = self.pos + i as i64;
            let over = u64::from(l.abs() > 1.0) + u64::from(r.abs() > 1.0);
            if over > 0 {
                self.clipped += over;
                self.first_clip.get_or_insert(at);
            }
            let (l, r) = (f64::from(l), f64::from(r));
            self.block.0 += l * l;
            self.block.1 += r * r;
            self.block.2 += 1;
            if self.block.2 == SILENCE_BLOCK {
                self.close_block(at + 1);
            }
        }
        self.pos += block.len() as i64;
        self.hop.l.extend_from_slice(&block.l);
        self.hop.r.extend_from_slice(&block.r);
        let mut done = 0;
        while self.hop.len() - done >= HOP {
            let piece = Stereo {
                l: self.hop.l[done..done + HOP].to_vec(),
                r: self.hop.r[done..done + HOP].to_vec(),
            };
            self.meter.push(&piece);
            done += HOP;
            let at = self.pos - (self.hop.len() - done) as i64;
            if let Some(st) = self.meter.short_term() {
                self.readings.push((at, st));
            }
        }
        self.hop.l.drain(..done);
        self.hop.r.drain(..done);
    }

    /// Ends the current silence block at `end` (exclusive).
    fn close_block(&mut self, end: i64) {
        let (l, r, n) = std::mem::replace(&mut self.block, (0.0, 0.0, 0));
        if n == 0 {
            return;
        }
        let begin = end - n as i64;
        let silent = l.max(r) / (n as f64) < SILENCE_POWER;
        if silent {
            self.run.get_or_insert(begin);
        } else if let Some(from) = self.run.take() {
            self.silence(from, begin);
        }
    }

    /// A run of silent blocks over `[from, to)`: reported when it lasts [`SILENCE_MIN`] or more,
    /// or when it touches an edge of the range (Decision 21: clipped there, never dropped, since
    /// the silence may go on outside the range).
    fn silence(&mut self, from: i64, to: i64) {
        let (from, to) = (from.max(self.start), to.min(self.end));
        let edge = from <= self.start || to >= self.end;
        if to > from && (edge || to - from >= SILENCE_MIN) {
            self.silences.push((from, to));
        }
    }

    /// The loudest readings (module docs).
    fn loudest(&self) -> Vec<ListenMoment> {
        let mut order: Vec<&(i64, f64)> = self.readings.iter().collect();
        order.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut chosen: Vec<(i64, f64)> = Vec::new();
        for &(end, lufs) in order {
            if chosen.len() == LOUDEST {
                break;
            }
            let at = (end - SHORT_TERM).max(self.start);
            if chosen.iter().all(|(c, _)| (c - at).abs() >= SHORT_TERM) {
                chosen.push((at, lufs));
            }
        }
        chosen
            .into_iter()
            .map(|(at, lufs)| ListenMoment {
                at: seconds(at),
                short_term: lufs,
            })
            .collect()
    }

    /// The statistics of everything pushed.
    pub fn finish(mut self) -> ListenStats {
        let end = self.pos;
        self.close_block(end);
        if let Some(from) = self.run.take() {
            self.silence(from, end);
        }
        if !self.hop.is_empty() {
            let rest = std::mem::take(&mut self.hop);
            self.meter.push(&rest);
        }
        let m = self.meter.measured();
        ListenStats {
            integrated: m.integrated,
            range: m.range,
            true_peak: m.true_peak,
            sample_peak: m.sample_peak,
            clipped: ListenClipped {
                samples: self.clipped,
                first_at: self.first_clip.map(seconds),
            },
            silences: self
                .silences
                .iter()
                .map(|&(from, to)| ListenSpan {
                    from: seconds(from),
                    to: seconds(to),
                })
                .collect(),
            loudest: self.loudest(),
        }
    }
}

/// `plan` with only the items `keep` holds, in order.
pub fn filter_plan(plan: &AudioPlan, keep: impl Fn(&AudioItem) -> bool) -> AudioPlan {
    AudioPlan {
        timeline: plan.timeline.clone(),
        from: plan.from,
        to: plan.to,
        items: plan.items.iter().filter(|i| keep(i)).cloned().collect(),
    }
}

/// Whether WSOLA is asked for a speed it cannot do well (Decision 6: below 1/4 or above 4).
pub fn stretch_extreme(speed: Rational) -> bool {
    speed < Rational::new(1, 4).expect("1/4") || speed > Rational::from_int(4)
}

/// One mix to measure: a mixer over a plan and its analyser, rendered block by block from the
/// plan's `from`.
pub struct Pass {
    mixer: Mixer,
    analyser: Analyser,
    buf: Stereo,
}

impl Pass {
    /// The plan's range in samples is `[sample_at(from), sample_at(to))`.
    pub fn new(
        plan: &AudioPlan,
        sources: BTreeMap<String, Box<dyn PcmSource>>,
    ) -> Result<Pass, CoreError> {
        let (start, end) = range(plan)?;
        Ok(Pass {
            mixer: Mixer::new(plan, sources)?,
            analyser: Analyser::new(start, end),
            buf: Stereo::default(),
        })
    }

    /// Mixes and measures the next `frames` frames.
    pub fn render(&mut self, frames: usize) -> Result<(), CoreError> {
        self.buf.l.clear();
        self.buf.l.resize(frames, 0.0);
        self.mixer.render(&mut self.buf)?;
        self.analyser.push(&self.buf);
        Ok(())
    }

    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    pub fn finish(self) -> ListenStats {
        self.analyser.finish()
    }
}

/// `[sample_at(plan.from), sample_at(plan.to))`.
pub fn range(plan: &AudioPlan) -> Result<(i64, i64), CoreError> {
    Ok((sample_at(plan.from)?, sample_at(plan.to)?))
}

/// Mixes and measures all of `plan` in blocks of up to `block` frames.
pub fn measure(
    plan: &AudioPlan,
    sources: BTreeMap<String, Box<dyn PcmSource>>,
    block: usize,
) -> Result<ListenStats, CoreError> {
    let (start, end) = range(plan)?;
    let mut pass = Pass::new(plan, sources)?;
    let mut at = start;
    while at < end {
        let n = (end - at).min(block.max(1) as i64) as usize;
        pass.render(n)?;
        at += n as i64;
    }
    Ok(pass.finish())
}
