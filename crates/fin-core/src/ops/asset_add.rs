//! `asset.add`: registers a media asset under a placeholder-allocated id.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{Asset, Project};
use crate::ops::util::{OpDiff, invalid_range, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// Checks the op against `project`; on success the op can be applied.
pub(crate) fn validate(project: &Project, op: &Op, bindings: &Bindings) -> Result<(), CoreError> {
    let Op::AssetAdd { id, duration, .. } = op else {
        return Err(wrong_op(op));
    };
    // Structural: clips need a positive source range to point into.
    if !duration.is_positive() {
        return Err(invalid_range(
            "asset duration must be positive",
            json!({ "duration": duration }),
            "probe the file again; a media asset has a positive duration",
        ));
    }
    bindings.check_new(project, id, IdKind::Asset)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    validate(project, op, bindings)?;
    let Op::AssetAdd {
        id,
        path,
        duration,
        video,
        audio,
        fingerprint,
    } = op
    else {
        return Err(wrong_op(op));
    };
    let id = bindings.create(project, id, IdKind::Asset, ids)?;
    let asset = Asset {
        id: id.clone(),
        path: path.clone(),
        duration: *duration,
        video: video.clone(),
        audio: audio.clone(),
        fingerprint: fingerprint.clone(),
    };
    project.assets.insert(id.clone(), asset);
    diff.record(EntityKind::Asset, &id, ChangeKind::Added);
    Ok(vec![Op::AssetRemove { id }])
}

/// Creates the asset; binds no track.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::AssetAdd { id, .. } = op {
        f.creates.push(id.clone());
    }
    f
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The wire name.
pub(crate) const NAME: &str = "asset.add";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::AssetAdd { id, .. } => vec![id],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::AssetAdd { id, .. } => vec![id],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::AssetAdd { id, .. } => Some(id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
