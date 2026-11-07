//! `effect.move`: moves an effect of an item to index `to` of its effect list (the index it
//! has afterwards). A client's index past the end is `INVALID_RANGE`; an internal apply (an
//! undo's inverse re-run after other effects were removed, and its replay) moves it to the
//! end. The change is the item's with `path: "effects"`, so two moves of one item's effects conflict (`SAME_PARAM`,
//! Decision 20); the footprint needs `<item>/<fx>`, so a move of a removed effect is
//! `REMOVED`. The inverse moves it back.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::effect_add::EFFECTS;
use crate::ops::effect_remove::effect_index;
use crate::ops::util::{OpDiff, check_unlocked, invalid_range, item_index, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::Diff;

/// `effect.move`: `effect` of `item` to index `to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffectMove {
    pub item: String,
    pub effect: String,
    pub to: u32,
}

impl From<EffectMove> for Op {
    fn from(op: EffectMove) -> Op {
        Op::EffectMove(op)
    }
}

/// The inverse is one `effect.move`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::EffectMove(m) = op else {
        return Err(wrong_op(op));
    };
    let item = bindings.resolve(&m.item)?;
    let (ti, ci) = item_index(project, &item)?;
    let it = &ti.get(project).items[ci];
    let from = effect_index(it, &m.effect)?;
    let len = it.common.effects.len();
    // An internal apply (an undo's inverse re-run after other effects were removed, and the
    // replay of what it committed) moves to the end instead; a client's index is exact.
    if m.to as usize >= len && !bindings.internal() {
        let max = len - 1;
        return Err(invalid_range(
            format!("item {item} has {len} effects: there is no index {}", m.to),
            json!({ "item": item, "to": m.to, "min": 0, "max": max }),
            format!("use an index from 0 to {max}"),
        ));
    }
    check_unlocked(ti.get(project), Some(&item), bindings)?;
    let track = ti.get_mut(project);
    let effects = &mut track.items[ci].common.effects;
    let effect = effects.remove(from);
    let to = (m.to as usize).min(effects.len());
    effects.insert(to, effect);
    let track_id = track.id.clone();
    diff.record_path(&track_id, &item, EFFECTS);
    Ok(vec![
        EffectMove {
            item,
            effect: m.effect.clone(),
            to: u32::try_from(from).unwrap_or(u32::MAX),
        }
        .into(),
    ])
}

/// Touches the item's effect order and needs `<item>/<fx>`.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::EffectMove(m) = op {
        f.touches
            .push((m.item.clone(), Touch::Param(EFFECTS.to_string())));
        f.uses.push(format!("{}/{}", m.item, m.effect));
    }
    f
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// No item-relative values.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The wire name.
pub(crate) const NAME: &str = "effect.move";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::EffectMove(m) => vec![&mut m.item, &mut m.effect],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::EffectMove(m) => vec![&m.item, &m.effect],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
