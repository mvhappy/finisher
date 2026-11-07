//! `item.move`, `item.slide`, `item.link`/`item.unlink` and link-group behaviour of the
//! structural ops (plan 4C Task 9, Decisions 15, 19 and 23).

mod common;

use common::ids_json::ids_cover_every_id_in_json;
use common::moves::*;
use common::*;
use fin_core::rebase::{footprint, split_diff};
use fin_core::{
    Change, ChangeKind, DeleteMode, Edge, EntityKind, InsertMode, ItemRemove, ItemRestore,
    ItemSlide, ItemTrim, Op, Rational, Shift, SlideSides, TrimMode,
};

fn shift(track: &str, from: i64, by: i64) -> Shift {
    Shift {
        track: track.into(),
        from: secs(from),
        by: secs(by),
        op: 0,
    }
}

fn moved(id: &str, track: &str) -> Change {
    Change {
        kind: EntityKind::Item,
        id: id.into(),
        change: ChangeKind::Updated,
        op: 0,
        track: Some(track.into()),
        edge: None,
        path: Some("start".into()),
        by: None,
    }
}

fn edge(id: &str, track: &str, edge: Edge, by: Option<i64>) -> Change {
    Change {
        edge: Some(edge),
        path: None,
        by: by.map(secs),
        ..moved(id, track)
    }
}

fn code(r: Result<fin_core::Applied, fin_core::CoreError>) -> String {
    r.unwrap_err().code
}

#[test]
fn ripple_move_within_a_track() {
    let p = with_clips(&[2, 3, 4]);
    let a = golden_apply(&p, vec![move_to("c1", "t1", secs(5), InsertMode::Ripple)]);
    assert_eq!(
        spans_on(&a.project, 0),
        [sp("c2", 0, 3), sp("c1", 3, 2), sp("c3", 5, 4)]
    );
    // A close at the end it left, then an open at the target mapped through the close.
    assert_eq!(a.diff.shifts, [shift("t1", 2, -2), shift("t1", 3, 2)]);
    assert_eq!(a.diff.changes, [moved("c1", "t1")]);
    let before = p.timelines[0].tracks[0].items[0].clone();
    let want: Vec<Op> = vec![
        ItemRemove {
            id: "c1".into(),
            mode: DeleteMode::Ripple,
        }
        .into(),
        ItemRestore {
            item: before,
            track: "t1".into(),
            mode: DeleteMode::Ripple,
        }
        .into(),
    ];
    assert_eq!(a.inverse, want);
    // Earlier on the track too.
    let a = golden_apply(&p, vec![move_to("c3", "t1", secs(2), InsertMode::Ripple)]);
    assert_eq!(
        spans_on(&a.project, 0),
        [sp("c1", 0, 2), sp("c3", 2, 4), sp("c2", 6, 3)]
    );
}

#[test]
fn ripple_move_across_tracks() {
    let p = with_v2(with_clips(&[2, 3]));
    let a = golden_apply(&p, vec![move_to("c1", "t3", secs(0), InsertMode::Ripple)]);
    assert_eq!(spans_on(&a.project, 0), [sp("c2", 0, 3)]);
    assert_eq!(spans_on(&a.project, 2), [sp("c1", 0, 2)]);
    assert_eq!(a.diff.shifts, [shift("t1", 2, -2), shift("t3", 0, 2)]);
    // The item is recorded on the track it left and the one it is on.
    assert_eq!(a.diff.changes, [moved("c1", "t1"), moved("c1", "t3")]);
}

