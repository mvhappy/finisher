//! `asset.reprobe` (schema 4, sub-project 6 spec §6.3): replaces a `media` asset's `video` and
//! `audio` with what a fresh probe of its file found (an absent field clears that stream). The
//! server sends it after probing the unchanged file again (`asset.reprobe` RPC), so an asset
//! imported before schema 4 gains its display size, rotation and pixel aspect.
//!
//! Structural: the asset exists (`NOT_FOUND`) and is a `media` asset (`INVALID_OP`, as
//! `asset.add`'s kind shapes); every item showing it on a video or audio track keeps the
//! stream that track plays (`TRACK_KIND_MISMATCH`, as `item.insert`). Policy: a media asset has
//! a video or an audio stream (`INVALID_OP`). The diff records the asset's `video` and `audio`
//! parameters; the inverse is an `asset.reprobe` carrying the previous values.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{AssetKind, AudioInfo, ItemBody, Project, VideoInfo};
use crate::ops::util::{OpDiff, check_kind, not_found, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{Diff, EntityKind};

/// The parameters a reprobe sets.
const VIDEO: &str = "video";
const AUDIO: &str = "audio";

/// `asset.reprobe`: `asset`'s streams are now `video` and `audio`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssetReprobe {
    pub asset: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "VideoInfo")]
    pub video: Option<VideoInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "AudioInfo")]
    pub audio: Option<AudioInfo>,
}

impl From<AssetReprobe> for Op {
    fn from(op: AssetReprobe) -> Op {
        Op::AssetReprobe(Box::new(op))
    }
}

fn payload(op: &Op) -> Result<&AssetReprobe, CoreError> {
    match op {
        Op::AssetReprobe(r) => Ok(r),
        _ => Err(wrong_op(op)),
    }
}

/// Checks the op against `project` (the asset id resolved): the checks of the module docs.
fn validate(
    project: &Project,
    r: &AssetReprobe,
    id: &str,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    let asset = project
        .assets
        .get(id)
        .ok_or_else(|| not_found("asset", id))?;
    let shape = |why: String, fix: &str| {
        CoreError::new("INVALID_OP", why)
            .with_details(json!({ "asset": id, "kind": asset.kind }))
            .with_fix(fix.to_string())
    };
    if asset.kind != AssetKind::Media {
        return Err(shape(
            format!("asset {id} is not a media asset; only media assets are reprobed"),
            "reprobe a media asset",
        ));
    }
    if bindings.policy() && r.video.is_none() && r.audio.is_none() {
        return Err(shape(
            format!("a media asset has a video or an audio stream; the reprobe of {id} has none"),
            "probe the file again",
        ));
    }
    let after = crate::model::Asset {
        video: r.video.clone(),
        audio: r.audio.clone(),
        ..asset.clone()
    };
    for timeline in &project.timelines {
        for track in &timeline.tracks {
            let shows = track.items.iter().any(|i| match &i.body {
                ItemBody::Clip { asset, .. } => asset == id,
                _ => false,
            });
            if shows {
                check_kind(track, &after)?;
            }
        }
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
    let r = payload(op)?;
    let id = bindings.resolve(&r.asset)?;
    validate(project, r, &id, bindings)?;
    let asset = project
        .assets
        .get_mut(&id)
        .ok_or_else(|| not_found("asset", &id))?;
    let video = std::mem::replace(&mut asset.video, r.video.clone());
    let audio = std::mem::replace(&mut asset.audio, r.audio.clone());
    diff.record_path_of(EntityKind::Asset, &id, None, VIDEO);
    diff.record_path_of(EntityKind::Asset, &id, None, AUDIO);
    Ok(vec![
        AssetReprobe {
            asset: id,
            video,
            audio,
        }
        .into(),
    ])
}

/// Sets the asset's `video` and `audio`.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::AssetReprobe(r) = op {
        for param in [VIDEO, AUDIO] {
            f.touches
                .push((r.asset.clone(), Touch::Param(param.into())));
        }
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

/// The inverse is one `asset.reprobe`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "asset.reprobe";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::AssetReprobe(r) => vec![&mut r.asset],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::AssetReprobe(r) => vec![r.asset.as_str()],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
