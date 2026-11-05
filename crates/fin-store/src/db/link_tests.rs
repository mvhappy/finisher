//! `record_links` rows of every 4B record kind, written by the local commit path
//! ([`insert_tx`]) and by an import ([`insert_imported`]).

use std::collections::BTreeMap;

use fin_core::{Diff, Link, SCHEMA_VERSION, Tx, encode_record};
use rusqlite::Connection;

use super::{SCHEMA, hash_bytes, insert_imported, insert_tx, new_record};

type Row = (
    String,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

fn cases() -> Vec<(Link, [Option<String>; 6])> {
    // Expected [origin, origin_hash, from_branch, merge_id, targets, name].
    let none = || None;
    vec![
        (
            Link::Undo {
                undoes: vec!["u-1".into(), "u-2".into(), "u-3".into()],
            },
            [
                none(),
                none(),
                none(),
                none(),
                s(r#"["u-1","u-2","u-3"]"#),
                none(),
            ],
        ),
        (
            Link::Redo {
                redoes: "r-1".into(),
            },
            [none(), none(), none(), none(), s(r#"["r-1"]"#), none()],
        ),
        (
            Link::Merge {
                merge_id: "m1-2".into(),
                from: "alt".into(),
                origin: "o-9".into(),
            },
            [s("o-9"), none(), s("alt"), s("m1-2"), none(), none()],
        ),
        (
            Link::CherryPick {
                origin: "o-5".into(),
                from: "alt".into(),
                origin_hash: s("blake3:abc"),
            },
            [s("o-5"), s("blake3:abc"), s("alt"), none(), none(), none()],
        ),
        (
            Link::Revert {
                reverts: "m1".into(),
            },
            [none(), none(), none(), none(), s(r#"["m1"]"#), none()],
        ),
        (
            Link::Checkpoint { name: "cut".into() },
            [none(), none(), none(), none(), none(), s("cut")],
        ),
        (
            Link::Branch {
                branch_id: "br1".into(),
                name: "alt".into(),
                fork_seq: 0,
                fork_hash: None,
                epoch: 1,
            },
            [none(), none(), none(), none(), s(r#"["br1"]"#), s("alt")],
        ),
        (
            Link::BranchSet {
                branch_id: "br1".into(),
                name: None,
                archived: Some(true),
            },
            [none(), none(), none(), none(), s(r#"["br1"]"#), none()],
        ),
    ]
}

fn rows(conn: &Connection) -> Vec<Row> {
    let mut stmt = conn
        .prepare(
            "SELECT tx_id, branch, kind, seq, author, origin, origin_hash, from_branch, merge_id,
                    targets FROM record_links ORDER BY rowid",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
            r.get(7)?,
            r.get(8)?,
            r.get(9)?,
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn name_of(conn: &Connection, tx_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT name FROM record_links WHERE tx_id = ?1",
        [tx_id],
        |r| r.get(0),
    )
    .unwrap()
}

fn intent(tx_id: &str, seq: u64) -> Tx {
    Tx {
        tx_id: tx_id.into(),
        author: "tester".into(),
        branch: "main".into(),
        base_rev: seq - 1,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        ops: vec![],
    }
}

fn check(imported: bool) {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    let mut parent = None;
    for (i, (link, want)) in cases().into_iter().enumerate() {
        let seq = i as u64 + 1;
        let tx_id = format!("tx{seq}");
        let kind = fin_core::kind_name(link.kind());
        let record = new_record(
            seq,
            link,
            intent(&tx_id, seq),
            vec![],
            BTreeMap::new(),
            parent.clone(),
            1_800_000_000_000,
        )
        .unwrap();
        if imported {
            let bytes = encode_record(&record);
            let hash = hash_bytes(&bytes);
            insert_imported(
                &conn,
                "main",
                &record,
                &bytes,
                &hash,
                SCHEMA_VERSION,
                None,
                1,
            )
            .unwrap();
            parent = Some(hash);
        } else {
            parent = Some(insert_tx(&conn, "main", &record, &Diff::default(), 1).unwrap());
        }
        let got = rows(&conn);
        assert_eq!(got.len(), seq as usize, "{kind}");
        let row = got.last().unwrap();
        assert_eq!(
            (&row.0, &row.1, &row.2, row.3, &row.4),
            (&tx_id, &"main".to_string(), &kind, seq as i64, &s("tester")),
            "{kind}"
        );
        let [origin, origin_hash, from, merge_id, targets, name] = want;
        assert_eq!(
            (&row.5, &row.6, &row.7, &row.8, &row.9),
            (&origin, &origin_hash, &from, &merge_id, &targets),
            "{kind}"
        );
        assert_eq!(name_of(&conn, &tx_id), name, "{kind}");
    }
}

#[test]
fn local_commit_path_writes_a_link_row_per_kind() {
    check(false);
}

#[test]
fn import_path_writes_a_link_row_per_kind() {
    check(true);
}

#[test]
fn an_edit_has_no_link_row() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    let record = new_record(
        1,
        Link::Edit,
        intent("e1", 1),
        vec![],
        BTreeMap::new(),
        None,
        1,
    )
    .unwrap();
    insert_tx(&conn, "main", &record, &Diff::default(), 1).unwrap();
    assert!(rows(&conn).is_empty());
}
