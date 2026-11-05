//! The state of a branch at a past rev, from the database. Single-branch (4B Task 7 makes it
//! lineage-aware).
//!
//! Two ways there: replay forward from the latest snapshot at or below the rev, or (for a
//! commit, which holds the head) rewind from the head through the stored inverses of the
//! records after the rev. [`state_near_head`] takes whichever touches fewer records.

use fin_core::{ApplyMode, MemIds, Project, Tx, apply_tx_mode};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::db::{self, to_i64, to_u64};

/// The state of `branch` after `seq`, replayed from its latest snapshot at or below `seq`.
/// `DB_ERROR` when there is none.
pub(crate) fn state_at(conn: &Connection, branch: &str, seq: u64) -> Result<Project, StoreError> {
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT seq, json FROM snapshots WHERE branch = ?1 AND seq <= ?2
             ORDER BY seq DESC LIMIT 1",
            params![branch, to_i64(seq)?],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((base, json)) = row else {
        return Err(fin_core::CoreError::new(
            "DB_ERROR",
            format!("branch {branch:?} has no snapshot at or below seq {seq}"),
        )
        .into());
    };
    let mut project = fin_core::upcast_snapshot(serde_json::from_str(&json)?)?;
    for row in db::record_rows(conn, branch, to_u64(base)?)? {
        if row.0 > seq {
            break;
        }
        project = db::replay_row(&project, branch, &row)?.project;
    }
    Ok(project)
}

/// The state of `branch` after `seq`, given its head `head` at `head_rev`: rewound from the
/// head through the stored inverses of the `head_rev - seq` records after `seq` when that is
/// no more than the records a replay from the latest snapshot at or below `seq` takes
/// ([`state_at`] otherwise, and whenever a rewind does not go through, so damage is reported
/// by the replay). Inverses are applied with structural checks only, as replay applies
/// intents: what was committed is not re-judged.
pub(crate) fn state_near_head(
    conn: &Connection,
    branch: &str,
    seq: u64,
    head_rev: u64,
    head: &Project,
) -> Result<Project, StoreError> {
    let snap: Option<i64> = conn.query_row(
        "SELECT MAX(seq) FROM snapshots WHERE branch = ?1 AND seq <= ?2",
        params![branch, to_i64(seq)?],
        |r| r.get(0),
    )?;
    let replay = snap.map(to_u64).transpose()?.map(|s| seq - s);
    let rewind = head_rev.saturating_sub(seq);
    if seq <= head_rev
        && replay.is_none_or(|r| rewind <= r)
        && let Some(p) = rewind_to(conn, branch, seq, head_rev, head)?
    {
        return Ok(p);
    }
    state_at(conn, branch, seq)
}

/// `head` (at `head_rev`) with the inverses of the records after `seq` applied, newest first.
/// `None` when the rows are not exactly `seq + 1 ..= head_rev` or an inverse does not apply.
fn rewind_to(
    conn: &Connection,
    branch: &str,
    seq: u64,
    head_rev: u64,
    head: &Project,
) -> Result<Option<Project>, StoreError> {
    let rows = db::record_rows(conn, branch, seq)?;
    let seqs: Vec<u64> = rows.iter().map(|r| r.0).collect();
    if !seqs.iter().copied().eq(seq + 1..=head_rev) {
        return Ok(None);
    }
    let mut project = head.clone();
    for (row_seq, _, bytes) in rows.iter().rev() {
        let Ok(record) = db::parse_record(Some(*row_seq), bytes) else {
            return Ok(None);
        };
        let back = Tx {
            ops: record.inverse,
            ..record.intent
        };
        match apply_tx_mode(&project, &back, &mut MemIds::new(), ApplyMode::Replay) {
            Ok(a) => project = a.project,
            Err(_) => return Ok(None),
        }
    }
    Ok(Some(project))
}

#[cfg(test)]
mod tests {
    use fin_core::{Edge, Op, Rational, SCHEMA_VERSION, Tx, VideoInfo};

    use super::*;
    use crate::{MAIN, Store};

    fn secs(n: i64) -> Rational {
        Rational::from_int(n)
    }

    fn tx(id: &str, base_rev: u64, ops: Vec<Op>) -> Tx {
        Tx {
            tx_id: id.into(),
            author: "test".into(),
            branch: MAIN.into(),
            base_rev,
            schema_version: SCHEMA_VERSION,
            args_hash: None,
            ops,
        }
    }

    fn insert(id: &str, at: i64, duration: i64) -> Op {
        Op::ClipInsert {
            id: id.into(),
            track: "t1".into(),
            at: secs(at),
            asset: "a1".into(),
            source_in: secs(0),
            duration: secs(duration),
        }
    }

    /// Rewinding from the head and replaying from a snapshot give the same state at every rev.
    #[test]
    fn rewind_from_head_equals_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, dir) = (tmp.path().join("data"), tmp.path().join("p"));
        let mut s = Store::create(&data, &dir, "Demo", secs(30), None).unwrap();
        let asset = Op::AssetAdd {
            id: "$a".into(),
            path: "/media/a.mov".into(),
            duration: secs(60),
            video: Some(VideoInfo {
                width: 1920,
                height: 1080,
                rate: secs(30),
            }),
            audio: None,
            fingerprint: "fp".into(),
        };
        let edits = vec![
            vec![asset],
            vec![insert("$c", 0, 5), insert("$d", 5, 3)],
            vec![Op::ClipTrim {
                clip: "c1".into(),
                edge: Edge::Start,
                delta: secs(1),
            }],
            vec![Op::ClipDelete { clip: "c2".into() }],
            vec![insert("$e", 0, 2)],
            vec![Op::ClipTrim {
                clip: "c3".into(),
                edge: Edge::End,
                delta: secs(4),
            }],
        ];
        for (i, ops) in edits.into_iter().enumerate() {
            s.commit(tx(&format!("tx-{i}"), i as u64, ops)).unwrap();
        }
        let (head_rev, head) = s.snapshot(MAIN).unwrap();
        for seq in 0..=head_rev {
            let replayed = state_at(&s.conn, MAIN, seq).unwrap();
            let rewound = rewind_to(&s.conn, MAIN, seq, head_rev, &head).unwrap();
            assert_eq!(rewound.as_ref(), Some(&replayed), "seq {seq}");
            let near = state_near_head(&s.conn, MAIN, seq, head_rev, &head).unwrap();
            assert_eq!(near, replayed, "seq {seq}");
        }
        // Rows that do not reach the given head are not rewound.
        assert_eq!(
            rewind_to(&s.conn, MAIN, 2, head_rev + 1, &head).unwrap(),
            None
        );
    }
}
