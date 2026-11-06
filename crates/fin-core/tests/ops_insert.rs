mod common;

use common::*;
use fin_core::{ChangeKind, MemIds, Op, Shift, apply_tx};
use serde_json::json;

#[test]
fn add_and_insert_in_one_tx_bind_placeholders() {
    let p = project();
    let a = apply(
        &p,
        vec![
            av_asset("$a", secs(10)),
            insert("$c", "t1", secs(0), "$a", secs(1), secs(4)),
        ],
    );
    assert_eq!(a.bindings.get("$a").map(String::as_str), Some("a1"));
    assert_eq!(a.bindings.get("$c").map(String::as_str), Some("c1"));
    let c = clip(&a.project, "c1");
    assert_eq!(
        (c.asset.as_str(), c.start, c.source_in, c.duration),
        ("a1", secs(0), secs(1), secs(4))
    );
    assert_eq!(a.project.assets["a1"].path, "/media/a.mov");
    let changes: Vec<_> = a
        .diff
        .changes
        .iter()
        .map(|c| (c.id.as_str(), c.change))
        .collect();
    assert_eq!(
        changes,
        [("a1", ChangeKind::Added), ("c1", ChangeKind::Added)]
    );
    assert_eq!(
        a.inverse,
        [
            Op::ClipRemove {
                id: "c1".into(),
                ripple: true
            },
            Op::AssetRemove { id: "a1".into() }
        ]
    );
}

#[test]
fn ops_use_the_wire_shape() {
    let op = insert("$c", "t1", r("1/2"), "a1", secs(0), secs(3));
    let v = serde_json::to_value(&op).unwrap();
    assert_eq!(
        v,
        json!({"op": "clip.insert", "id": "$c", "track": "t1", "at": "1/2", "asset": "a1", "sourceIn": "0", "duration": "3"})
    );
    assert_eq!(serde_json::from_value::<Op>(v).unwrap(), op);
    assert_eq!(
        serde_json::to_value(av_asset("$a", secs(2))).unwrap()["op"],
        "asset.add"
    );
}

#[test]
fn insert_at_track_end_appends() {
    let p = with_clips(&[5]);
    let a = apply(
        &p,
        vec![insert("$c", "t1", secs(5), "a1", secs(0), secs(2))],
    );
    assert_eq!(
        starts(&a.project, 0),
        [("c1".into(), secs(0)), ("c2".into(), secs(5))]
    );
    assert_eq!(
        a.diff.shifts,
        [Shift {
            track: "t1".into(),
            from: secs(5),
            by: secs(2),
            op: 0
        }]
    );
}

#[test]
fn insert_at_boundary_ripples_later_clip_by_duration() {
    let p = with_clips(&[5, 3]);
    let a = apply(
        &p,
        vec![insert("$c", "t1", secs(5), "a1", secs(0), r("5/2"))],
    );
    assert_eq!(
        starts(&a.project, 0),
        [
            ("c1".into(), secs(0)),
            ("c3".into(), secs(5)),
            ("c2".into(), r("15/2"))
        ]
    );
    assert_eq!(
        a.diff.shifts,
        [Shift {
            track: "t1".into(),
            from: secs(5),
            by: r("5/2"),
            op: 0
        }]
    );
    assert_well_formed(&a.project);
}

#[test]
fn insert_in_gap_after_end_is_allowed() {
    let p = with_clips(&[5]);
    let a = apply(
        &p,
        vec![insert("$c", "t1", secs(8), "a1", secs(0), secs(1))],
    );
    assert_eq!(clip(&a.project, "c2").start, secs(8));
}

#[test]
fn insert_inside_clip_is_rejected_with_fix() {
    let p = with_clips(&[5]);
    let e = try_apply(
        &p,
        vec![insert("$c", "t1", secs(4), "a1", secs(0), secs(1))],
    )
    .unwrap_err();
    assert_eq!(e.code, "INSERT_INSIDE_CLIP");
    assert_eq!(e.details["clip"], "c1");
    assert_eq!(e.details["nearest"], "5");
    assert_eq!(e.details["opIndex"], 0);
    assert!(
        e.fix.as_deref().unwrap().contains("nearest is 5"),
        "{:?}",
        e.fix
    );
}

