//! The `ids` table: the project-global id counters, one per `fin_core::IdKind` (`asset`,
//! `track`, `clip`, `timeline`, `branch`; from 4C also `caption`, `graphic`, `image`,
//! `adjustment`, `nested`, `marker`, `transition`, `bin`, `effect`, `link`, `style`), loaded
//! for one SQLite transaction and saved back in it.
//!
//! Commit allocates from them. Replay never allocates, so after an import the counters are
//! raised past every id in the log ([`Counters::raise_to_log`], spec §3.3); otherwise the first
//! edit after a takeover would reuse another machine's ids. Counters only ever go up: ids
//! allocated by records that were later set aside (a diverged tail) are never handed out
//! again either.
//!
//! The `ids` table is keyed by kind name, so a kind added later (`branch`, the 4C kinds) needs
//! no migration: a missing row starts at 1.

use fin_core::{ID_KINDS, IdAlloc, IdKind, MemIds, Project};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::db::{meta, parse_record, set_meta, to_i64, to_u64};

const N: usize = ID_KINDS.len();

fn kind_name(kind: IdKind) -> &'static str {
    match kind {
        IdKind::Asset => "asset",
        IdKind::Track => "track",
        IdKind::Clip => "clip",
        IdKind::Timeline => "timeline",
        IdKind::Branch => "branch",
        IdKind::Caption => "caption",
        IdKind::Graphic => "graphic",
        IdKind::Image => "image",
        IdKind::Adjustment => "adjustment",
        IdKind::Nested => "nested",
        IdKind::Marker => "marker",
        IdKind::Transition => "transition",
        IdKind::Bin => "bin",
        IdKind::Effect => "effect",
        IdKind::Link => "link",
        IdKind::Style => "style",
        IdKind::Keyframe => "keyframe",
    }
}

fn slot(kind: IdKind) -> usize {
    ID_KINDS.iter().position(|k| *k == kind).unwrap_or(0)
}

/// Numeric suffix of `id` when it is exactly `prefix` followed by digits.
fn suffix(id: &str, prefix: &str) -> Option<u64> {
    let digits = id.strip_prefix(prefix)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// `meta` key: the highest seq of `branch` whose bindings [`Counters::raise_to_log`] has seen.
pub(crate) fn scanned_key(branch: &str) -> String {
    format!("idsScannedThrough.{branch}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Counters {
    next: [u64; N],
}

impl Counters {
    /// Counters past every id in `project`, by `fin-core`'s own rule (`MemIds::after`).
    pub(crate) fn seed(project: &Project) -> Counters {
        let mut mem = MemIds::after(project);
        let next = ID_KINDS.map(|k| suffix(&mem.next(k), k.prefix()).unwrap_or(1));
        Counters { next }
    }

    pub(crate) fn load(conn: &Connection) -> Result<Counters, StoreError> {
        let mut next = [1; N];
        for kind in ID_KINDS {
            let n: Option<i64> = conn
                .query_row(
                    "SELECT next FROM ids WHERE kind = ?1",
                    params![kind_name(kind)],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(n) = n {
                next[slot(kind)] = to_u64(n)?;
            }
        }
        Ok(Counters { next })
    }

    pub(crate) fn save(&self, conn: &Connection) -> Result<(), StoreError> {
        for kind in ID_KINDS {
            conn.execute(
                "INSERT INTO ids (kind, next) VALUES (?1, ?2) ON CONFLICT(kind) DO UPDATE SET next = excluded.next",
                params![kind_name(kind), to_i64(self.next[slot(kind)])?],
            )?;
        }
        Ok(())
    }

    /// Raises `kind` past `id` if it is one of its ids. Never lowers.
    fn saw(&mut self, id: &str) {
        for kind in ID_KINDS {
            if let Some(n) = suffix(id, kind.prefix()) {
                let s = &mut self.next[slot(kind)];
                *s = (*s).max(n.saturating_add(1));
            }
        }
    }

    /// Raises every counter to (the highest id in the log and in `projects`) + 1, never
    /// lowering one, and saves them.
    ///
    /// "The log" is every record's `bindings` and the ids its `resolved` ops name (replay
    /// accepts literal ids), plus every branch name (branch ids), the index's ids and the
    /// branches `branch` and `branchSet` records name (`record_links.targets`). Each
    /// branch's rows are scanned once: `meta` `idsScannedThrough.<branch>` remembers the
    /// highest seq seen, so a follower importing every few seconds reads only the new rows.
    /// Removing rows ([`rewind_scan`]) moves it back. `projects` are the branch heads after
    /// the import (ids present without a binding: the rev-0 tracks and timeline).
    pub(crate) fn raise_to_log(
        conn: &Connection,
        projects: &[&Project],
    ) -> Result<Counters, StoreError> {
        let loaded = Counters::load(conn)?;
        let mut c = loaded.clone();
        let branches: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT branch FROM txs UNION SELECT branch FROM snapshots ORDER BY 1")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        for branch in &branches {
            c.saw(branch);
            let from: u64 = meta(conn, &scanned_key(branch))?
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let mut stmt = conn.prepare(
                "SELECT seq, record_bytes FROM txs WHERE branch = ?1 AND seq > ?2 ORDER BY seq",
            )?;
            let mut rows = stmt.query(params![branch, to_i64(from)?])?;
            let mut last = None;
            while let Some(row) = rows.next()? {
                let seq = to_u64(row.get(0)?)?;
                let bytes: Vec<u8> = row.get(1)?;
                // Bound ids, and literal ids in the resolved ops (replay accepts them).
                let record = parse_record(Some(seq), &bytes)?;
                for id in record.bindings.values() {
                    c.saw(id);
                }
                for op in &record.resolved {
                    for id in op.ids() {
                        c.saw(id);
                    }
                }
                last = Some(seq);
            }
            if let Some(last) = last {
                set_meta(conn, &scanned_key(branch), &last.to_string())?;
            }
        }
        // Branch ids: the index, and the branches `branch` and `branchSet` records name.
        for sql in [
            "SELECT id FROM branches",
            "SELECT targets FROM record_links WHERE kind IN ('branch', 'branchSet')
             AND targets IS NOT NULL",
        ] {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for v in rows {
                let v = v?;
                match serde_json::from_str::<Vec<String>>(&v) {
                    Ok(ids) => ids.iter().for_each(|id| c.saw(id)),
                    Err(_) => c.saw(&v),
                }
            }
        }
        for p in projects {
            let s = Counters::seed(p);
            for (mine, theirs) in c.next.iter_mut().zip(s.next) {
                *mine = (*mine).max(theirs);
            }
        }
        if c != loaded {
            c.save(conn)?;
        }
        Ok(c)
    }
}

impl Counters {
    /// Raises every counter past the ids that quarantined records used (plan 4B Decision 28),
    /// so this server never hands out an id a set-aside record used: their bindings, the ids
    /// their resolved ops name and a `branch` record's `branchId`. Only rows added since
    /// `meta` `quarantineIdsScannedThrough` (the highest `quarantine.id` scanned) are read,
    /// so a pass that added none reads one row. Bytes that are no readable record (a
    /// malformed or newer one) still give their `bindings` and `branchId`. Never lowers.
    pub(crate) fn raise_to_quarantine(conn: &Connection) -> Result<(), StoreError> {
        let from: i64 = meta(conn, QUARANTINE_SCANNED)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let mut stmt =
            conn.prepare("SELECT id, record_bytes FROM quarantine WHERE id > ?1 ORDER BY id")?;
        let mut rows = stmt.query(params![from])?;
        let loaded = Counters::load(conn)?;
        let mut c = loaded.clone();
        let mut last = None;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let bytes: Vec<u8> = row.get(1)?;
            last = Some(id);
            if let Ok(record) = parse_record(None, &bytes) {
                for op in &record.resolved {
                    for id in op.ids() {
                        c.saw(id);
                    }
                }
            }
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                continue;
            };
            if let Some(b) = v.get("bindings").and_then(|b| b.as_object()) {
                b.values().filter_map(|x| x.as_str()).for_each(|x| c.saw(x));
            }
            if let Some(b) = v.get("branchId").and_then(|b| b.as_str()) {
                c.saw(b);
            }
        }
        if let Some(last) = last {
            set_meta(conn, QUARANTINE_SCANNED, &last.to_string())?;
        }
        if c != loaded {
            c.save(conn)?;
        }
        Ok(())
    }
}

