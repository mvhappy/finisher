//! `bin.remove`: takes an empty bin off the project (plan 4C Task 13). A bin holding assets or
//! other bins is `BIN_NOT_EMPTY` (structural: nothing may name a missing bin), with
//! `details.assets` and `details.bins`. The diff records it (`removed`); the inverse is
//! `bin.add` with its literal id.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::bin_add::BinAdd;
use crate::ops::util::{OpDiff, not_found, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// `bin.remove`: takes the empty `bin` off the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BinRemove {
    pub bin: String,
}

impl From<BinRemove> for Op {
    fn from(op: BinRemove) -> Op {
        Op::BinRemove(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::BinRemove(r) = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(&r.bin)?;
    if !project.bins.contains_key(&id) {
        return Err(not_found("bin", &id));
    }
    let inside = |p: &Option<String>| p.as_deref() == Some(id.as_str());
    let assets: Vec<&str> = project
        .assets
        .values()
        .filter(|a| inside(&a.bin))
        .map(|a| a.id.as_str())
        .collect();
    let bins: Vec<&str> = project
        .bins
        .values()
        .filter(|b| inside(&b.parent))
        .map(|b| b.id.as_str())
        .collect();
    if !assets.is_empty() || !bins.is_empty() {
        return Err(
            CoreError::new("BIN_NOT_EMPTY", format!("bin {id} is not empty"))
                .with_details(json!({ "bin": id, "assets": assets, "bins": bins }))
                .with_fix("move its assets and bins out first (asset.move, bin.move)"),
        );
    }
    let bin = project
        .bins
        .remove(&id)
        .ok_or_else(|| not_found("bin", &id))?;
    diff.record(EntityKind::Bin, &id, ChangeKind::Removed);
    Ok(vec![
        BinAdd {
            id: bin.id,
            name: bin.name,
            parent: bin.parent,
        }
        .into(),
    ])
}

/// Needs the bin and removes it.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::BinRemove(r) = op {
        f.touches.push((r.bin.clone(), Touch::Exists));
        f.removes.push(r.bin.clone());
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

/// The inverse is one `bin.add`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "bin.remove";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::BinRemove(r) => vec![&mut r.bin],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::BinRemove(r) => vec![&r.bin],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
