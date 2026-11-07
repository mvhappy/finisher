//! Primitive edit operations, one module per op (schema, validate, apply, invert, conflict
//! footprint, positions).
//!
//! Client ops: `asset.add`, `item.insert`, `item.trim`, `item.delete`, `item.slip`,
//! `item.split`, `item.move`, `item.slide`, `item.link`, `item.unlink` (and the `clip.*`
//! aliases of `item.insert`, `item.trim` and `item.delete`, [`alias`]). Internal ops exist
//! only as inverses (`asset.remove`, `item.remove`, `item.restore`, and `item.trim` in `strict`
//! mode or `alone`, `item.slide` with `sides`, `item.link` with a literal id); clients sending
//! them get `INVALID_OP`.
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
mod asset_remove;
mod defs;
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
pub(crate) mod util;

pub use alias::normalize;
use alias::{clip_delete, clip_insert, clip_trim};
pub use defs::{
    AssetAdd, DeleteMode, Edge, InsertMode, ItemDelete, ItemInsert, ItemLink, ItemMove, ItemRemove,
    ItemRestore, ItemSlide, ItemSlip, ItemSplit, ItemTrim, ItemUnlink, Op, SlideSides, TrimMode,
};

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
            /// record re-run under fresh ids, plan 4B Decision 27).
            pub fn refs_mut(&mut self) -> Vec<&mut String> {
                match self { $(Op::$variant { .. } => $module::refs_mut(self),)* }
            }

            /// Every id the op names (new-entity ids, tracks, assets, clips). After
            /// placeholders are resolved these are real ids; replay accepts literal ones,
            /// which the store's id counters must see too. Lists the same fields as
            /// `refs_mut`.
            pub fn ids(&self) -> Vec<&str> {
                match self { $(Op::$variant { .. } => $module::ids(self),)* }
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

        /// Applies one op to `project`, recording into `diff` under the op's index; returns
        /// the op's inverse ops (in order).
        pub(crate) fn apply(
            project: &mut Project,
            op: &Op,
            bindings: &mut Bindings,
            ids: &mut dyn IdAlloc,
            diff: &mut util::OpDiff<'_>,
        ) -> Result<Vec<Op>, CoreError> {
            match op { $(Op::$variant { .. } => $module::apply(project, op, bindings, ids, diff),)* }
        }

        /// How many ops the inverse of `op` holds, read off its own diff (`own`: what it
        /// recorded). The rebase engine cuts an undo of several steps at these boundaries
        /// ([`crate::rebase::steps_from`]); [`crate::apply::Applier`] checks it against every
        /// inverse it builds in debug builds.
        pub(crate) fn inverse_len(op: &Op, own: &Diff) -> usize {
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
    AssetRemove => asset_remove,
    ItemRemove => item_remove,
    ItemRestore => item_restore,
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
