//! `branch_index` (plan 4B Decision 6, check 9; `super` module docs): the derived history
//! tables against the log, and every branch directory's segments against the index.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use fin_core::branch::BranchInfo;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

use super::Subject;
use super::folder::Folder;
use crate::chain::SegRef;
use crate::db::{self, LinkRow, hash_bytes};
use crate::derived::{self, Derived};
use crate::{MAIN, StoreError, branch};

const LIST: usize = 20;

/// Three lists of findings.
type Lists = (Vec<Value>, Vec<Value>, Vec<Value>);

/// The reasons that set a `branch` record aside with its directories (`import::aside`).
const ASIDE: [&str; 5] = [
    "STALE_EPOCH",
    "SUPERSEDED",
    "DIVERGED_TAIL",
    "MALFORMED",
    "ID_COLLISION",
];

/// Pushes `v` while fewer than [`LIST`] are listed; counts every one.
fn push(list: &mut Vec<Value>, count: &mut u64, v: Value) {
    *count += 1;
    if list.len() < LIST {
        list.push(v);
    }
}

pub(super) fn check(s: &Subject, f: &Folder) -> Result<(bool, Value), StoreError> {
    let mut count = 0u64;
    let d = derived::compute(s.conn)?;
    let (index, stateless, unindexed) = index(s, &d, &mut count)?;
    let links = links(s, &d, &mut count)?;
    let (drops, n) = crate::drops_index::drift(s.conn, LIST)?;
    count += n;
    let (segments, (waiting, pending, ahead)) = segments(s, f, &mut count)?;
    // The recovered marks' side table against the quarantine rows' bytes.
    let mut marks = vec![];
    for v in crate::recover::check_marks(s.conn)? {
        push(&mut marks, &mut count, v);
    }
    Ok((
        count == 0,
        json!({ "index": index, "stateless": stateless, "unindexed": unindexed,
                "links": links, "drops": drops, "segments": segments, "waiting": waiting,
                "pending": pending, "ahead": ahead, "marks": marks, "problemCount": count }),
    ))
}

/// The `branches` table against [`derived::compute`]; index rows without any state (the
/// branch cannot be read); branches with rows but no index row (they refuse writes).
fn index(s: &Subject, d: &Derived, count: &mut u64) -> Result<Lists, StoreError> {
    let want: BTreeMap<String, BranchInfo> = d
        .index
        .list()
        .into_iter()
        .map(|(_, i)| (i.id.clone(), i.clone()))
        .collect();
    let have: BTreeMap<String, BranchInfo> = branch::index::load(s.conn)?
        .list()
        .into_iter()
        .map(|(_, i)| (i.id.clone(), i.clone()))
        .collect();
    let mut drift = vec![];
    for id in want.keys().chain(have.keys()).collect::<BTreeSet<_>>() {
        match (want.get(id), have.get(id)) {
            (Some(w), Some(h)) if w == h => {}
            (w, h) => push(
                &mut drift,
                count,
                json!({ "branch": id, "fromLog": w, "stored": h }),
            ),
        }
    }
    let mut stateless = vec![];
    for (id, _) in have.iter().filter(|(_, i)| i.parent.is_some()) {
        if db::latest_rev(s.conn, id)?.is_none() {
            push(&mut stateless, count, json!({ "branch": id }));
        }
    }
    let mut unindexed = vec![];
    for b in db::branches(s.conn)? {
        if b != MAIN && !have.contains_key(&b) {
            push(&mut unindexed, count, json!({ "branch": b }));
        }
    }
    Ok((drift, stateless, unindexed))
}

/// `record_links` against the rows the log gives (one per non-`edit` record, its `ignored`
/// mark from [`derived::compute`]).
fn links(s: &Subject, d: &Derived, count: &mut u64) -> Result<Vec<Value>, StoreError> {
    let ignored = &d.ignored;
    let mut want: BTreeMap<String, (LinkRow, Option<String>)> = BTreeMap::new();
    let mut stmt = s.conn.prepare(
        "SELECT branch, seq, tx_hash, record_bytes FROM txs WHERE kind != 'edit' ORDER BY rowid",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Vec<u8>>(3)?,
        ))
    })?;
    for row in rows {
        let (branch, seq, hash, bytes) = row?;
        let rec = db::parse_record(Some(db::to_u64(seq)?), &bytes)?;
        if let Some(l) = db::link_row(&branch, &rec)? {
            want.insert(l.tx_id.clone(), (l, ignored.get(&hash).cloned()));
        }
    }
    let mut have: BTreeMap<String, (LinkRow, Option<String>)> = BTreeMap::new();
    let mut stmt = s.conn.prepare(
        "SELECT tx_id, branch, seq, kind, author, origin, origin_hash, from_branch, merge_id,
                targets, name, ignored, recovers FROM record_links",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            LinkRow {
                tx_id: r.get(0)?,
                branch: r.get(1)?,
                seq: r.get(2)?,
                kind: r.get(3)?,
                author: r.get(4)?,
                origin: r.get(5)?,
                origin_hash: r.get(6)?,
                from_branch: r.get(7)?,
                merge_id: r.get(8)?,
                targets: r.get(9)?,
                name: r.get(10)?,
                recovers: r.get(12)?,
            },
            r.get::<_, Option<String>>(11)?,
        ))
    })?;
    for row in rows {
        let (l, ig) = row?;
        have.insert(l.tx_id.clone(), (l, ig));
    }
    let mut drift = vec![];
    for id in want.keys().chain(have.keys()).collect::<BTreeSet<_>>() {
        match (want.get(id), have.get(id)) {
            (Some(w), Some(h)) if w == h => {}
            (w, h) => {
                let row = |x: Option<&(LinkRow, Option<String>)>| {
                    x.map(|(l, ig)| json!({ "row": l, "ignored": ig }))
                };
                push(
                    &mut drift,
                    count,
                    json!({ "txId": id, "fromLog": row(w), "stored": row(h) }),
                );
            }
        }
    }
    Ok(drift)
}

