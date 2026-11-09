//! Rebasing a transaction onto a head (spec §4.3, Decisions 16–18).
//!
//! The engine ([`rebase_apply`]) takes one op at a time:
//! 1. its timeline positions are mapped (`Assoc::After`) through a [`Mapping`] of
//!    `[inverse(our earlier ops), intervening steps, our rebased earlier ops]`, each inverse
//!    mirrored with its rebased op, so a position inside our own earlier insert comes back;
//! 2. its footprint is judged against the unmirrored intervening steps ([`entity_conflict`]);
//! 3. it is re-run on the evolving head with full policy. Any failure is a [`Conflict`].
//!
//! [`rebase_tx`] is the stale-commit entry: the frame is the tx applied at its base.

pub mod anchors;
pub mod conflict;
mod engine;
pub mod footprint;
pub mod map;
pub mod places;
mod steps;

pub use conflict::{Conflict, Conflicts, reasons};
pub use engine::{
    PreparedRebase, Rebase, Rebased, rebase_apply, rebase_prepared, rebase_tx, rebase_tx_env,
};
pub use footprint::{
    Footprint, Touch, committed_footprints, committed_ops, entity_conflict, footprint, split_diff,
};
pub use map::{Assoc, MapResult, Mapping, Recover, StepMap, Unmapped};
pub(crate) use steps::Mirrored;
pub use steps::{Prepared, Step, StepUnit, steps_from, steps_with_units};