#[test]
fn overwrite_move_carves_the_target() {
    let p = with_clips(&[2, 3, 4]);
    let a = golden_apply(
        &p,
        vec![move_to("c1", "t1", secs(5), InsertMode::Overwrite)],
    );
    assert_eq!(
        spans_on(&a.project, 0),
        [sp("c2", 2, 3), sp("c1", 5, 2), sp("c3", 7, 2)]
    );
    assert!(a.diff.shifts.is_empty());
    assert_eq!(
        a.diff.changes,
        [edge("c3", "t1", Edge::Start, Some(-2)), moved("c1", "t1")]
    );
    let p = with_v2(with_clips(&[2, 3]));
    let a = golden_apply(
        &p,
        vec![move_to("c2", "t3", secs(1), InsertMode::Overwrite)],
    );
    assert_eq!(spans_on(&a.project, 0), [sp("c1", 0, 2)]);
    assert_eq!(spans_on(&a.project, 2), [sp("c2", 1, 3)]);
}

#[test]
fn move_into_itself_is_refused() {
    let p = with_clips(&[10, 2]);
    let op = move_to("c1", "t1", secs(4), InsertMode::Ripple);
    assert_eq!(code(try_apply(&p, vec![op])), "MOVE_INTO_ITSELF");
    // Its own edges are no-ops, and an overwrite may overlap its old range.
    golden_apply(&p, vec![move_to("c1", "t1", secs(10), InsertMode::Ripple)]);
    let a = golden_apply(
        &p,
        vec![move_to("c1", "t1", secs(1), InsertMode::Overwrite)],
    );
    assert_eq!(spans_on(&a.project, 0), [sp("c1", 1, 10), sp("c2", 11, 1)]);
}

#[test]
fn move_checks_track_kind_and_timeline() {
    // An image item cannot go on an audio track; a clip needs the track's stream.
    let p = apply(
        &with_clips(&[2]),
        vec![
            audio_asset("$b", secs(10)),
            insert("$s", "t2", secs(0), "$b", secs(0), secs(2)),
        ],
    )
    .project;
    let to_video = move_to("c2", "t1", secs(2), InsertMode::Ripple);
    assert_eq!(code(try_apply(&p, vec![to_video])), "TRACK_KIND_MISMATCH");
}

#[test]
fn slide_trims_both_neighbours() {
    let p = with_sourced(&[(10, 10), (10, 10), (10, 10)]);
    let a = golden_apply(&p, vec![slide("c2", secs(2))]);
    assert_eq!(
        spans_on(&a.project, 0),
        [sp("c1", 0, 12), sp("c2", 12, 10), sp("c3", 22, 8)]
    );
    assert_eq!(clip(&a.project, "c3").source_in, secs(12));
    assert_eq!(
        clip(&a.project, "c2").source_in,
        secs(10),
        "content moves with it"
    );
    assert!(a.diff.shifts.is_empty());
    assert_eq!(
        a.diff.changes,
        [
            moved("c2", "t1"),
            edge("c1", "t1", Edge::End, None),
            edge("c3", "t1", Edge::Start, Some(-2)),
        ]
    );
    let back: Op = ItemSlide {
        item: "c2".into(),
        delta: secs(-2),
        sides: Some(SlideSides {
            before: Some("c1".into()),
            after: Some("c3".into()),
        }),
    }
    .into();
    assert_eq!(a.inverse, std::slice::from_ref(&back));
    ids_cover_every_id_in_json(&back);
    let a = golden_apply(&p, vec![slide("c2", secs(-3))]);
    assert_eq!(
        spans_on(&a.project, 0),
        [sp("c1", 0, 7), sp("c2", 7, 10), sp("c3", 17, 13)]
    );
}

#[test]
fn slide_blocked_by_gap() {
    // c1 [0,10), a 2 s gap, c3 [12,20).
    let p = apply(&with_clips(&[10, 2, 8]), vec![lift("c2")]).project;
    assert_eq!(
        code(try_apply(&p, vec![slide("c3", secs(-3))])),
        "SLIDE_BLOCKED"
    );
    // Into the gap exactly: c3 then touches c1, and its undo trims nothing.
    let a = golden_apply(&p, vec![slide("c3", secs(-2))]);
    assert_eq!(spans_on(&a.project, 0), [sp("c1", 0, 10), sp("c3", 10, 8)]);
    assert_eq!(
        a.inverse,
        [ItemSlide {
            item: "c3".into(),
            delta: secs(2),
            sides: Some(SlideSides {
                before: None,
                after: None
            }),
        }
        .into()]
    );
    // A client may not name the sides.
    let named: Op = ItemSlide {
        item: "c3".into(),
        delta: secs(-1),
        sides: Some(SlideSides {
            before: None,
            after: None,
        }),
    }
    .into();
    assert_eq!(code(try_apply(&p, vec![named])), "INVALID_OP");
}

