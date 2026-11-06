//! Per-op diffs (plan 4C Decision 4): every change and shift names its op, `split_diff` cuts a
//! diff by it, and a footprint adds what the op's own diff records (Decision 20).

mod common;

use common::*;
use fin_core::rebase::{Touch, footprint, split_diff};
use fin_core::{Change, ChangeKind, Diff, Edge, EntityKind, Shift};

/// A change; an item's is on `t1`.
fn change(kind: EntityKind, id: &str, change: ChangeKind, op: u32) -> Change {
    Change {
        kind,
        id: id.into(),
        change,
        op,
        track: (kind == EntityKind::Item).then(|| "t1".to_string()),
        edge: None,
        path: None,
        by: None,
    }
}

fn shift(track: &str, from: i64, by: i64, op: u32) -> Shift {
    Shift {
        track: track.into(),
        from: secs(from),
        by: secs(by),
        op,
    }
}

/// A tx of `asset.add`, two inserts and a start trim: each entry carries its op's index, and
/// the trim's change its edge and content offset.
#[test]
fn ops_record_their_own_index() {
    let a = apply(
        &project(),
        vec![
            av_asset("$a", secs(100)),
            insert("$c1", "t1", secs(0), "$a", secs(0), secs(5)),
            insert("$c2", "t1", secs(5), "$a", secs(0), secs(4)),
            trim("$c1", Edge::Start, secs(2)),
        ],
    );
    let trimmed = Change {
        edge: Some(Edge::Start),
        by: Some(secs(-2)),
        ..change(EntityKind::Item, "c1", ChangeKind::Updated, 3)
    };
    assert_eq!(
        a.diff.changes,
        [
            change(EntityKind::Asset, "a1", ChangeKind::Added, 0),
            change(EntityKind::Item, "c1", ChangeKind::Added, 1),
            change(EntityKind::Item, "c2", ChangeKind::Added, 2),
            trimmed,
        ]
    );
    assert_eq!(
        a.diff.shifts,
        [
            shift("t1", 0, 5, 1),
            shift("t1", 5, 4, 2),
            shift("t1", 5, -2, 3)
        ]
    );
    // The wire shape: `track` on item changes, `edge` and `by` only where set.
    let wire = serde_json::to_value(&a.diff).unwrap();
    assert_eq!(
        wire["changes"][3],
        serde_json::json!({ "kind": "item", "id": "c1", "change": "updated", "op": 3,
                            "track": "t1", "edge": "start", "by": "-2" })
    );
    assert_eq!(
        wire["changes"][0],
        serde_json::json!({ "kind": "asset", "id": "a1", "change": "added", "op": 0 })
    );
    // An end trim moves no content: no `by`.
    let end = apply(&a.project, vec![trim("c2", Edge::End, secs(-1))]);
    assert_eq!(end.diff.changes[0].edge, Some(Edge::End));
    assert_eq!(end.diff.changes[0].by, None);
}

#[test]
fn split_diff_by_op_index() {
    let p = with_clips(&[5, 5]);
    let ops = vec![
        av_asset("$b", secs(10)),
        trim("c1", Edge::End, secs(-1)),
        delete("c2"),
    ];
    let a = apply(&p, ops.clone());
    let diffs = split_diff(&ops, &a.diff).unwrap();
    assert_eq!(diffs.len(), 3);
    assert_eq!(diffs[0].changes, a.diff.changes[..1]);
    assert!(diffs[0].shifts.is_empty());
    assert_eq!(diffs[1].changes, a.diff.changes[1..2]);
    assert_eq!(diffs[1].shifts, a.diff.shifts[..1]);
    assert_eq!(diffs[2].changes, a.diff.changes[2..]);
    assert_eq!(diffs[2].shifts, a.diff.shifts[1..]);
    // Any number of entries per op, and ops with none.
    let many = Diff {
        changes: vec![
            change(EntityKind::Item, "c1", ChangeKind::Updated, 1),
            change(EntityKind::Item, "c2", ChangeKind::Removed, 1),
        ],
        shifts: vec![shift("t1", 0, 1, 1), shift("t2", 0, 1, 1)],
    };
    let diffs = split_diff(&ops, &many).unwrap();
    assert_eq!(diffs[0], Diff::default());
    assert_eq!(diffs[1], many);
    assert_eq!(diffs[2], Diff::default());
}

