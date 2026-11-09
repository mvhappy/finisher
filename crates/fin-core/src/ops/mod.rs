//! Primitive edit operations, one module per op (schema, validate, apply, invert, conflict
//! footprint, positions).
//!
//! Client ops: `asset.add`, `item.insert`, `item.trim`, `item.delete`, `item.slip`,
//! `item.split`, `item.move`, `item.slide`, `item.link`, `item.unlink`, `transition.set`,
//! `transition.remove`, `track.add|remove|move|set`, `timeline.add|set|remove`,
//! `marker.add|set|remove`, `bin.add|move|remove`, `asset.move`, `asset.remove`, `param.set`,
//! `effect.add|remove|move`, `keyframe.set|remove`, `style.add|set|remove`,
//! `font.pin|unpin|repin`, `caption.setText|split|merge|setStyle` (and the
//! `clip.*` aliases of `item.insert`, `item.trim` and `item.delete`, [`alias`]). Internal ops
//! exist only as inverses (`item.remove`, `item.restore`, `track.restore`,
//! `timeline.restore`, `effect.restore`, `caption.setWords`, and `item.trim` in `strict` mode
//! or `alone`, `item.slide` with `sides`, `item.link` with a literal id); clients sending them
//! get `INVALID_OP`.
//!
//! Every change and shift an op records carries the op's index in its transaction
//! ([`util::OpDiff`]), so a transaction's diff splits into its ops' own diffs
//! ([`crate::rebase::split_diff`]) whatever each op records.

use std::collections::BTreeMap;

use serde_json::json;

use crate::apply::ApplyMode;
use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::Project;
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::Diff;

mod alias;
mod asset_add;
mod asset_move;
mod asset_remove;
mod bin_add;
mod bin_move;
mod bin_remove;
mod caption_merge;
mod caption_set_style;
mod caption_set_text;
mod caption_set_words;
mod caption_split;
mod defs;
mod effect_add;
mod effect_move;
mod effect_remove;
mod effect_restore;
mod font_pin;
mod font_repin;
mod font_unpin;
mod item_delete;
mod item_insert;
mod item_link;
mod item_move;
mod item_remove;
mod item_restore;
mod item_slide;
mod item_slip;
mod item_split;
mod item_trim;
mod item_unlink;
mod keyframe_remove;
mod keyframe_set;
mod marker_add;
mod marker_remove;
mod marker_set;
mod param_set;
mod style_add;
mod style_remove;
mod style_set;
mod timeline_add;
mod timeline_remove;
mod timeline_restore;
mod timeline_set;
mod track_add;
mod track_move;
mod track_remove;
mod track_restore;
mod track_set;
mod transition_remove;
mod transition_set;
pub(crate) mod util;

pub use alias::normalize;
use alias::{clip_delete, clip_insert, clip_trim};
pub use asset_move::AssetMove;
pub use bin_add::BinAdd;
pub use bin_move::BinMove;
pub use bin_remove::BinRemove;
pub use caption_merge::CaptionMerge;
pub use caption_set_style::CaptionSetStyle;
pub use caption_set_text::CaptionSetText;
pub use caption_set_words::CaptionSetWords;
pub use caption_split::CaptionSplit;
pub use defs::{
    AssetAdd, DeleteMode, Edge, InsertMode, ItemDelete, ItemInsert, ItemLink, ItemMove, ItemRemove,
    ItemRestore, ItemSlide, ItemSlip, ItemSplit, ItemTrim, ItemUnlink, Op, SlideSides,
    TransitionRemove, TransitionSet, TrimMode,
};
pub use effect_add::EffectAdd;
pub use effect_move::EffectMove;
pub use effect_remove::EffectRemove;
pub use effect_restore::EffectRestore;
pub use font_pin::FontPinOp;
pub use font_repin::FontRepin;
pub use font_unpin::FontUnpin;
pub use keyframe_remove::KeyframeRemove;
pub use keyframe_set::KeyframeSet;
pub use marker_add::MarkerAdd;
pub use marker_remove::MarkerRemove;
pub use marker_set::MarkerSet;
pub use param_set::ParamSet;
pub use style_add::StyleAdd;
pub use style_remove::StyleRemove;
pub use style_set::StyleSet;
pub use timeline_add::TimelineAdd;
pub use timeline_remove::TimelineRemove;
pub use timeline_restore::TimelineRestore;
pub use timeline_set::TimelineSet;
pub use track_add::TrackAdd;
pub use track_move::TrackMove;
pub use track_remove::TrackRemove;
pub use track_restore::TrackRestore;
pub use track_set::TrackSet;

