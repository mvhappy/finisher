//! The mixer (spec §8.2, §9.1 item 3; plan 6C Decisions 3, 4, 16 and 17).
//!
//! The `fin-core` [`AudioPlan`] is the only input: the mixer never reads the model. Each item,
//! in plan order, reads its source (through [`crate::Wsola`] when its speed is not 1), runs its
//! effect chain ([`crate::effects::Chain`]), takes gain × shapes and pan, and is added into the
//! master bus, so the bus is `((0 + item₀) + item₁) + …` sample by sample.
//!
//! - **Time** is sample-exact: every item, gain, shape and pan edge is `sample_at` of its own
//!   rational (`round_half_up(t × 48000)`), never a sum of lengths ([`curves`]).
//! - **Gain, shapes and pan** are evaluated at control points (every 32 samples on the
//!   timeline grid and at every segment edge) and ramped linearly in amplitude between them;
//!   gain is `10^(dB/20)` (`libm::powf`), `equalPower` is `sqrt(x)`, beziers ease through
//!   `fin_core::eval::bezier_y`.
//! - **Decision (Task 7 review): the control grid is the timeline's.** The every-32-samples
//!   control points are the multiples of 32 of the *absolute timeline sample index*, not of
//!   the item's own first sample. Every item and every crossfade partner therefore
//!   shares one grid, and a control interval is a function of the sample index alone. The
//!   cost: an item moved by a number of samples that is not a multiple of 32 ramps between
//!   slightly different points, so it is not bit-identical to the original shifted (pinned by
//!   the `control_grid_is_on_the_timeline` test).
//! - **Overflow:** a block's end, an item's source reads and their exclusive ends must fit an
//!   `i64`; otherwise `render` answers `TIME_OVERFLOW` instead of wrapping.
//! - **Pan** is constant power for a mono source (which is also its one −3 dB upmix) and
//!   balance for stereo ([`pan`]); effects therefore run on one channel for mono.
//! - **Streaming:** [`Mixer::render`] fills blocks of any length from the plan's `from` on, and
//!   the output does not depend on the block lengths: control intervals are a function of the
//!   sample index, the stretcher and the effects carry their state across blocks, and each
//!   item's state starts at its first sample.
//! - **Memory:** an item frees its buffers once it has ended, and a source is dropped (closed)
//!   once every item reading it has ended, so a long plan holds only what is still to play.
//! - A source missing from the map is silent and listed by [`Mixer::missing`]; an effect of a
//!   kind the engine does not know is skipped and listed by [`Mixer::skipped_effects`].

pub(crate) mod curves;
mod item;
mod pan;

use std::collections::{BTreeMap, BTreeSet};

use fin_core::CoreError;
use fin_core::eval::AudioPlan;

use crate::channels::Canonical;
use crate::effects::SkippedEffect;
use crate::time::range_end;
use crate::{Stereo, sample_at};
use item::ItemMix;

/// A source's canonical layout (Decision 3): it picks the pan law.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLayout {
    Mono,
    Stereo,
}

/// One asset's 48 kHz canonical PCM, readable at any frame.
pub trait PcmSource {
    /// The layout every `read` answers in.
    fn layout(&self) -> SourceLayout;
    /// Replaces `out` with source frames `[at, at + frames)` in [`PcmSource::layout`]'s
    /// layout, exactly `frames` long; frames outside the source (before 0 or past its end) are
    /// zeros.
    fn read(&mut self, at: i64, frames: usize, out: &mut Canonical) -> Result<(), CoreError>;
}

/// Renders an [`AudioPlan`] block by block from its `from`.
pub struct Mixer {
    items: Vec<ItemMix>,
    sources: BTreeMap<String, Box<dyn PcmSource>>,
    missing: Vec<String>,
    skipped: Vec<SkippedEffect>,
    position: i64,
    /// The last end of the items reading each source: past it the source is closed.
    last_end: BTreeMap<String, i64>,
}

impl Mixer {
    /// A mixer for `plan` over `sources` (keyed by asset id). Errors: `TIME_OVERFLOW` when a
    /// time does not fit a sample index, `INTERNAL` for a non-positive speed (which
    /// `eval_audio` never plans).
    pub fn new(
        plan: &AudioPlan,
        sources: BTreeMap<String, Box<dyn PcmSource>>,
    ) -> Result<Mixer, CoreError> {
        let mut missing = BTreeSet::new();
        let mut items = Vec::with_capacity(plan.items.len());
        let mut skipped = Vec::new();
        let mut last_end = BTreeMap::new();
        for it in &plan.items {
            let layout = sources.get(&it.asset).map(|s| s.layout());
            if layout.is_none() {
                missing.insert(it.asset.clone());
            }
            let item = ItemMix::new(it, layout)?;
            let end = last_end.entry(it.asset.clone()).or_insert(item.end);
            *end = (*end).max(item.end);
            skipped.extend_from_slice(item.skipped_effects());
            items.push(item);
        }
        Ok(Mixer {
            items,
            sources,
            missing: missing.into_iter().collect(),
            skipped,
            position: sample_at(plan.from)?,
            last_end,
        })
    }

    /// Fills `out` (its length is the block length; `r` is resized to match `l`) with the next
    /// frames of the mix and advances [`Mixer::position`] by that length. `TIME_OVERFLOW` (and
    /// no change) when the block's end, or a source read's, would pass `i64::MAX`. A source's
    /// read error is returned as is; the mixer's item state is then partly advanced, so drop it (a new one
    /// starts again from the plan's `from`).
    pub fn render(&mut self, out: &mut Stereo) -> Result<(), CoreError> {
        let n = out.l.len();
        out.l.fill(0.0);
        out.r.clear();
        out.r.resize(n, 0.0);
        let b0 = self.position;
        let b1 = range_end(b0, n as u64)?;
        for item in &mut self.items {
            if item.end <= b0 || item.start >= b1 {
                continue;
            }
            if let Some(src) = self.sources.get_mut(&item.asset) {
                item.render(src.as_mut(), b0, b1, out)?;
            }
            if item.end <= b1 {
                item.release();
            }
        }
        // A source no item reads after this block is closed (6C Task 21 review S2).
        self.sources
            .retain(|asset, _| self.last_end.get(asset).is_some_and(|&e| e > b1));
        self.position = b1;
        Ok(())
    }

    /// The frames the items hold allocated (their read and stretch buffers): an item that has
    /// ended holds none (6C Task 21 review S2).
    pub fn buffered_frames(&self) -> usize {
        self.items.iter().map(ItemMix::buffered).sum()
    }

    /// The sources still open: a source is closed once every item reading it has ended.
    pub fn open_sources(&self) -> usize {
        self.sources.len()
    }

    /// The timeline sample the next `render` starts at.
    pub fn position(&self) -> i64 {
        self.position
    }

    /// The effects of kinds this engine does not know (a newer model's), in plan order:
    /// skipped, so their items play without them.
    pub fn skipped_effects(&self) -> &[SkippedEffect] {
        &self.skipped
    }

    /// The assets the plan names but no source was given for (sorted, once each); their items
    /// are silent.
    pub fn missing(&self) -> &[String] {
        &self.missing
    }
}
