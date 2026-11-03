//! `asset.add`: registers a media asset under a placeholder-allocated id.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{Asset, Project};
use crate::ops::util::{invalid_range, record, wrong_op};
use crate::ops::{Bindings, Op};
use crate::tx::{ChangeKind, Diff, EntityKind};

/// Checks the op against `project`; on success the op can be applied.
pub(crate) fn validate(project: &Project, op: &Op, bindings: &Bindings) -> Result<(), CoreError> {
    let Op::AssetAdd { id, duration, .. } = op else { return Err(wrong_op(op)) };
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
    diff: &mut Diff,
) -> Result<Vec<Op>, CoreError> {
    validate(project, op, bindings)?;
    let Op::AssetAdd { id, path, duration, video, audio, fingerprint } = op else { return Err(wrong_op(op)) };
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
    record(diff, EntityKind::Asset, &id, ChangeKind::Added);
    Ok(vec![Op::AssetRemove { id }])
}