/// `$name` placeholders bound during one transaction, in op order.
pub(crate) struct Bindings {
    pub(crate) map: BTreeMap<String, String>,
    /// Literal ids may name new entities (internal and replay).
    internal: bool,
    /// Policy checks run (everything but replay).
    policy: bool,
}

impl Bindings {
    pub(crate) fn new(mode: ApplyMode) -> Self {
        Bindings {
            map: BTreeMap::new(),
            internal: mode != ApplyMode::Client,
            policy: mode != ApplyMode::Replay,
        }
    }

    /// Whether literal ids may name new entities and internal ops and modes are allowed
    /// (internal and replay applies).
    pub(crate) fn internal(&self) -> bool {
        self.internal
    }

    /// Whether policy checks run. Replay ([`ApplyMode::Replay`]) checks structure only: what
    /// was committed under the rules of its day is not re-judged.
    pub(crate) fn policy(&self) -> bool {
        self.policy
    }

    /// A reference: `$name` resolves through the bindings, anything else is a literal id.
    pub(crate) fn resolve(&self, reference: &str) -> Result<String, CoreError> {
        if !reference.starts_with('$') {
            return Ok(reference.to_string());
        }
        self.map.get(reference).cloned().ok_or_else(|| {
            CoreError::new(
                "UNBOUND_PLACEHOLDER",
                format!("placeholder {reference} is not bound by an earlier op"),
            )
            .with_details(json!({ "placeholder": reference }))
            .with_fix("create the entity earlier in the same transaction, or use an existing id")
        })
    }

    /// Checks that `id` can name a new entity without allocating anything.
    pub(crate) fn check_new(
        &self,
        project: &Project,
        id: &str,
        kind: IdKind,
    ) -> Result<(), CoreError> {
        if let Some(name) = id.strip_prefix('$') {
            if name.is_empty() && self.policy {
                return Err(
                    CoreError::new("INVALID_OP", "placeholder needs a name after `$`")
                        .with_details(json!({ "id": id }))
                        .with_fix("use a placeholder like $clip1"),
                );
            }
            if self.map.contains_key(id) {
                return Err(CoreError::new(
                    "DUPLICATE_PLACEHOLDER",
                    format!("placeholder {id} is bound twice"),
                )
                .with_details(json!({ "placeholder": id }))
                .with_fix("give every new entity in the transaction its own placeholder"));
            }
            return Ok(());
        }
        if !self.internal {
            return Err(CoreError::new(
                "INVALID_OP",
                "new entities are named with `$name` placeholders",
            )
            .with_details(json!({ "id": id }))
            .with_fix(format!(
                "use a placeholder such as ${id}; the server allocates the id"
            )));
        }
        if !self.policy {
            check_literal(id, kind)?;
        }
        if util::exists(project, kind, id) {
            return Err(
                CoreError::new("DUPLICATE_ID", format!("id {id} already exists"))
                    .with_details(json!({ "id": id })),
            );
        }
        Ok(())
    }

    /// Names a new entity: binds a placeholder to a freshly allocated id, or keeps a literal id
    /// (internal mode only). Call `check_new` first; this is the only allocating step.
    pub(crate) fn create(
        &mut self,
        project: &Project,
        id: &str,
        kind: IdKind,
        ids: &mut dyn IdAlloc,
    ) -> Result<String, CoreError> {
        self.check_new(project, id, kind)?;
        if !id.starts_with('$') {
            return Ok(id.to_string());
        }
        let new = ids.next(kind);
        if util::exists(project, kind, &new) {
            return Err(CoreError::new(
                "DUPLICATE_ID",
                format!("allocated id {new} already exists"),
            )
            .with_details(json!({ "id": new })));
        }
        self.map.insert(id.to_string(), new.clone());
        Ok(new)
    }
}

/// Replay: a literal id for a new entity must be `<kind prefix><digits>`, so ids read off disk
/// cannot break the id counters (`INVALID_ID`).
pub(crate) fn check_literal(id: &str, kind: IdKind) -> Result<(), CoreError> {
    if crate::ids::suffix(id, kind.prefix()).is_some() {
        return Ok(());
    }
    Err(
        CoreError::new("INVALID_ID", format!("{id} is not a {} id", kind.prefix()))
            .with_details(json!({ "id": id, "prefix": kind.prefix() })),
    )
}

