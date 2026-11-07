//! The `clip.*` client aliases (plan 4C Decision 6): `clip.insert`, `clip.trim` and
//! `clip.delete` are the `item.*` ops in ripple mode, kept for clients written against
//! schema 1. [`normalize`] rewrites them (and upgrades a schema-1 client tx) at the commit
//! entry, so stored intents never hold one; a client apply rewrites each alias the same way
//! before applying it, and internal and replay applies refuse them (`INVALID_OP`).

use serde_json::json;

use crate::error::CoreError;
use crate::model::{AssetKind, ItemBody, SCHEMA_VERSION};
use crate::ops::util::{normalize_body, normalize_interp, normalize_value};
use crate::ops::{DeleteMode, InsertMode, ItemDelete, ItemInsert, ItemTrim, Op, TrimMode};
use crate::time::Rational;
use crate::tx::Tx;

impl Op {
    /// Whether the op is a `clip.*` client alias.
    pub fn is_alias(&self) -> bool {
        matches!(
            self,
            Op::ClipInsert { .. } | Op::ClipTrim { .. } | Op::ClipDelete { .. }
        )
    }
}

/// The `item.*` op an alias stands for; any other op unchanged.
pub(crate) fn to_item(op: &Op) -> Op {
    match op {
        Op::ClipInsert {
            id,
            track,
            at,
            asset,
            source_in,
            duration,
        } => ItemInsert {
            id: id.clone(),
            track: track.clone(),
            at: *at,
            duration: *duration,
            mode: InsertMode::Ripple,
            body: ItemBody::Clip {
                asset: asset.clone(),
                source_in: *source_in,
                speed: Rational::from_int(1),
            },
        }
        .into(),
        Op::ClipTrim { clip, edge, delta } => ItemTrim {
            item: clip.clone(),
            edge: *edge,
            delta: *delta,
            mode: TrimMode::Ripple,
            alone: false,
        }
        .into(),
        Op::ClipDelete { clip } => ItemDelete {
            item: clip.clone(),
            mode: DeleteMode::Ripple,
        }
        .into(),
        other => other.clone(),
    }
}

/// A client tx as it is committed (plan 4C Decision 6): every `clip.*` alias rewritten to its
/// `item.*` op. A schema-1 tx is accepted when it holds only schema-1 op kinds (`asset.add`,
/// `clip.insert`, `clip.trim`, `clip.delete`): it becomes schema 2, and its `asset.add`s name
/// `kind: "media"`. A schema-1 tx with another op is `SCHEMA_VERSION` (with `opIndex`); any
/// other schema version is left for the apply to refuse. Ids, bindings and `argsHash` are
/// untouched.
///
/// The values an op carries are made canonical too, so the stored intent's bytes match the
/// state it made: every `-0.0` in an `item.insert` body (graphic slots, caption overrides)
/// becomes `0.0`, and a non-finite number is `INVALID_RANGE` (`details.path`, `opIndex`).
pub fn normalize(mut tx: Tx) -> Result<Tx, CoreError> {
    if tx.schema_version == 1 {
        for (i, op) in tx.ops.iter_mut().enumerate() {
            match op {
                Op::AssetAdd(a) => a.kind = AssetKind::Media,
                Op::ClipInsert { .. } | Op::ClipTrim { .. } | Op::ClipDelete { .. } => {}
                other => {
                    return Err(CoreError::new(
                        "SCHEMA_VERSION",
                        format!("{} is not a schema 1 op", other.name()),
                    )
                    .with_details(json!({
                        "expected": SCHEMA_VERSION,
                        "actual": 1,
                        "op": other.name(),
                        "opIndex": i,
                    }))
                    .with_fix(format!("send schemaVersion {SCHEMA_VERSION}")));
                }
            }
        }
        tx.schema_version = SCHEMA_VERSION;
    }
    if tx.schema_version == SCHEMA_VERSION {
        for (i, op) in tx.ops.iter_mut().enumerate() {
            if op.is_alias() {
                *op = to_item(op);
            }
            let canonical = match op {
                Op::ItemInsert(insert) => normalize_body(&mut insert.body),
                Op::ParamSet(s) => match &mut s.value {
                    Some(v) => normalize_value(&s.path, v),
                    None => Ok(()),
                },
                Op::EffectAdd(a) => a
                    .params
                    .iter_mut()
                    .try_for_each(|(k, v)| normalize_value(&format!("effects.{}.{k}", a.id), v)),
                Op::KeyframeSet(k) => normalize_value(&k.path, &mut k.value)
                    .and_then(|()| normalize_interp(&k.path, &mut k.interp)),
                _ => Ok(()),
            };
            canonical.map_err(|mut e| {
                if let serde_json::Value::Object(d) = &mut e.details {
                    d.insert("opIndex".into(), json!(i));
                }
                e
            })?;
        }
    }
    Ok(tx)
}

