//! The per-item effect chain (spec §8.4; plan 6C Decisions 13, 15 and 16): the item's enabled
//! effects in plan order, run on its own signal after WSOLA and before gain and pan.
//!
//! **Contract the mixer relies on.** [`Chain::process`] is called with the item's own signal
//! (1 channel for a mono source, 2 for stereo: effects run before pan) for consecutive,
//! non-overlapping timeline sample ranges in order, starting at the item's first rendered
//! sample. Its output depends only on the samples and their timeline positions, never on how
//! the range is cut into calls (Decision 17): coefficient updates sit on a grid of [`BLOCK`]
//! samples anchored at the item's first sample `sample_at(item.from)`, and filter and detector
//! state carries across calls. State is created by [`Chain::new`], i.e. reset only at the
//! item's first sample.
//!
//! **Blocks.** The chain cuts every call at the grid edges. At the start of each grid block
//! (or the first time a block is entered mid-way, when a render starts inside an item) every
//! effect's [`Effect::update`] reads its parameters at the **block's first sample** and
//! recomputes its coefficients; then each effect in plan order runs [`Effect::run`] over the
//! part of the block in hand. So within a block coefficients never move, and an effect sees the
//! previous effect's output for the same samples.
//!
//! **Adding an effect (Task 9).** Implement [`Effect`] (build its [`params::ParamCurve`]s from
//! [`params::ParamsAt`] in the constructor, design in `f64` with `libm` in `update`, run scalar
//! `f32` in `run`, state per channel or linked across `ch`), then add its kind to
//! [`Chain::new`]'s match. A kind the engine does not know (a newer model's) is skipped and
//! listed by [`Chain::skipped`] (and [`crate::Mixer::skipped_effects`]).

pub mod biquad;
pub mod eq;
pub mod params;

use fin_core::CoreError;
use fin_core::eval::AudioItem;

use crate::channels::Canonical;
use crate::mix::SourceLayout;
use crate::sample_at;

/// Samples between coefficient updates (spec §8.4).
pub const BLOCK: i64 = 64;

/// One effect's running state.
pub trait Effect {
    /// Reads the parameters at grid sample `n` (a block's first sample) and recomputes the
    /// coefficients the next [`Effect::run`] calls use.
    fn update(&mut self, n: i64);
    /// Processes frames in place: `ch` holds 1 (mono) or 2 (L, R) slices of equal length, all
    /// inside one grid block, following the previous call's frames.
    fn run(&mut self, ch: &mut [&mut [f32]]);
}

/// An effect the plan names but this engine does not know: skipped (it does not sound).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedEffect {
    /// The plan item (`AudioItem::item`).
    pub item: String,
    /// The effect's id.
    pub effect: String,
    pub kind: String,
}

/// An item's enabled effects in plan order.
pub struct Chain {
    /// The grid's anchor: the item's first sample.
    start: i64,
    /// The grid block whose parameters are loaded (its first sample).
    loaded: Option<i64>,
    fx: Vec<Box<dyn Effect>>,
    skipped: Vec<SkippedEffect>,
}

impl Chain {
    /// The chain for `item`'s effects on a source of `layout`. Errors: `TIME_OVERFLOW` when a
    /// time does not fit a sample index.
    pub fn new(item: &AudioItem, layout: SourceLayout) -> Result<Chain, CoreError> {
        let _ = layout; // the effects so far run whatever channels they are handed
        let (start, end) = (sample_at(item.from)?, sample_at(item.to)?);
        let mut fx: Vec<Box<dyn Effect>> = Vec::new();
        let mut skipped = Vec::new();
        for e in &item.effects {
            match e.kind.as_str() {
                "eq" => fx.push(Box::new(eq::Eq::new(e, start, end)?)),
                // Registry kinds whose DSP arrives with plan 6C Task 9: a passthrough until then.
                "compressor" | "gate" | "deEsser" => {}
                _ => skipped.push(SkippedEffect {
                    item: item.item.clone(),
                    effect: e.id.clone(),
                    kind: e.kind.clone(),
                }),
            }
        }
        Ok(Chain {
            start,
            loaded: None,
            fx,
            skipped,
        })
    }

    /// The effects the plan names that this engine skipped, in plan order.
    pub fn skipped(&self) -> &[SkippedEffect] {
        &self.skipped
    }

    /// Processes `buf` in place; its first frame is timeline sample `at`.
    pub fn process(&mut self, at: i64, buf: &mut Canonical) {
        if self.fx.is_empty() {
            return;
        }
        let len = match buf {
            Canonical::Mono(m) => m.len(),
            Canonical::Stereo(s) => s.l.len(),
        };
        let mut i = 0usize;
        while i < len {
            let n = at + i as i64;
            // In i128: an item may span more than i64 samples apart (Task 7's overflow rules).
            let into =
                (i128::from(n) - i128::from(self.start)).rem_euclid(i128::from(BLOCK)) as i64;
            let block = n - into;
            if self.loaded != Some(block) {
                for f in &mut self.fx {
                    f.update(block);
                }
                self.loaded = Some(block);
            }
            let stop = len.min(i + (BLOCK - into) as usize);
            match buf {
                Canonical::Mono(m) => {
                    let mut ch = [&mut m[i..stop]];
                    self.fx.iter_mut().for_each(|f| f.run(&mut ch));
                }
                Canonical::Stereo(s) => {
                    let mut ch = [&mut s.l[i..stop], &mut s.r[i..stop]];
                    self.fx.iter_mut().for_each(|f| f.run(&mut ch));
                }
            }
            i = stop;
        }
    }
}