/// Lists every op variant with its module once, and generates the dispatch from it. Each
/// module provides `NAME`, `INTERNAL`, `refs_mut`, `ids`, `allocates`,
/// `positions_mut`, `offsets_mut`, `inverse_len`, `apply` and `footprint`.
macro_rules! ops_table {
    ($($variant:ident => $module:ident),* $(,)?) => {
        impl Op {
            /// Every op's wire name, in table order (`events::names::OP_KINDS`).
            pub const NAMES: &'static [&'static str] = &[$($module::NAME),*];

            /// The wire name (`"item.insert"`, ...).
            pub fn name(&self) -> &'static str {
                match self { $(Op::$variant { .. } => $module::NAME,)* }
            }

            /// Internal ops are produced as inverses and never accepted from clients.
            pub fn is_internal(&self) -> bool {
                match self { $(Op::$variant { .. } => $module::INTERNAL,)* }
            }

            /// Every id reference in the op (new-entity ids, tracks, assets, items, and the ids
            /// nested in a restored item, plan 4C Decision 24), mutably,
            /// so placeholders can be substituted in one place, and ids renamed (a recovered
            /// record re-run under fresh ids, plan 4B Decision 27). The effect ids inside
            /// parameter paths are not fields of their own: [`Op::try_for_each_ref`] reaches
            /// these and those, and is what renames ids.
            pub fn refs_mut(&mut self) -> Vec<&mut String> {
                match self { $(Op::$variant { .. } => $module::refs_mut(self),)* }
            }

            /// Every id the op names (new-entity ids, tracks, assets, clips). After
            /// placeholders are resolved these are real ids; replay accepts literal ones,
            /// which the store's id counters must see too. Lists the same fields as
            /// `refs_mut`.
            /// Then the effect ids inside its parameter paths (`effects.<fx>.<param>`, the
            /// keys of the keyframe maps it carries), which [`Op::try_for_each_ref`] rewrites.
            pub fn ids(&self) -> Vec<&str> {
                let mut out = match self { $(Op::$variant { .. } => $module::ids(self),)* };
                out.extend(util::paths::path_ids(self));
                out
            }

            /// The placeholder this op binds to a newly allocated id, if any (in op order
            /// these are the order ids are allocated in). `item.restore` binds one when its
            /// item id is a placeholder (spec §4.3).
            pub fn allocates(&self) -> Option<&str> {
                match self { $(Op::$variant { .. } => $module::allocates(self),)* }
            }

            /// Every timeline position the op carries, with its track, mutably, so a rebase
            /// can map them (`item.insert` `at`, `item.restore` `item.start`).
            pub(crate) fn positions_mut(&mut self) -> Vec<(String, &mut Rational)> {
                match self { $(Op::$variant { .. } => $module::positions_mut(self),)* }
            }

            /// Every item-relative value the op carries (content-anchored, plan 4C Decisions
            /// 13 and 16), with the item it counts from (as the op names it), mutably, so a
            /// rebase can map them through the content offsets of the steps between
            /// (`item.split` `offset`; Task 14 adds keyframe `at`s).
            pub(crate) fn offsets_mut(&mut self) -> Vec<(String, &mut Rational)> {
                match self { $(Op::$variant { .. } => $module::offsets_mut(self),)* }
            }
        }

        /// [`apply`] by the op's own module (without the prune pass).
        fn module_apply(
            project: &mut Project,
            op: &Op,
            bindings: &mut Bindings,
            ids: &mut dyn IdAlloc,
            diff: &mut util::OpDiff<'_>,
        ) -> Result<Vec<Op>, CoreError> {
            match op { $(Op::$variant { .. } => $module::apply(project, op, bindings, ids, diff),)* }
        }

        /// [`inverse_len`] by the op's own module (without the prune pass).
        fn module_inverse_len(op: &Op, own: &Diff) -> usize {
            match op { $(Op::$variant { .. } => $module::inverse_len(op, own),)* }
        }

        /// The conflict footprint of `op` by its own rules, from its own diff (`op_diff`:
        /// what it recorded; [`crate::rebase::footprint`] adds the diff's changes). A
        /// committed op comes with its placeholders resolved; one of ours may keep its
        /// `$name`s.
        pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
            match op { $(Op::$variant { .. } => $module::footprint(op, op_diff),)* }
        }
    };
}