/// The op-table entries of the aliases: names and ids from their own fields, everything else
/// through the `item.*` op they stand for.
macro_rules! alias_module {
    ($module:ident, $name:literal, $variant:ident { $($field:ident),* } => [$($id:ident),*], $alloc:expr, $positions:expr) => {
        pub(crate) mod $module {
            use crate::error::CoreError;
            use crate::ids::IdAlloc;
            use crate::model::Project;
            use crate::ops::util::OpDiff;
            use crate::ops::{Bindings, Op};
            use crate::rebase::Footprint;
            use crate::time::Rational;
            use crate::tx::Diff;

            /// The wire name.
            pub(crate) const NAME: &str = $name;
            /// Aliases are client ops.
            pub(crate) const INTERNAL: bool = false;

            /// Applies the `item.*` op the alias stands for (client applies only; the applier
            /// refuses aliases in every other mode).
            pub(crate) fn apply(
                project: &mut Project,
                op: &Op,
                bindings: &mut Bindings,
                ids: &mut dyn IdAlloc,
                diff: &mut OpDiff<'_>,
            ) -> Result<Vec<Op>, CoreError> {
                crate::ops::apply(project, &super::to_item(op), bindings, ids, diff)
            }

            /// The footprint of the `item.*` op the alias stands for.
            pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
                crate::ops::footprint(&super::to_item(op), op_diff)
            }

            #[allow(unused_variables)]
            pub(crate) fn positions_mut(op: &mut Op) -> Vec<(String, &mut Rational)> {
                match op {
                    Op::$variant { $($field,)* .. } => $positions,
                    _ => Vec::new(),
                }
            }

            /// Aliases carry no item-relative values.
            pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
                Vec::new()
            }

            /// As the `item.*` op the alias stands for.
            pub(crate) fn inverse_len(op: &Op, own: &Diff) -> usize {
                crate::ops::inverse_len(&super::to_item(op), own)
            }

            #[allow(unused_variables)]
            pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
                match op {
                    Op::$variant { $($field,)* .. } => vec![$($id),*],
                    _ => Vec::new(),
                }
            }

            #[allow(unused_variables)]
            pub(crate) fn ids(op: &Op) -> Vec<&str> {
                match op {
                    Op::$variant { $($field,)* .. } => vec![$($id.as_str()),*],
                    _ => Vec::new(),
                }
            }

            #[allow(unused_variables)]
            pub(crate) fn allocates(op: &Op) -> Option<&str> {
                match op {
                    Op::$variant { $($field,)* .. } => $alloc,
                    _ => None,
                }
            }
        }
    };
}

alias_module!(
    clip_insert, "clip.insert", ClipInsert { id, track, asset, at } => [id, track, asset],
    Some(id.as_str()).filter(|id| id.starts_with('$')),
    vec![(track.clone(), at)]
);
alias_module!(
    clip_trim, "clip.trim", ClipTrim { clip } => [clip], None, Vec::new()
);
alias_module!(
    clip_delete, "clip.delete", ClipDelete { clip } => [clip], None, Vec::new()
);
