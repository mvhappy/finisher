//! `asset.remove` (internal): the inverse of `asset.add`. Only unused assets can go.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::util::{asset, record, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

pub(crate) fn validate(
    project: &Project,
    op: &Op,
    bindings: &Bindings,
) -> Result<String, CoreError> {
    let Op::AssetRemove { id } = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(id)?;
    asset(project, &id)?;
    let users: Vec<&str> = project
        .timeline
        .tracks
        .iter()
        .flat_map(|t| t.items.iter())
        .filter(|c| c.asset == id)
        .map(|c| c.id.as_str())
        .collect();
    if !users.is_empty() {
        return Err(
            CoreError::new("ASSET_IN_USE", format!("asset {id} is used by clips"))
                .with_details(json!({ "id": id, "clips": users }))
                .with_fix("delete the clips that use the asset first"),
        );
    }
    Ok(id)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut Diff,
) -> Result<Vec<Op>, CoreError> {
    let id = validate(project, op, bindings)?;
    let Some(a) = project.assets.remove(&id) else {
        return Err(crate::ops::util::not_found("asset", &id));
    };
    record(diff, EntityKind::Asset, &id, ChangeKind::Removed);
    Ok(vec![Op::AssetAdd {
        id: a.id,
        path: a.path,
        duration: a.duration,
        video: a.video,
        audio: a.audio,
        fingerprint: a.fingerprint,
    }])
}

/// Removes the asset; binds no track.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::AssetRemove { id } = op {
        f.touches.push((id.clone(), Touch::Exists));
        f.removes.push(id.clone());
    }
    f
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The wire name.
pub(crate) const NAME: &str = "asset.remove";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = true;
/// Whether applying the op records exactly one `Shift`.
pub(crate) const RECORDS_SHIFT: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::AssetRemove { id } => vec![id],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::AssetRemove { id } => vec![id],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
