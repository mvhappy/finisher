//! `fin-library`: everything that touches a library root (plan 5D Decision 1).
//!
//! Task 1 (this crate's pure half): the frozen wire types ([`types`], [`raw::RawRel`]), raw
//! paths and WTF-8 ([`raw`]), display names and lookup keys ([`names`]), key collisions
//! ([`collide`]), placeholder and reparse-tag rules ([`placeholder`], [`reparse`]), exclusion
//! rules and globs ([`exclude`], [`glob`]), one listing as scan entries ([`listing`]), volume
//! rules ([`volume`]), and the [`source::Source`] a scan reads through, with a fake
//! filesystem behind `test-hooks` (`source::fake`). Those modules make no OS call.
//!
//! Task 2 (Linux and macOS): the native [`source::NativeSource`] (`getdents64` + `statx`;
//! `getattrlistbulk`), volume identity ([`identity`], [`root_state`]), the safe opener
//! ([`open_for_read`], the only open of a file under a root) and the process's dataless
//! policy ([`process`]).
//!
//! Task 3 (Windows): the same [`source::NativeSource`] from directory handles
//! (`NtQueryDirectoryFileEx`, relative `NtCreateFile` opens that never follow a reparse
//! point), volume identity from the volume serial and the UNC share, and the opener.
//!
//! Task 5: the budgeted reader ([`budget`]), the probe of one file ([`probe`], through
//! `fin_probe::probe_reader`) and the on-demand whole-file hash ([`hash`]).
//!
//! Task 4: the parallel walker ([`walk()`], [`mod@walk`]; deadlines in [`timing`]) and, behind
//! `test-hooks`, the overlay that marks real paths as placeholders, delays, errors and mount
//! changes for worker tests (`overlay`).
//!
//! Task 13: `asset.probe` of a catalogued file ([`asset`]): [`fin_probe::probe_with`] over the
//! safe opener, its refusals as the codes the RPC answers.
//!
//! **Read-only roots:** nothing in this crate creates, writes, renames, locks or deletes
//! anything under a root.

pub mod asset;
pub mod batchfile;
pub mod budget;
pub mod collide;
pub mod exclude;
pub mod glob;
pub mod hash;
pub mod listing;
pub mod names;
pub mod open;
pub mod params;
pub mod placeholder;
pub mod probe;
pub mod process;
pub mod raw;
pub mod reparse;
pub mod source;
pub mod timing;
pub mod types;
pub mod volume;
pub mod walk;

#[cfg(feature = "test-hooks")]
pub mod overlay;
#[cfg(feature = "test-hooks")]
pub mod params_test;

pub use asset::probe_asset;
pub use budget::{BudgetedReader, Throttle};
pub use collide::{Collision, collisions};
pub use exclude::{ExcludeSpec, Excludes};
pub use hash::{HashOutcome, hash_file};
pub use open::open_for_read;
pub use probe::{FileRef, ProbeOutcome, probe_file};
pub use raw::RawRel;
pub use source::{Attrs, DirHandle, Listing, RawEntry, RootStat, Source};
pub use timing::Timing;
pub use types::{
    DirDone, EntryType, Expect, KnownDir, OpenError, Placeholder, RootState, SampleEntry,
    ScanBatch, ScanEntry, VolumeIdentity,
};
pub use volume::{classify_root, identity, root_state};
pub use walk::{
    CancelToken, KnownDirs, WalkError, WalkMode, WalkOpts, WalkStop, WalkSummary, walk,
};