#[test]
fn split_diff_rejects_out_of_range_or_unordered_op() {
    let ops = vec![av_asset("$a", secs(10)), trim("c1", Edge::End, secs(1))];
    let past = Diff {
        changes: vec![change(EntityKind::Item, "c1", ChangeKind::Updated, 2)],
        shifts: vec![],
    };
    let e = split_diff(&ops, &past).unwrap_err();
    assert_eq!(e.code, "INVALID_FORMAT");
    assert_eq!(e.details["kind"], "diff");
    let shift_past = Diff {
        changes: vec![],
        shifts: vec![shift("t1", 0, 1, 2)],
    };
    assert_eq!(
        split_diff(&ops, &shift_past).unwrap_err().code,
        "INVALID_FORMAT"
    );
    let unordered = Diff {
        changes: vec![
            change(EntityKind::Item, "c1", ChangeKind::Updated, 1),
            change(EntityKind::Asset, "a1", ChangeKind::Added, 0),
        ],
        shifts: vec![],
    };
    assert_eq!(
        split_diff(&ops, &unordered).unwrap_err().code,
        "INVALID_FORMAT"
    );
    let unordered_shifts = Diff {
        changes: vec![],
        shifts: vec![shift("t1", 0, 1, 1), shift("t1", 0, 1, 0)],
    };
    assert_eq!(
        split_diff(&ops, &unordered_shifts).unwrap_err().code,
        "INVALID_FORMAT"
    );
    // No ops: only an empty diff splits.
    assert_eq!(
        split_diff(&[], &Diff::default()).unwrap(),
        Vec::<Diff>::new()
    );
    assert!(split_diff(&[], &past).is_err());
}

/// What the op's own diff records joins its footprint: removed → removes, added → creates,
/// an edge → `Touch::Edge`, a path → `Touch::Param`, any other update → `Touch::Exists`; what
/// the op's rules already list is not repeated.
#[test]
fn with_changes_maps_removed_added_edge_path() {
    let op = trim("c1", Edge::End, secs(1));
    let own = Diff {
        changes: vec![
            Change {
                edge: Some(Edge::End),
                ..change(EntityKind::Item, "c1", ChangeKind::Updated, 0)
            },
            change(EntityKind::Item, "c7", ChangeKind::Removed, 0),
            change(EntityKind::Item, "c8", ChangeKind::Added, 0),
            Change {
                edge: Some(Edge::Start),
                by: Some(secs(-1)),
                ..change(EntityKind::Item, "c9", ChangeKind::Updated, 0)
            },
            Change {
                path: Some("transform.scale".into()),
                ..change(EntityKind::Item, "c10", ChangeKind::Updated, 0)
            },
            change(EntityKind::Item, "c11", ChangeKind::Updated, 0),
            Change {
                track: Some("t4".into()),
                ..change(EntityKind::Item, "c11", ChangeKind::Updated, 0)
            },
        ],
        shifts: vec![shift("t1", 5, 1, 0)],
    };
    let f = footprint(&op, &own);
    assert_eq!(
        f.touches,
        [
            ("c1".to_string(), Touch::Edge(Edge::End)),
            ("c9".to_string(), Touch::Edge(Edge::Start)),
            ("c10".to_string(), Touch::Param("transform.scale".into())),
            ("c11".to_string(), Touch::Exists),
        ]
    );
    assert_eq!(f.removes, ["c7"]);
    assert_eq!(f.creates, ["c8"]);
    // Tracks come from the shifts and from the changes' tracks.
    assert_eq!(
        f.tracks.iter().map(String::as_str).collect::<Vec<_>>(),
        ["t1", "t4"]
    );
    // A committed insert's own Added change names the id it already creates: once.
    let a = apply(
        &with_clips(&[5]),
        vec![insert("$c", "t1", secs(5), "a1", secs(0), secs(2))],
    );
    let resolved = insert("c2", "t1", secs(5), "a1", secs(0), secs(2));
    assert_eq!(footprint(&resolved, &a.diff).creates, ["c2"]);
}
