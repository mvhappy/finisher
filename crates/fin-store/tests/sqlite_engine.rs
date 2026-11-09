//! The bundled SQLite engine has the WAL-reset corruption fix (review #45): 3.51.3 or later,
//! or the 3.50.7+ backport (https://sqlite.org/wal.html#walreset). Project databases run in
//! WAL mode, so a dependency update that brings an older engine back fails here.

mod integration_support;

use integration_support::*;
use rusqlite::Connection;

/// Whether SQLite `v` (`SQLITE_VERSION_NUMBER` form, `3_051_003`) has the WAL-reset fix.
fn wal_reset_fixed(v: i32) -> bool {
    v >= 3_051_003 || (3_050_007..3_051_000).contains(&v)
}

#[test]
fn wal_reset_fix_bound() {
    for v in [3_050_002, 3_050_006, 3_051_000, 3_051_002] {
        assert!(!wal_reset_fixed(v), "{v}");
    }
    for v in [3_050_007, 3_050_009, 3_051_003, 3_054_000, 4_000_000] {
        assert!(wal_reset_fixed(v), "{v}");
    }
}

#[test]
fn bundled_sqlite_has_the_wal_reset_fix() {
    let e = env();
    let s = e.create("a");
    let c = Connection::open(s.db_path()).unwrap();
    let mode: String = c
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    let version: String = c
        .query_row("SELECT sqlite_version()", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, rusqlite::version(), "one engine is linked");
    let n = rusqlite::version_number();
    assert!(
        wal_reset_fixed(n),
        "SQLite {version} predates the WAL-reset fix (3.51.3, backport 3.50.7)"
    );
}