/// `meta` key: the highest `quarantine.id` [`Counters::raise_to_quarantine`] has scanned.
const QUARANTINE_SCANNED: &str = "quarantineIdsScannedThrough";

/// Rows of `branch` after `tip` were removed: the next [`Counters::raise_to_log`] scans
/// whatever takes their seqs. The counters themselves stay where they are.
pub(crate) fn rewind_scan(conn: &Connection, branch: &str, tip: u64) -> Result<(), StoreError> {
    let key = scanned_key(branch);
    let at: Option<u64> = meta(conn, &key)?.and_then(|v| v.parse().ok());
    if at.is_some_and(|at| at > tip) {
        set_meta(conn, &key, &tip.to_string())?;
    }
    Ok(())
}

impl IdAlloc for Counters {
    fn next(&mut self, kind: IdKind) -> String {
        let n = &mut self.next[slot(kind)];
        let id = format!("{}{}", kind.prefix(), n);
        *n += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fin_core::{Rational, new_project};

    #[test]
    fn seed_starts_past_the_new_project_ids() {
        let p = new_project("p".into(), "n".into(), Rational::from_int(30)).unwrap();
        let mut c = Counters::seed(&p);
        assert_eq!(
            ID_KINDS.map(|k| c.next(k)),
            [
                "a1", "t3", "c1", "tl2", "br1", "cap1", "g1", "img1", "adj1", "nst1", "m1", "tr1",
                "b1", "fx1", "lk1", "st1", "kf1"
            ]
            .map(String::from)
        );
    }

    #[test]
    fn saw_raises_only_its_kind_and_never_lowers() {
        let p = new_project("p".into(), "n".into(), Rational::from_int(30)).unwrap();
        let mut c = Counters::seed(&p);
        c.saw("c9");
        c.saw("c4");
        c.saw("tl1");
        c.saw("br3");
        c.saw("$x");
        c.saw("cap7");
        c.saw("fx2");
        assert_eq!(c.next, [1, 3, 10, 2, 4, 8, 1, 1, 1, 1, 1, 1, 1, 3, 1, 1, 1]);
    }

    /// The `ids` rows are keyed by these names: one per kind, never renamed.
    #[test]
    fn kind_names_are_unique_and_frozen() {
        assert_eq!(
            ID_KINDS.map(kind_name),
            [
                "asset",
                "track",
                "clip",
                "timeline",
                "branch",
                "caption",
                "graphic",
                "image",
                "adjustment",
                "nested",
                "marker",
                "transition",
                "bin",
                "effect",
                "link",
                "style",
                "keyframe"
            ]
        );
    }
}
