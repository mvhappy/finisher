//! `asset.reprobe` (schema 5, plan 6B-1 Task 11): a media asset's `video` and `audio` replaced
//! by a fresh probe's, with the previous values as the inverse.

mod common;

use common::stale::{conflict_reason, rebase_over};
use common::*;
use fin_core::rebase::{Touch, footprint, split_diff};
use fin_core::{
    ApplyMode, AssetAdd, AssetKind, AssetReprobe, AudioInfo, Change, ChangeKind, CoreError,
    DisplaySize, EntityKind, ImageInfo, MemIds, Op, Project, Rotation, SCHEMA_VERSION, VideoInfo,
    apply_tx_mode, encode_record, upcast_record,
};

fn portrait() -> VideoInfo {
    VideoInfo {
        display: Some(DisplaySize {
            width: 1080,
            height: 1920,
        }),
        rotation: Some(Rotation::R90),
        pixel_aspect: Some(secs(1)),
        ..VideoInfo::coded(1920, 1080, secs(30))
    }
}

fn stereo() -> AudioInfo {
    AudioInfo {
        sample_rate: 48_000,
        channels: 2,
    }
}

fn reprobe(asset: &str, video: Option<VideoInfo>, audio: Option<AudioInfo>) -> Op {
    AssetReprobe {
        asset: asset.into(),
        video,
        audio,
    }
    .into()
}

fn param(id: &str, path: &str) -> Change {
    Change {
        kind: EntityKind::Asset,
        id: id.into(),
        change: ChangeKind::Updated,
        track: None,
        edge: None,
        by: None,
        path: Some(path.into()),
        op: 0,
        timeline: None,
    }
}

fn code<T: std::fmt::Debug>(r: Result<T, CoreError>) -> String {
    r.unwrap_err().code
}

/// A project with asset `a1` (video and audio, display unknown) shown by a 4 s clip on `t1`.
fn base() -> Project {
    with_clips(&[4])
}

#[test]
fn reprobe_sets_the_streams_and_records_both_params() {
    let p = base();
    assert_eq!(p.assets["a1"].video.as_ref().unwrap().display, None);
    let op = reprobe("a1", Some(portrait()), Some(stereo()));
    let a = golden_apply(&p, vec![op.clone()]);
    let asset = &a.project.assets["a1"];
    assert_eq!(asset.video, Some(portrait()));
    assert_eq!(asset.audio, Some(stereo()));
    // Nothing else of the asset changes.
    let mut same = asset.clone();
    same.video = p.assets["a1"].video.clone();
    assert_eq!(same, p.assets["a1"]);
    assert_eq!(a.diff.changes, [param("a1", "video"), param("a1", "audio")]);
    let f = footprint(
        &op,
        &split_diff(std::slice::from_ref(&op), &a.diff).unwrap()[0],
    );
    assert_eq!(
        f.touches,
        [
            ("a1".to_string(), Touch::Param("video".into())),
            ("a1".to_string(), Touch::Param("audio".into())),
        ]
    );
    assert!(f.tracks.is_empty() && f.uses.is_empty());
}

#[test]
fn reprobe_inverse_restores() {
    let p = base();
    let before = p.assets["a1"].clone();
    let a = golden_apply(&p, vec![reprobe("a1", Some(portrait()), None)]);
    assert_eq!(
        a.project.assets["a1"].audio, None,
        "an absent stream is cleared"
    );
    assert_eq!(
        a.inverse,
        [reprobe("a1", before.video.clone(), before.audio.clone())]
    );
    assert_eq!(undo(&a).assets["a1"], before);
    // The inverse replays as history does.
    let back = apply_tx_mode(
        &a.project,
        &tx(a.inverse.clone()),
        &mut MemIds::after(&a.project),
        ApplyMode::Replay,
    )
    .unwrap();
    assert_eq!(back.project, p);
}

#[test]
fn reprobe_refusals() {
    let p = base();
    assert_eq!(
        code(try_apply(&p, vec![reprobe("a9", Some(portrait()), None)])),
        "NOT_FOUND"
    );
    // A media asset keeps a stream (policy).
    assert_eq!(
        code(try_apply(&p, vec![reprobe("a1", None, None)])),
        "INVALID_OP"
    );
    // The clip on the video track keeps its video.
    let e = try_apply(&p, vec![reprobe("a1", None, Some(stereo()))]).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["track"]),
        ("TRACK_KIND_MISMATCH", &"t1".into())
    );
    // Only media assets are reprobed.
    let image = AssetAdd {
        image: Some(ImageInfo {
            width: 8,
            height: 8,
        }),
        ..AssetAdd::media("$i", "/media/i.png", secs(1), None, None, "fp")
    };
    let image = AssetAdd {
        kind: AssetKind::Image,
        duration: None,
        ..image
    };
    let with_image = apply(&p, vec![image.into()]).project;
    let e = try_apply(&with_image, vec![reprobe("a2", Some(portrait()), None)]).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["kind"]),
        ("INVALID_OP", &"image".into())
    );
}

#[test]
fn stale_reprobe_of_a_removed_asset_conflicts() {
    let p = apply(&project(), vec![av_asset("$a", secs(10))]).project;
    let e = rebase_over(
        &p,
        vec![Op::AssetRemove { id: "a1".into() }],
        vec![reprobe("a1", Some(portrait()), Some(stereo()))],
    )
    .unwrap_err();
    assert_eq!(conflict_reason(&e), "REMOVED");
}

/// A committed record holding the op is stamped with the current schema (5), and reads back
/// as it was written.
#[test]
fn a_reprobe_record_is_schema_5_and_round_trips() {
    let p = base();
    let t = tx(vec![reprobe("a1", Some(portrait()), Some(stereo()))]);
    let a = apply(&p, t.ops.clone());
    let record = fin_core::Record::new_edit(
        "tx-test".into(),
        1,
        t.schema_version,
        None,
        "test".into(),
        1_700_000_000_000,
        t.clone(),
        t.ops.clone(),
        a.inverse.clone(),
        Default::default(),
    );
    let bytes = encode_record(&record);
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!((v["schemaVersion"].as_u64(), SCHEMA_VERSION), (Some(5), 5));
    assert_eq!(
        v["resolved"][0]["video"],
        serde_json::json!({ "width": 1920, "height": 1080, "rate": "30", "rotation": 90,
                            "display": { "width": 1080, "height": 1920 }, "pixelAspect": "1" })
    );
    let back = upcast_record(v.clone()).unwrap();
    assert_eq!(encode_record(&back), bytes);
    // A newer record than this reader is refused, never read without its fields.
    let mut newer = v;
    newer["schemaVersion"] = (SCHEMA_VERSION + 1).into();
    assert_eq!(code(upcast_record(newer)), "NEWER_FORMAT");
}
