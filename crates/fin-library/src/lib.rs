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
//! policy ([`process`]). Windows is Task 3.
//!
//! **Read-only roots:** nothing in this crate creates, writes, renames, locks or deletes
//! anything under a root.

pub mod collide;
pub mod exclude;
pub mod glob;
pub mod listing;
pub mod names;
pub mod open;
pub mod placeholder;
pub mod process;
pub mod raw;
pub mod reparse;
pub mod source;
pub mod types;
pub mod volume;

pub use collide::{Collision, collisions};
pub use exclude::{ExcludeSpec, Excludes};
pub use open::open_for_read;
pub use raw::RawRel;
pub use source::{Attrs, DirHandle, RawEntry, RootStat, Source};
pub use types::{
    DirDone, EntryType, Expect, OpenError, Placeholder, RootState, SampleEntry, ScanBatch,
    ScanEntry, VolumeIdentity,
};
pub use volume::{classify_root, identity, root_state};
