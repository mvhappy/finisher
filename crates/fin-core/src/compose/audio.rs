//! `compose::clip_with_audio` (plan 4C Decision 21): a clip of an A/V asset on a video track
//! comes with its audio, as a clip on an audio track, the two one link group.

use serde_json::{Value, json};

use crate::compose::Composed;
use crate::compose::overwrite::overwrite_named;
use crate::error::CoreError;
use crate::model::{ItemBody, Project, TrackKind};
use crate::ops::util::{asset, kind_name, track_index};
use crate::ops::{InsertMode, ItemInsert, ItemLink, Op};
use crate::time::Rational;

/// The placeholders the builder names: the video clip (as `fin clip insert` names a clip),
/// its audio, and their link group.
const CLIP: &str = "$clip";
const AUDIO: &str = "$audio";
const LINK: &str = "$link";

/// Ops for a clip of `asset` on `track` at `at` (in `mode`; an overwrite splits an item it
/// lands inside, [`crate::compose::overwrite`]). When `track` is a video track and the asset
/// has audio, an audio clip of the same span follows on `audio_track` (default: the first
/// audio track of the same timeline), then `item.link` of the two. `echo` is
/// `{ mode, audio: { track, id } | null, link: "$link" | null, splits: [...] }`.
///
/// `USAGE` (with a fix) when the audio needs a track and the timeline has none;
/// `TRACK_KIND_MISMATCH` when `audio_track` is not an audio track; `NOT_FOUND` for an unknown
/// track or asset. Everything else is the ops' own validation.
#[allow(clippy::too_many_arguments)]
pub fn clip_with_audio(
    project: &Project,
    track: &str,
    at: Rational,
    duration: Rational,
    asset_id: &str,
    source_in: Rational,
    speed: Rational,
    mode: InsertMode,
    audio_track: Option<&str>,
) -> Result<Composed, CoreError> {
    let ti = track_index(project, track)?;
    let a = asset(project, asset_id)?;
    let with_audio = ti.get(project).kind == TrackKind::Video && a.audio.is_some();
    let audio = if with_audio {
        Some(audio_track_for(project, ti.timeline, audio_track)?)
    } else {
        None
    };
    let body = ItemBody::Clip {
        asset: asset_id.to_string(),
        source_in,
        speed,
    };
    let mut ops = Vec::new();
    let mut splits = Vec::new();
    let mut place = |track: &str, id: &str, right: &str| -> Result<(), CoreError> {
        let (more, split) = clip(project, track, at, duration, body.clone(), mode, id, right)?;
        ops.extend(more);
        splits.extend(split);
        Ok(())
    };
    place(track, CLIP, "$split")?;
    if let Some(audio) = &audio {
        place(audio, AUDIO, "$split2")?;
    }
    if audio.is_some() {
        ops.push(
            ItemLink {
                items: vec![CLIP.to_string(), AUDIO.to_string()],
                id: LINK.to_string(),
            }
            .into(),
        );
    }
    let mode_name = match mode {
        InsertMode::Ripple => "ripple",
        InsertMode::Overwrite => "overwrite",
    };
    Ok(Composed {
        ops,
        echo: json!({
            "mode": mode_name,
            "audio": audio.as_ref().map(|t| json!({ "track": t, "id": AUDIO })),
            "link": audio.as_ref().map(|_| LINK),
            "splits": splits,
        }),
    })
}

/// The ops placing one clip, and the split an overwrite sends first (if any).
#[allow(clippy::too_many_arguments)]
fn clip(
    project: &Project,
    track: &str,
    at: Rational,
    duration: Rational,
    body: ItemBody,
    mode: InsertMode,
    id: &str,
    right: &str,
) -> Result<(Vec<Op>, Option<Value>), CoreError> {
    if mode == InsertMode::Overwrite {
        let c = overwrite_named(project, track, at, duration, body, id, right)?;
        let split = Some(c.echo["split"].clone()).filter(|s| !s.is_null());
        return Ok((c.ops, split));
    }
    let insert = ItemInsert {
        id: id.to_string(),
        track: track.to_string(),
        at,
        duration,
        mode,
        body,
    };
    Ok((vec![insert.into()], None))
}

/// The audio track for the linked audio: `named` (an audio track) or the timeline's first.
fn audio_track_for(
    project: &Project,
    timeline: usize,
    named: Option<&str>,
) -> Result<String, CoreError> {
    if let Some(named) = named {
        let t = track_index(project, named)?.get(project);
        if t.kind != TrackKind::Audio {
            return Err(CoreError::new(
                "TRACK_KIND_MISMATCH",
                format!(
                    "track {named} is a {} track, not an audio track",
                    kind_name(t.kind)
                ),
            )
            .with_details(json!({ "track": named, "trackKind": kind_name(t.kind) }))
            .with_fix("name an audio track for the clip's audio"));
        }
        return Ok(t.id.clone());
    }
    project.timelines[timeline]
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Audio)
        .map(|t| t.id.clone())
        .ok_or_else(|| {
            CoreError::new(
                "USAGE",
                "the asset has audio and the timeline has no audio track for it",
            )
            .with_details(json!({ "timeline": project.timelines[timeline].id }))
            .with_fix("add an audio track first, or insert the video alone with --no-audio")
        })
}
