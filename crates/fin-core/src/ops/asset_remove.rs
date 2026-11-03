//! `asset.remove` (internal): the inverse of `asset.add`. Only unused assets can go.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::util::{asset, record, wrong_op};
use crate::ops::{Bindings, Op};
use crate::tx::{ChangeKind, Diff, EntityKind};

pub(crate) fn validate(project: &Project, op: &Op, bindings: &Bindings) -> Result<String, CoreError> {
    let Op::AssetRemove { id } = op else { return Err(wrong_op(op)) };
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
        return Err(CoreError::new("ASSET_IN_USE", format!("asset {id} is used by clips"))
            .with_details(json!({ "id": id, "clips": users }))
            .with_fix("delete the clips that use the asset first"));
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
    let Some(a) = project.assets.remove(&id) else { return Err(crate::ops::util::not_found("asset", &id)) };
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
