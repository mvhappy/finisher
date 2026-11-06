//! Primitive edit operations, one module per op (schema, validate, apply, invert, conflict
//! footprint, positions).
//!
//! Client ops: `asset.add`, `clip.insert`, `clip.trim`, `clip.delete`. Internal ops exist only
//! as inverses (`asset.remove`, `clip.remove`, `clip.restore`); clients sending them get
//! `INVALID_OP`.
//!
//! Every clip op records exactly one `Shift` in the diff (possibly `by: 0`) and asset ops record
//! none. The rebase engine relies on that to pair a transaction's shifts with its ops.

use std::collections::BTreeMap;

use serde_json::json;

use crate::apply::ApplyMode;
use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::Project;
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::Diff;

mod asset_add;
mod asset_remove;
mod clip_delete;
mod clip_insert;
mod clip_remove;
mod clip_restore;
mod clip_trim;
mod defs;
pub(crate) mod util;

pub use defs::{Edge, Op};

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
/// module provides `NAME`, `INTERNAL`, `RECORDS_SHIFT`, `refs_mut`, `ids`, `allocates`,
/// `positions_mut`, `apply` and `footprint`.
macro_rules! ops_table {
    ($($variant:ident => $module:ident),* $(,)?) => {
        impl Op {
            /// The wire name (`"clip.insert"`, ...).
            pub fn name(&self) -> &'static str {
                match self { $(Op::$variant { .. } => $module::NAME,)* }
            }

            /// Internal ops are produced as inverses and never accepted from clients.
            pub fn is_internal(&self) -> bool {
                match self { $(Op::$variant { .. } => $module::INTERNAL,)* }
            }

            /// Clip ops record exactly one `Shift` when applied; asset ops record none.
            pub(crate) fn records_shift(&self) -> bool {
                match self { $(Op::$variant { .. } => $module::RECORDS_SHIFT,)* }
            }

            /// Every id reference in the op (new-entity ids, tracks, assets, clips), mutably,
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
            /// these are the order ids are allocated in). `clip.restore` binds one when its
            /// clip id is a placeholder (spec §4.3).
            pub fn allocates(&self) -> Option<&str> {
                match self { $(Op::$variant { .. } => $module::allocates(self),)* }
            }

            /// Every timeline position the op carries, with its track, mutably, so a rebase
            /// can map them (`clip.insert` `at`, `clip.restore` `clip.start`).
            pub(crate) fn positions_mut(&mut self) -> Vec<(String, &mut Rational)> {
                match self { $(Op::$variant { .. } => $module::positions_mut(self),)* }
            }
        }

        /// Applies one op to `project`, appending to `diff`; returns the op's inverse ops (in
        /// order).
        pub(crate) fn apply(
            project: &mut Project,
            op: &Op,
            bindings: &mut Bindings,
            ids: &mut dyn IdAlloc,
            diff: &mut Diff,
        ) -> Result<Vec<Op>, CoreError> {
            match op { $(Op::$variant { .. } => $module::apply(project, op, bindings, ids, diff),)* }
        }

        /// The conflict footprint of `op` from its own diff (`op_diff`: its single shift, if
        /// any). A committed op comes with its placeholders resolved; one of ours may keep its
        /// `$name`s.
        pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
            match op { $(Op::$variant { .. } => $module::footprint(op, op_diff),)* }
        }
    };
}

ops_table! {
    AssetAdd => asset_add,
    ClipInsert => clip_insert,
    ClipTrim => clip_trim,
    ClipDelete => clip_delete,
    AssetRemove => asset_remove,
    ClipRemove => clip_remove,
    ClipRestore => clip_restore,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Clip;
    use crate::time::Rational;

    #[test]
    fn positions_mut_lists_placements() {
        let r = Rational::from_int;
        let mut insert = Op::ClipInsert {
            id: "$c".into(),
            track: "t2".into(),
            asset: "a1".into(),
            at: r(3),
            source_in: r(0),
            duration: r(1),
        };
        for (track, pos) in insert.positions_mut() {
            assert_eq!(track, "t2");
            *pos = r(7);
        }
        assert!(matches!(insert, Op::ClipInsert { at, .. } if at == r(7)));
        let mut restore = Op::ClipRestore {
            clip: Clip {
                id: "c1".into(),
                asset: "a1".into(),
                start: r(4),
                source_in: r(0),
                duration: r(1),
            },
            track: "t1".into(),
            ripple: true,
        };
        let got: Vec<(String, Rational)> = restore
            .positions_mut()
            .into_iter()
            .map(|(t, p)| (t, *p))
            .collect();
        assert_eq!(got, vec![("t1".to_string(), r(4))]);
        let mut trim = Op::ClipTrim {
            clip: "c1".into(),
            edge: Edge::End,
            delta: r(1),
        };
        assert!(trim.positions_mut().is_empty());
        assert!(
            Op::ClipDelete { clip: "c1".into() }
                .positions_mut()
                .is_empty()
        );
    }

    #[test]
    fn ids_lists_the_fields_refs_mut_does() {
        let r = Rational::from_int;
        let clip = Clip {
            id: "c1".into(),
            asset: "a1".into(),
            start: r(0),
            source_in: r(0),
            duration: r(1),
        };
        let ops = [
            Op::AssetAdd {
                id: "a1".into(),
                path: "p".into(),
                duration: r(1),
                video: None,
                audio: None,
                fingerprint: "f".into(),
            },
            Op::AssetRemove { id: "a2".into() },
            Op::ClipInsert {
                id: "c3".into(),
                track: "t1".into(),
                asset: "a1".into(),
                at: r(0),
                source_in: r(0),
                duration: r(1),
            },
            Op::ClipTrim {
                clip: "c4".into(),
                edge: crate::Edge::End,
                delta: r(1),
            },
            Op::ClipDelete { clip: "c5".into() },
            Op::ClipRemove {
                id: "c6".into(),
                ripple: true,
            },
            Op::ClipRestore {
                clip,
                track: "t2".into(),
                ripple: true,
            },
        ];
        for op in ops {
            let mut m = op.clone();
            let want: Vec<String> = m.refs_mut().into_iter().map(|s| s.clone()).collect();
            assert_eq!(op.ids(), want, "{}", op.name());
        }
    }
}
