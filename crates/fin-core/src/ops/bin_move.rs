//! `bin.move`: puts a bin inside another one, or at the top with no `parent` (plan 4C Task
//! 13). Structural: the parent exists (`NOT_FOUND`), and is neither the bin nor inside it
//! (`BIN_CYCLE`). The diff records the bin's `parent`; the inverse moves it back.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::bin_add::resolve_bin;
use crate::ops::util::{OpDiff, not_found, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{Diff, EntityKind};

/// The parameter a bin move sets.
const PARENT: &str = "parent";

/// `bin.move`: puts `bin` inside `parent` (at the top when absent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BinMove {
    pub bin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub parent: Option<String>,
}

impl From<BinMove> for Op {
    fn from(op: BinMove) -> Op {
        Op::BinMove(op)
    }
}

/// `BIN_CYCLE` when `parent` is `bin` or inside it.
fn check_cycle(project: &Project, bin: &str, parent: &str) -> Result<(), CoreError> {
    let mut path = vec![parent.to_string()];
    let mut at = Some(parent.to_string());
    while let Some(b) = at {
        if b == bin {
            return Err(CoreError::new(
                "BIN_CYCLE",
                format!("bin {parent} is bin {bin} or inside it"),
            )
            .with_details(json!({ "bin": bin, "parent": parent, "path": path }))
            .with_fix("move the bin into a bin outside it, or to the top"));
        }
        // Parents form a tree (this check keeps them one), so the walk ends; the bound
        // guards against a hand-made cycle.
        if path.len() > project.bins.len() + 1 {
            break;
        }
        at = project.bins.get(&b).and_then(|x| x.parent.clone());
        path.extend(at.clone());
    }
    Ok(())
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::BinMove(m) = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(&m.bin)?;
    if !project.bins.contains_key(&id) {
        return Err(not_found("bin", &id));
    }
    let parent = resolve_bin(project, m.parent.as_ref(), bindings)?;
    if let Some(p) = &parent {
        check_cycle(project, &id, p)?;
    }
    let bin = project
        .bins
        .get_mut(&id)
        .ok_or_else(|| not_found("bin", &id))?;
    let old = std::mem::replace(&mut bin.parent, parent);
    diff.record_path_of(EntityKind::Bin, &id, None, PARENT);
    Ok(vec![
        BinMove {
            bin: id,
            parent: old,
        }
        .into(),
    ])
}

/// Sets the bin's `parent`, using the new parent.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::BinMove(m) = op {
        f.touches.push((m.bin.clone(), Touch::Param(PARENT.into())));
        f.uses.extend(m.parent.clone());
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

/// The inverse is one `bin.move`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "bin.move";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::BinMove(m) => {
            let mut out = vec![&mut m.bin];
            out.extend(m.parent.as_mut());
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::BinMove(m) => {
            let mut out = vec![m.bin.as_str()];
            out.extend(m.parent.as_deref());
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