fn lift(item: &str) -> Op {
    fin_core::ItemDelete {
        item: item.into(),
        mode: DeleteMode::Lift,
    }
    .into()
}

#[test]
fn link_unlink_inverse_exact() {
    let p = with_clips(&[2, 3, 4]);
    let a = golden_apply(&p, vec![link(&["c1", "c3"], "$g")]);
    assert_eq!(a.bindings["$g"], "lk1");
    let items = &a.project.timelines[0].tracks[0].items;
    assert_eq!(items[0].common.link.as_deref(), Some("lk1"));
    assert_eq!(items[1].common.link, None);
    assert_eq!(items[2].common.link.as_deref(), Some("lk1"));
    assert_eq!(a.diff.changes[0].kind, EntityKind::Link);
    let linked = a.project;
    let u = golden_apply(&linked, vec![unlink("lk1")]);
    assert!(
        u.project.timelines[0].tracks[0]
            .items
            .iter()
            .all(|c| c.common.link.is_none())
    );
    assert_eq!(u.inverse, [link(&["c1", "c3"], "lk1")]);
    assert_eq!(
        code(try_apply(&linked, vec![link(&["c3", "c2"], "$h")])),
        "ALREADY_LINKED"
    );
    assert_eq!(code(try_apply(&p, vec![link(&["c1"], "$h")])), "INVALID_OP");
    assert_eq!(
        code(try_apply(&p, vec![link(&["c1", "c1"], "$h")])),
        "INVALID_OP"
    );
    // The literal-id link is the internal inverse only.
    assert_eq!(
        code(try_apply(&p, vec![link(&["c1", "c2"], "lk7")])),
        "INVALID_OP"
    );
}

#[test]
fn linked_move_moves_members_same_delta() {
    let p = linked(10);
    let a = golden_apply(&p, vec![move_to("c1", "t1", secs(8), InsertMode::Ripple)]);
    assert_eq!(spans_on(&a.project, 0), [sp("c3", 0, 4), sp("c1", 4, 4)]);
    assert_eq!(spans_on(&a.project, 1), [sp("c4", 0, 4), sp("c2", 4, 4)]);
    assert_eq!(
        a.diff.shifts,
        [
            shift("t1", 4, -4),
            shift("t1", 4, 4),
            shift("t2", 4, -4),
            shift("t2", 4, 4)
        ]
    );
    assert_eq!(a.diff.changes, [moved("c1", "t1"), moved("c2", "t2")]);
    // Only the named item changes track.
    let a = golden_apply(
        &p,
        vec![move_to("c1", "t3", secs(2), InsertMode::Overwrite)],
    );
    assert_eq!(spans_on(&a.project, 2), [sp("c1", 2, 4)]);
    assert_eq!(spans_on(&a.project, 1), [sp("c2", 2, 4), sp("c4", 6, 2)]);
}