#[test]
fn range_past_asset_end_is_invalid() {
    let p = with_clips(&[]);
    let e = try_apply(
        &p,
        vec![insert("$c", "t1", secs(0), "a1", secs(98), secs(3))],
    )
    .unwrap_err();
    assert_eq!(e.code, "INVALID_RANGE");
    assert_eq!(e.details["max"], "2");
}

#[test]
fn duration_below_one_frame_and_negative_at_are_invalid() {
    let p = with_clips(&[]);
    let e = try_apply(
        &p,
        vec![insert("$c", "t1", secs(0), "a1", secs(0), r("1/60"))],
    )
    .unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["min"]),
        ("INVALID_RANGE", &json!("1/30"))
    );
    let e = try_apply(
        &p,
        vec![insert("$c", "t1", secs(-1), "a1", secs(0), secs(1))],
    )
    .unwrap_err();
    assert_eq!(e.code, "INVALID_RANGE");
}

#[test]
fn audio_only_asset_on_video_track_mismatches() {
    let e = try_apply(
        &project(),
        vec![
            audio_asset("$a", secs(10)),
            insert("$c", "t1", secs(0), "$a", secs(0), secs(1)),
        ],
    )
    .unwrap_err();
    assert_eq!(e.code, "TRACK_KIND_MISMATCH");
    assert_eq!(e.details["opIndex"], 1);
    let ok = apply(
        &project(),
        vec![
            audio_asset("$a", secs(10)),
            insert("$c", "t2", secs(0), "$a", secs(0), secs(1)),
        ],
    );
    assert_eq!(ok.project.timeline.tracks[1].items.len(), 1);
}

#[test]
fn failing_second_op_leaves_project_unchanged() {
    let p = with_clips(&[5]);
    let before = p.clone();
    let e = try_apply(
        &p,
        vec![
            insert("$c", "t1", secs(5), "a1", secs(0), secs(1)),
            insert("$d", "t1", secs(1), "a1", secs(0), secs(1)),
        ],
    )
    .unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["opIndex"]),
        ("INSERT_INSIDE_CLIP", &json!(1))
    );
    assert_eq!(p, before);
}

#[test]
fn unknown_ids_and_placeholders_are_typed_errors() {
    let p = with_clips(&[]);
    let e = try_apply(
        &p,
        vec![insert("$c", "t9", secs(0), "a1", secs(0), secs(1))],
    )
    .unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["id"]),
        ("NOT_FOUND", &json!("t9"))
    );
    let e = try_apply(
        &p,
        vec![insert("$c", "t1", secs(0), "a7", secs(0), secs(1))],
    )
    .unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["id"]),
        ("NOT_FOUND", &json!("a7"))
    );
    let e = try_apply(
        &p,
        vec![insert("$c", "t1", secs(0), "$nope", secs(0), secs(1))],
    )
    .unwrap_err();
    assert_eq!(e.code, "UNBOUND_PLACEHOLDER");
    let dup = vec![
        insert("$c", "t1", secs(0), "a1", secs(0), secs(1)),
        insert("$c", "t1", secs(1), "a1", secs(0), secs(1)),
    ];
    assert_eq!(
        try_apply(&p, dup).unwrap_err().code,
        "DUPLICATE_PLACEHOLDER"
    );
}

#[test]
fn clients_cannot_send_internal_ops_or_literal_new_ids() {
    let p = with_clips(&[5]);
    let e = try_apply(
        &p,
        vec![Op::ClipRemove {
            id: "c1".into(),
            ripple: true,
        }],
    )
    .unwrap_err();
    assert_eq!(e.code, "INVALID_OP");
    let e = try_apply(
        &p,
        vec![insert("c9", "t1", secs(5), "a1", secs(0), secs(1))],
    )
    .unwrap_err();
    assert_eq!(e.code, "INVALID_OP");
}

#[test]
fn wrong_schema_version_is_rejected() {
    let p = project();
    let mut t = tx(vec![]);
    t.schema_version = 99;
    let e = apply_tx(&p, &t, &mut MemIds::after(&p)).unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["expected"]),
        ("SCHEMA_VERSION", &json!(1))
    );
}

#[test]
fn inverse_restores_the_original_project() {
    let p = with_clips(&[5, 3]);
    let a = apply(
        &p,
        vec![
            av_asset("$b", secs(20)),
            insert("$c", "t1", secs(5), "$b", secs(2), secs(4)),
        ],
    );
    assert_eq!(undo(&a), p);
}
