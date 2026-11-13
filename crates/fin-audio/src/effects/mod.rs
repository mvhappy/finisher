//! The per-item effect chain (spec §8.4; plan 6C Decision 16). Task 7 wires the hook into the
//! mixer; Task 8 fills it (parameter interpolation, the EQ) and Task 9 adds the dynamics.
//!
//! **Contract the mixer relies on.** [`Chain::process`] is called with the item's own signal
//! (1 channel for a mono source, 2 for stereo: effects run before pan) for consecutive,
//! non-overlapping timeline sample ranges in order, starting at the item's first rendered
//! sample. Its output must depend only on the samples and their timeline positions, never on
//! how the range is cut into calls (Decision 17): coefficient updates (every 64 samples) sit on
//! a grid anchored at the item's first sample `sample_at(item.from)`, and filter and detector
//! state carries across calls. State is created by [`Chain::new`], i.e. reset only at the item's
//! first sample.

use fin_core::CoreError;
use fin_core::eval::AudioItem;

use crate::channels::Canonical;
use crate::mix::SourceLayout;

/// An item's enabled effects in plan order.
pub struct Chain {}

impl Chain {
    /// The chain for `item`'s effects on a source of `layout`.
    pub fn new(item: &AudioItem, layout: SourceLayout) -> Result<Chain, CoreError> {
        let _ = (&item.effects, layout);
        Ok(Chain {})
    }

    /// Processes `buf` in place; its first frame is timeline sample `at`. A no-op until Task 8.
    pub fn process(&mut self, at: i64, buf: &mut Canonical) {
        let _ = (at, buf);
    }
}