#[test]
fn linked_trim_is_all_or_nothing() {
    // c2's audio asset is 5 s long: extending the group's end by 3 s leaves its source.
    let p = linked(5);
    let ext = trim("c1", Edge::End, secs(3));
    assert_eq!(code(try_apply(&p, vec![ext])), "INVALID_RANGE");
    let a = golden_apply(&p, vec![trim("c1", Edge::End, secs(-1))]);
    assert_eq!(spans_on(&a.project, 0), [sp("c1", 0, 3), sp("c3", 3, 4)]);
    assert_eq!(spans_on(&a.project, 1), [sp("c2", 0, 3), sp("c4", 3, 4)]);
    // The inverse trims each member back alone, the last member first.
    let back = |item: &str| -> Op {
        ItemTrim {
            item: item.into(),
            edge: Edge::End,
            delta: secs(1),
            mode: TrimMode::Ripple,
            alone: true,
        }
        .into()
    };
    assert_eq!(a.inverse, [back("c2"), back("c1")]);
    // Rolls and overwrites act on the group too.
    let a = golden_apply(&p, vec![trim_in("c1", Edge::End, secs(1), TrimMode::Roll)]);
    assert_eq!(spans_on(&a.project, 1), [sp("c2", 0, 5), sp("c4", 5, 3)]);
    let a = golden_apply(
        &p,
        vec![trim_in("c2", Edge::End, secs(1), TrimMode::Overwrite)],
    );
    assert_eq!(spans_on(&a.project, 0), [sp("c1", 0, 5), sp("c3", 5, 3)]);
    // A client may not trim one member alone.
    let alone: Op = ItemTrim {
        item: "c1".into(),
        edge: Edge::End,
        delta: secs(-1),
        mode: TrimMode::Ripple,
        alone: true,
    }
    .into();
    assert_eq!(code(try_apply(&p, vec![alone])), "INVALID_OP");
}

fn trim_in(item: &str, edge: Edge, delta: Rational, mode: TrimMode) -> Op {
    ItemTrim {
        item: item.into(),
        edge,
        delta,
        mode,
        alone: false,
    }
    .into()
}

#[test]
fn linked_delete_ripples_each_track() {
    let p = linked(10);
    let a = golden_apply(&p, vec![delete("c1")]);
    assert_eq!(spans_on(&a.project, 0), [sp("c3", 0, 4)]);
    assert_eq!(spans_on(&a.project, 1), [sp("c4", 0, 4)]);
    assert_eq!(a.diff.shifts, [shift("t1", 4, -4), shift("t2", 4, -4)]);
    assert_eq!(a.inverse.len(), 2);
    // Lift too.
    let a = golden_apply(&p, vec![lift("c2")]);
    assert_eq!(spans_on(&a.project, 0), [sp("c3", 4, 4)]);
    assert!(a.diff.shifts.is_empty());
}

#[test]
fn linked_delete_footprint_covers_members() {
    let p = linked(10);
    let op = delete("c1");
    let a = apply(&p, vec![op.clone()]);
    let d = split_diff(std::slice::from_ref(&op), &a.diff).unwrap();
    let f = footprint(&op, &d[0]);
    assert_eq!(f.removes, ["c1", "c2"]);
    assert_eq!(
        f.tracks.iter().map(String::as_str).collect::<Vec<_>>(),
        ["t1", "t2"]
    );
}

#[test]
fn slip_slide_and_split_act_on_the_named_item() {
    let p = linked(10);
    let a = golden_apply(
        &p,
        vec![
            fin_core::ItemSplit {
                item: "c1".into(),
                offset: secs(1),
                id: "$r".into(),
            }
            .into(),
        ],
    );
    assert_eq!(spans_on(&a.project, 1), spans_on(&p, 1));
    let a = golden_apply(&p, vec![slide("c3", secs(-1))]);
    assert_eq!(spans_on(&a.project, 1), spans_on(&p, 1));
}

#[test]
fn moves_and_links_list_their_ids() {
    for op in [
        move_to("c1", "t3", secs(1), InsertMode::Ripple),
        slide("c1", secs(1)),
        link(&["c1", "$c"], "$lk"),
        unlink("lk1"),
    ] {
        ids_cover_every_id_in_json(&op);
    }
    assert_eq!(link(&["c1", "c2"], "$g").ids(), ["c1", "c2", "$g"]);
    let _ = Rational::zero();
}
