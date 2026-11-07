//! `bin.add`: a folder of assets (plan 4C Task 13), at the top or inside `parent` (which must
//! exist: `NOT_FOUND`, structural). `id` is a `$name` placeholder bound to a new `b<n>` id (a
//! literal id restores one: internal only, the inverse of `bin.remove`). The diff records the
//! bin (`added`); the inverse is `bin.remove`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{Bin, Project};
use crate::ops::bin_remove::BinRemove;
use crate::ops::util::{OpDiff, not_found, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// `bin.add`: a bin named `name`, inside `parent` when given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BinAdd {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub parent: Option<String>,
}

impl From<BinAdd> for Op {
    fn from(op: BinAdd) -> Op {
        Op::BinAdd(op)
    }
}

/// `parent` resolved through `bindings`, which must name a bin.
pub(crate) fn resolve_bin(
    project: &Project,
    bin: Option<&String>,
    bindings: &Bindings,
) -> Result<Option<String>, CoreError> {
    let Some(b) = bin else {
        return Ok(None);
    };
    let b = bindings.resolve(b)?;
    if !project.bins.contains_key(&b) {
        return Err(not_found("bin", &b));
    }
    Ok(Some(b))
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::BinAdd(a) = op else {
        return Err(wrong_op(op));
    };
    let parent = resolve_bin(project, a.parent.as_ref(), bindings)?;
    let id = bindings.create(project, &a.id, IdKind::Bin, ids)?;
    project.bins.insert(
        id.clone(),
        Bin {
            id: id.clone(),
            name: a.name.clone(),
            parent,
        },
    );
    diff.record(EntityKind::Bin, &id, ChangeKind::Added);
    Ok(vec![BinRemove { bin: id }.into()])
}

/// Creates the bin, using its parent.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::BinAdd(a) = op {
        f.creates.push(a.id.clone());
        f.uses.extend(a.parent.clone());
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

/// The inverse is one `bin.remove`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "bin.add";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the new id and the parent.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::BinAdd(a) => {
            let mut out = vec![&mut a.id];
            out.extend(a.parent.as_mut());
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::BinAdd(a) => {
            let mut out = vec![a.id.as_str()];
            out.extend(a.parent.as_deref());
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::BinAdd(a) => Some(a.id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