/// What became of the `branch` record a segment of directory `dir` names, when it is not the
/// index's record for `dir`: `Ok(true)` set aside (an inert row, or in quarantine),
/// `Ok(false)` unknown here (the directory waits), `Err(why)` a mismatch.
fn named(s: &Subject, dir: &str, record: &str) -> Result<Result<bool, String>, StoreError> {
    let row: Option<(String, Option<String>, Option<String>)> = s
        .conn
        .query_row(
            "SELECT t.kind, l.targets, l.ignored FROM txs t
             LEFT JOIN record_links l ON l.tx_id = t.tx_id WHERE t.tx_hash = ?1",
            params![record],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((kind, targets, ignored)) = row {
        let ids: Vec<String> = targets
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        return Ok(if kind != "branch" || ids != [dir] {
            Err(format!("names {record}, which does not make branch {dir}"))
        } else if ignored.is_none() {
            Err(format!(
                "names {record}, a live branch record the index does not hold for {dir}"
            ))
        } else {
            Ok(true)
        });
    }
    // As import's `aside::named`: a current row with a reason that sets a directory aside
    // (a diverged tail always is); anything else waits.
    let row: Option<(String, bool)> = s
        .conn
        .query_row(
            "SELECT reason, current FROM quarantine WHERE tx_hash = ?1",
            params![record],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(Ok(row.is_some_and(|(reason, current)| {
        ASIDE.contains(&reason.as_str()) && (current || reason == "DIVERGED_TAIL")
    })))
}

/// The segments of each branch directory that do not make its branch (they name another
/// `branch` record than the index's, Decision 33): each must name a record that is set
/// aside here (its records then are too, by the next import: `pending` lists those not yet)
/// or unknown here, never a live record or one of another branch. An unknown record that
/// lies on a folder chain this verify resolved (or in a segment that names such a record,
/// recursively) is the folder ahead of the database, as `chain_continuity`'s `folderAhead`:
/// listed (`ahead`), not a failure. Any other unknown record is `waiting`: a failure unless
/// files are still arriving.
fn segments(s: &Subject, f: &Folder, count: &mut u64) -> Result<(Vec<Value>, Lists), StoreError> {
    let (mut bad, mut waiting, mut pending, mut ahead) = (vec![], vec![], vec![], vec![]);
    let (mut uncounted, mut pends) = (0u64, 0u64);
    // Unknown records: (directory, record, segments).
    let mut unknown: Vec<(&str, &str, Vec<&SegRef>)> = vec![];
    for b in &f.branches {
        let mut groups: BTreeMap<Option<&str>, Vec<&SegRef>> = BTreeMap::new();
        for g in &b.others {
            groups
                .entry(g.header().branch_record.as_deref())
                .or_default()
                .push(g);
        }
        for (record, segs) in groups {
            let hashes: Vec<&str> = segs.iter().map(|g| g.hash()).collect();
            let Some(record) = record else {
                push(
                    &mut bad,
                    count,
                    json!({ "branch": b.branch, "segments": hashes,
                    "problem": "a branch segment without branchRecord" }),
                );
                continue;
            };
            match named(s, &b.branch, record)? {
                Err(why) => push(
                    &mut bad,
                    count,
                    json!({ "branch": b.branch, "segments": hashes, "problem": why }),
                ),
                Ok(false) => unknown.push((&b.branch, record, segs)),
                Ok(true) => {
                    let mut seen = HashSet::new();
                    for g in &segs {
                        for bytes in g.records() {
                            let h = hash_bytes(bytes);
                            if seen.insert(h.clone()) && !held(s, &h)? {
                                push(
                                    &mut pending,
                                    &mut pends,
                                    json!({ "branch": b.branch,
                                    "segment": g.hash(), "txHash": h }),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    // The folder ahead of the database: the resolved chains, then what they make.
    let mut on_folder: HashSet<String> = f
        .branches
        .iter()
        .flat_map(|b| b.chain.iter().map(|(_, h)| h.clone()))
        .collect();
    let mut grew = true;
    while grew {
        grew = false;
        unknown.retain(|(dir, record, segs)| {
            if !on_folder.contains(*record) {
                return true;
            }
            on_folder.extend(segs.iter().flat_map(|g| g.records().map(hash_bytes)));
            let hashes: Vec<&str> = segs.iter().map(|g| g.hash()).collect();
            push(
                &mut ahead,
                &mut uncounted,
                json!({ "branch": dir, "record": record, "segments": hashes }),
            );
            grew = true;
            false
        });
    }
    for (dir, record, segs) in unknown {
        let hashes: Vec<&str> = segs.iter().map(|g| g.hash()).collect();
        let v = json!({ "branch": dir, "record": record, "segments": hashes });
        if f.arriving {
            push(&mut waiting, &mut uncounted, v);
        } else {
            push(&mut waiting, count, v);
        }
    }
    Ok((bad, (waiting, pending, ahead)))
}

/// Whether the database holds the record `hash`, in `txs` or in quarantine.
fn held(s: &Subject, hash: &str) -> Result<bool, StoreError> {
    let n: i64 = s.conn.query_row(
        "SELECT (SELECT COUNT(*) FROM txs WHERE tx_hash = ?1)
              + (SELECT COUNT(*) FROM quarantine WHERE tx_hash = ?1)",
        params![hash],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