impl Op {
    /// Passes every id reference of the op through `f`: the fields [`Op::refs_mut`] lists,
    /// then the effect segment of each parameter path it carries (`effects.<fx>.<param>`, in a
    /// `param.set`/`keyframe.*` path and in the keys of a keyframe map), written back when `f`
    /// changes it (plan 4C Decision 24). Stops at the first error.
    pub fn try_for_each_ref<E>(
        &mut self,
        mut f: impl FnMut(&mut String) -> Result<(), E>,
    ) -> Result<(), E> {
        for r in self.refs_mut() {
            f(r)?;
        }
        for p in util::paths::paths_mut(self) {
            p.try_each(&mut f)?;
        }
        Ok(())
    }

    /// [`Op::try_for_each_ref`] with an `f` that cannot fail.
    pub fn for_each_ref(&mut self, mut f: impl FnMut(&mut String)) {
        let done: Result<(), std::convert::Infallible> = self.try_for_each_ref(|r| {
            f(r);
            Ok(())
        });
        let Ok(()) = done;
    }
}

/// Whether the post-op transition pass runs after `op` (plan 4C Decision 22): after every op
/// but `transition.*`, and an alias's runs as the op it stands for. Track, timeline, marker,
/// bin, asset, `param.set`, `effect.*`, `keyframe.*`, `style.*`, `font.*`,
/// `caption.setText|setWords|setStyle` ops never part touching items, so it
/// does not run after them either: a track or timeline removal records its own transitions as
/// removed, and a restore brings them back whole.
fn prunes(op: &Op) -> bool {
    !op.is_alias()
        && !matches!(
            op,
            Op::TransitionSet(_)
                | Op::TransitionRemove(_)
                | Op::TrackAdd(_)
                | Op::TrackRemove(_)
                | Op::TrackRestore(_)
                | Op::TrackMove(_)
                | Op::TrackSet(_)
                | Op::TimelineAdd(_)
                | Op::TimelineSet(_)
                | Op::TimelineRemove(_)
                | Op::TimelineRestore(_)
                | Op::MarkerAdd(_)
                | Op::MarkerSet(_)
                | Op::MarkerRemove(_)
                | Op::BinAdd(_)
                | Op::BinMove(_)
                | Op::BinRemove(_)
                | Op::AssetMove(_)
                | Op::ParamSet(_)
                | Op::EffectAdd(_)
                | Op::EffectRemove(_)
                | Op::EffectRestore(_)
                | Op::EffectMove(_)
                | Op::KeyframeSet(_)
                | Op::KeyframeRemove(_)
                | Op::StyleAdd(_)
                | Op::StyleSet(_)
                | Op::StyleRemove(_)
                | Op::FontPin(_)
                | Op::FontUnpin(_)
                | Op::FontRepin(_)
                | Op::CaptionSetText(_)
                | Op::CaptionSetWords(_)
                | Op::CaptionSetStyle(_)
        )
}

/// Applies one op to `project`, recording into `diff` under the op's index; returns the op's
/// inverse ops (in order). Then the post-op transition pass runs ([`util::prune`]): the
/// transitions whose items no longer touch on the tracks it touched are removed and restored
/// after the op's own inverse, and the ones that inverse would prune on its way are carried
/// (removed before it, restored after it).
pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut util::OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let mut inverse = module_apply(project, op, bindings, ids, diff)?;
    if dissolves(op) {
        inverse = util::dissolve_alone(project, diff, inverse)?;
    }
    if !prunes(op) {
        return Ok(inverse);
    }
    util::prune(project, diff, inverse)
}

/// Whether the post-op link pass runs after `op` ([`util::dissolve_alone`]): after every op
/// but `item.link`/`item.unlink` (which edit groups themselves), and an alias's runs as the op
/// it stands for.
fn dissolves(op: &Op) -> bool {
    !op.is_alias() && !matches!(op, Op::ItemLink(_) | Op::ItemUnlink(_))
}

