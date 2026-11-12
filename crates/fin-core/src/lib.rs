//! Finisher edit domain: model, operations, evaluation, branches.

pub mod apply;
pub mod branch;
pub mod captions;
pub mod compose;
pub mod error;
pub mod eval;
pub mod events;
pub mod history;
pub mod ids;
pub mod layout;
pub mod model;
pub mod ops;
pub mod params;
pub mod rebase;
pub mod record;
pub mod replay;
pub mod schema;
pub mod time;
pub mod tx;
pub mod upgrade;
pub mod view;

pub use apply::{
    ApplyEnv, ApplyMode, apply_internal, apply_tx, apply_tx_env, apply_tx_mode, canonicalize,
};
pub use error::CoreError;
pub use ids::{ID_KINDS, IdAlloc, IdKind, MemIds};
pub use layout::{FaceRef, Measured, TextLayout, TextMeasurer, TextRequest, TextStyle};
pub use model::*;
pub use ops::{
    AssetAdd, AssetMove, BinAdd, BinMove, BinRemove, DeleteMode, Edge, InsertMode, ItemDelete,
    ItemInsert, ItemLink, ItemMove, ItemRemove, ItemRestore, ItemSlide, ItemSlip, ItemSplit,
    ItemTrim, ItemUnlink, MarkerAdd, MarkerRemove, MarkerSet, Op, ParamSet, SlideSides,
    TimelineAdd, TimelineRemove, TimelineRestore, TimelineSet, TrackAdd, TrackMove, TrackRemove,
    TrackRestore, TrackSet, TransitionRemove, TransitionSet, TrimMode, normalize,
};
pub use ops::{
    CaptionMerge, CaptionSetStyle, CaptionSetText, CaptionSetWords, CaptionSplit, FontPinOp,
    FontRepin, FontUnpin, StyleAdd, StyleRemove, StyleSet,
};
pub use ops::{
    EffectAdd, EffectMove, EffectRemove, EffectRestore, KeyframeRemove, KeyframeRestore,
    KeyframeSet,
};
pub use rebase::{Conflict, Conflicts, rebase_tx, rebase_tx_env};
pub use record::{
    Link, MAX_DROP_IDS, MIN_READER, RECORD_FORMAT, Record, RecordKind, encode_record, kind_name,
    record_format, resolve_ops,
};
pub use replay::{BoundIds, apply_committed};
pub use schema::{json_schema, json_schema_with};
pub use time::{Rational, check_canonical_times, frame_duration, ntsc_rate, parse_time};
pub use tx::*;
pub use upgrade::{upcast_diff, upcast_record, upcast_snapshot};

/// The crate version, as declared in its manifest.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_manifest() {
        assert_eq!(version(), "0.0.0");
    }
}
