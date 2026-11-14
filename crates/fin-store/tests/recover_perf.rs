//! The cost of the recovered marks on the status path (Task 20 measure; plan 4B Decision 5):
//! status reads them from `quarantine_marks` and `record_links`, so with many quarantine rows
//! a poll does not parse or scan their bytes, and an unchanged database answers from the last
//! summary. Generous bounds, debug build; `PERF_Q` changes the size.

mod integration_support;

use std::time::{Duration, Instant};

use fin_store::Store;
use integration_support::*;
use rusqlite::{Connection, params};

/// `q` quarantine rows copied from a real record (one of them a `branchSet` of a branch a
/// `branch` row among them makes), one of the `branch` rows stood for by a copy, then a poll
/// (which reads their marks).
fn crowded(e: &Env, q: usize) -> Store {
    let mut a = e.create("a");
    commit(&mut a, "t0", vec![asset("/m/0.mov")]);
    let c = Connection::open(a.db_path()).unwrap();
    let bytes: Vec<u8> = c
        .query_row("SELECT record_bytes FROM txs WHERE tx_id = 't0'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let text = String::from_utf8(bytes).unwrap();
    c.execute_batch("BEGIN").unwrap();
    for i in 0..q {
        let mut t = text.replace("\"txId\":\"t0\"", &format!("\"txId\":\"q{i}\""));
        if i == 1 {
            t = t.replace(
                "\"kind\":\"edit\"",
                "\"kind\":\"branchSet\",\"name\":\"n\",\"branchId\":\"br9\"",
            );
        }
        c.execute(
            "INSERT INTO quarantine (tx_hash, branch, origin, reason, detail, record_bytes,
             added_at, seen_at, current) VALUES (?1, 'main', 'local', 'DIVERGED_TAIL', '', ?2, 0, 0, 1)",
            params![format!("h{i}"), t.as_bytes()],
        )
        .unwrap();
    }
    c.execute(
        "INSERT INTO record_links (tx_id, branch, seq, kind, author, origin, recovers)
         VALUES ('pk', 'main', 999, 'cherryPick', 'x', 'q0', 'h0')",
        [],
    )
    .unwrap();
    c.execute_batch("COMMIT").unwrap();
    drop(c);
    e.poll(&mut a);
    a
}

/// Ten status reads, each after another connection changed the database (so none is
/// answered from the last summary), and ten more with nothing changed.
fn timed(s: &Store) -> (Duration, Duration) {
    let other = Connection::open(s.db_path()).unwrap();
    let mut fresh = Duration::ZERO;
    for i in 0..10 {
        other
            .execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('perfProbe', ?1)",
                [i.to_string()],
            )
            .unwrap();
        let t = Instant::now();
        assert_eq!(s.status().quarantine.count as usize, count_of(s));
        fresh += t.elapsed();
    }
    let t = Instant::now();
    for _ in 0..10 {
        s.status();
    }
    (fresh, t.elapsed())
}

fn count_of(s: &Store) -> usize {
    s.status().quarantine.count as usize
}

#[test]
fn status_reads_marks_without_scanning_quarantine() {
    let q: usize = std::env::var("PERF_Q")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    let e = env();
    let s = crowded(&e, q);
    // h0 is stood for; nothing makes br9, so its rename still counts.
    assert_eq!(s.status().quarantine.count as usize, q - 1);
    let (fresh, cached) = timed(&s);
    eprintln!("PERF status q={q}: 10 fresh {fresh:?}, 10 unchanged {cached:?}");
    // Before (Task 20 probe, debug, 10,000 rows): 126 ms per poll with a row stood for,
    // 1,337 ms once a row without ops was among them.
    assert!(fresh < Duration::from_millis(1_000), "{fresh:?}");
    assert!(cached < Duration::from_millis(100), "{cached:?}");
}