/// How many ops the inverse of `op` holds, read off its own diff (`own`: what it recorded):
/// its module's count, plus what the prune pass added (an op that is not `transition.*`
/// records only that pass's `transition` changes): one restore per `removed` transition, a
/// remove and a restore per carried (`updated`) one; plus one `item.link` per group the link
/// pass dissolved. The rebase engine cuts an undo of
/// several steps at these boundaries ([`crate::rebase::steps_from`]);
/// [`crate::apply::Applier`] checks it against every inverse it builds in debug builds.
pub(crate) fn inverse_len(op: &Op, own: &Diff) -> usize {
    // The link pass's changes come first (after the module's): one `item.link` per dissolved
    // group.
    let (dissolved, own) = if dissolves(op) {
        util::split_dissolved(own)
    } else {
        (0, own.clone())
    };
    if !prunes(op) {
        return module_inverse_len(op, &own) + dissolved;
    }
    let (transitions, changes): (Vec<_>, Vec<_>) = own
        .changes
        .iter()
        .cloned()
        .partition(|c| c.kind == crate::tx::EntityKind::Transition);
    let carried = transitions
        .iter()
        .filter(|c| c.change == crate::tx::ChangeKind::Updated)
        .count();
    let rest = Diff {
        changes,
        shifts: own.shifts.clone(),
    };
    module_inverse_len(op, &rest) + transitions.len() + carried + dissolved
}

ops_table! {
    AssetAdd => asset_add,
    ItemInsert => item_insert,
    ItemTrim => item_trim,
    ItemDelete => item_delete,
    ItemSlip => item_slip,
    ItemSplit => item_split,
    ItemMove => item_move,
    ItemSlide => item_slide,
    ItemLink => item_link,
    ItemUnlink => item_unlink,
    TransitionSet => transition_set,
    TransitionRemove => transition_remove,
    TrackAdd => track_add,
    TrackRemove => track_remove,
    TrackMove => track_move,
    TrackSet => track_set,
    TimelineAdd => timeline_add,
    TimelineSet => timeline_set,
    TimelineRemove => timeline_remove,
    MarkerAdd => marker_add,
    MarkerSet => marker_set,
    MarkerRemove => marker_remove,
    BinAdd => bin_add,
    BinMove => bin_move,
    BinRemove => bin_remove,
    AssetMove => asset_move,
    TrackRestore => track_restore,
    TimelineRestore => timeline_restore,
    AssetRemove => asset_remove,
    ItemRemove => item_remove,
    ItemRestore => item_restore,
    ParamSet => param_set,
    EffectAdd => effect_add,
    EffectRemove => effect_remove,
    EffectMove => effect_move,
    KeyframeSet => keyframe_set,
    KeyframeRemove => keyframe_remove,
    EffectRestore => effect_restore,
    StyleAdd => style_add,
    StyleSet => style_set,
    StyleRemove => style_remove,
    FontPin => font_pin,
    FontUnpin => font_unpin,
    FontRepin => font_repin,
    CaptionSetText => caption_set_text,
    CaptionSetWords => caption_set_words,
    CaptionSplit => caption_split,
    CaptionMerge => caption_merge,
    CaptionSetStyle => caption_set_style,
    ClipInsert => clip_insert,
    ClipTrim => clip_trim,
    ClipDelete => clip_delete,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Item, ItemBody};
    use crate::time::Rational;

    #[test]
    fn positions_mut_lists_placements() {
        let r = Rational::from_int;
        let mut insert: Op = ItemInsert {
            id: "$c".into(),
            track: "t2".into(),
            at: r(3),
            duration: r(1),
            mode: InsertMode::Ripple,
            body: ItemBody::Adjustment {},
        }
        .into();
        for (track, pos) in insert.positions_mut() {
            assert_eq!(track, "t2");
            *pos = r(7);
        }
        assert!(matches!(&insert, Op::ItemInsert(i) if i.at == r(7)));
        let mut restore: Op = ItemRestore {
            item: Item::clip("c1", "a1", r(4), r(0), r(1)),
            track: "t1".into(),
            mode: DeleteMode::Ripple,
        }
        .into();
        let got: Vec<(String, Rational)> = restore
            .positions_mut()
            .into_iter()
            .map(|(t, p)| (t, *p))
            .collect();
        assert_eq!(got, vec![("t1".to_string(), r(4))]);
        let mut alias = Op::ClipInsert {
            id: "$c".into(),
            track: "t1".into(),
            at: r(2),
            asset: "a1".into(),
            source_in: r(0),
            duration: r(1),
        };
        assert_eq!(alias.positions_mut().len(), 1);
        let mut trim: Op = ItemTrim {
            item: "c1".into(),
            edge: Edge::End,
            delta: r(1),
            mode: TrimMode::Ripple,
            alone: false,
        }
        .into();
        assert!(trim.positions_mut().is_empty());
    }
}
