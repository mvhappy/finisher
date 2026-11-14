//! `asset.move`: puts an asset in a bin, or at the top with no `bin` (plan 4C Task 13). The
//! bin must exist (`NOT_FOUND`, structural). The diff records the asset's `bin`; the inverse
//! moves it back.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::bin_add::resolve_bin;
use crate::ops::util::{OpDiff, not_found, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{Diff, EntityKind};

/// The parameter an asset move sets.
const BIN: &str = "bin";

/// `asset.move`: puts `asset` in `bin` (at the top when absent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssetMove {
    pub asset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub bin: Option<String>,
}

impl From<AssetMove> for Op {
    fn from(op: AssetMove) -> Op {
        Op::AssetMove(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::AssetMove(m) = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(&m.asset)?;
    let bin = resolve_bin(project, m.bin.as_ref(), bindings)?;
    let asset = project
        .assets
        .get_mut(&id)
        .ok_or_else(|| not_found("asset", &id))?;
    let old = std::mem::replace(&mut asset.bin, bin);
    diff.record_path_of(EntityKind::Asset, &id, None, BIN);
    Ok(vec![
        AssetMove {
            asset: id,
            bin: old,
        }
        .into(),
    ])
}

/// Sets the asset's `bin`, using the bin.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::AssetMove(m) = op {
        f.touches.push((m.asset.clone(), Touch::Param(BIN.into())));
        f.uses.extend(m.bin.clone());
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

/// The inverse is one `asset.move`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "asset.move";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::AssetMove(m) => {
            let mut out = vec![&mut m.asset];
            out.extend(m.bin.as_mut());
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::AssetMove(m) => {
            let mut out = vec![m.asset.as_str()];
            out.extend(m.bin.as_deref());
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
