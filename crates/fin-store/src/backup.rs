//! Database backups (sub-project 4 spec §3.1, §3.8): `VACUUM INTO` a temp file in
//! `<project-data>/backups/`, fsync it, move it into place without replacing anything. Never
//! a raw file copy, which would miss the WAL.
//!
//! - Pre-migration: `pre-migrate-<from>-<to>-<UTC ts>Z.fdb`, the last 3 kept.
//! - Daily ([`daily`]): `daily-YYYY-MM-DD.fdb` (the **local** calendar date), at most one per
//!   day while the project is open, the last 7 kept. Every server backs up its own
//!   database, holder or follower: it is local state either way (a follower's is a rebuild
//!   it need not redo), and the folder is not touched.
//!
//! Both: only strictly named files are pruned, the new backup is never pruned, and once it
//! is durable a failure to clean up or prune is ignored (an extra old file is harmless).

use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fin_core::CoreError;
use rusqlite::{Connection, params};

use crate::StoreError;
use crate::fsio::sync_dir;

/// How many `pre-migrate-*` backups are kept.
const KEEP_PRE_MIGRATION: usize = 3;
/// How many `daily-*` backups are kept.
const KEEP_DAILY: usize = 7;
/// A backup temp file older than this was left by a crash (a live backup takes seconds).
const STALE_TEMP: Duration = Duration::from_secs(10 * 60);

/// A consistent copy of the database behind `conn` at `dest`, durable once this returns.
/// `false` (nothing written) when `dest` already exists.
pub(crate) fn backup_to(conn: &Connection, dest: &Path) -> Result<bool, StoreError> {
    let dir = dest.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".~fin-{}.tmp", ulid::Ulid::new()));
    let result = (|| {
        let target = tmp.to_str().ok_or_else(|| {
            CoreError::new(
                "IO_ERROR",
                format!("backup path {} is not valid UTF-8", tmp.display()),
            )
        })?;
        conn.execute("VACUUM INTO ?1", params![target])?;
        // A backup restored by hand is another database than the live one: callers holding
        // the live one's generations must not be accepted by it.
        {
            let copy = Connection::open(&tmp)?;
            let has_meta: i64 = copy.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
                [],
                |r| r.get(0),
            )?;
            if has_meta > 0 {
                copy.execute(
                    "UPDATE meta SET value = ?1 WHERE key = 'incarnation'",
                    params![crate::history::new_incarnation()],
                )?;
            }
        }
        // Windows' FlushFileBuffers needs a handle with write access.
        File::options()
            .write(true)
            .open(&tmp)
            .and_then(|f| crate::fsio::sync_file(&f))
            .map_err(StoreError::io(&tmp))?;
        match crate::fsio::rename_noreplace(&tmp, dest) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
            Err(e) => return Err(StoreError::io(dest)(e)),
        }
        sync_dir(dir).map_err(StoreError::io(dir))?;
        Ok(true)
    })();
    if !matches!(result, Ok(true)) {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// The daily backup for the local calendar day of unix ms `now_ms`
/// (`<project_data>/backups/daily-YYYY-MM-DD.fdb`), unless that day has one; keeps the
/// newest [`KEEP_DAILY`]. Returns the new backup's path.
pub(crate) fn daily(
    conn: &Connection,
    project_data: &Path,
    now_ms: i64,
) -> Result<Option<std::path::PathBuf>, StoreError> {
    let (y, m, d) = crate::localdate::local_ymd(now_ms);
    let name = format!("daily-{y:04}-{m:02}-{d:02}.fdb");
    let dir = project_data.join("backups");
    let dest = dir.join(&name);
    if dest.exists() {
        return Ok(None);
    }
    fs::create_dir_all(&dir).map_err(StoreError::io(&dir))?;
    remove_stale_temps(&dir);
    if !backup_to(conn, &dest)? {
        return Ok(None);
    }
    // A name dated after tomorrow (the clock was wrong when it was taken) must not hold a
    // slot for ever: it sorts as the oldest.
    let (ty, tm, td) = crate::localdate::local_ymd(now_ms + 86_400_000);
    let tomorrow = format!("{ty:04}-{tm:02}-{td:02}");
    let _ = prune(&dir, &name, KEEP_DAILY, &|n: &str| {
        daily_date(n).map(|d| {
            if d > tomorrow.as_str() {
                format!("0000-00-00 {d}")
            } else {
                d.to_string()
            }
        })
    });
    Ok(Some(dest))
}

/// The date of a strict `daily-YYYY-MM-DD.fdb` name.
fn daily_date(name: &str) -> Option<&str> {
    let date = name.strip_prefix("daily-")?.strip_suffix(".fdb")?;
    let b = date.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    (b.len() == 10 && digits(0..4) && b[4] == b'-' && digits(5..7) && b[7] == b'-' && digits(8..10))
        .then_some(date)
}

/// Backs the database up to `<project_data>/backups/pre-migrate-<from>-<to>-<ts>.fdb` and
/// keeps only the newest [`KEEP_PRE_MIGRATION`] such backups, always including the new one.
///
/// First removes `.~fin-*.tmp` files older than [`STALE_TEMP`] (left by a crash). Once the new
/// backup is durable, failing to clean up or prune does not fail the migration: an extra old
/// file is harmless.
pub(crate) fn pre_migration(
    conn: &Connection,
    project_data: &Path,
    from: u32,
    to: u32,
) -> Result<(), StoreError> {
    let dir = project_data.join("backups");
    fs::create_dir_all(&dir).map_err(StoreError::io(&dir))?;
    remove_stale_temps(&dir);
    let mut ms = now_ms();
    let (name, dest) = loop {
        let name = format!("pre-migrate-{from}-{to}-{}.fdb", timestamp(ms));
        let dest = dir.join(&name);
        if !dest.exists() {
            break (name, dest);
        }
        ms += 1;
    };
    backup_to(conn, &dest)?;
    let _ = prune(&dir, &name, KEEP_PRE_MIGRATION, &|n: &str| {
        pre_migration_ts(n).map(str::to_string)
    });
    Ok(())
}

fn remove_stale_temps(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !(name.starts_with(".~fin-") && name.ends_with(".tmp")) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE_TEMP);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The timestamp of a strict `pre-migrate-<n>-<n>-<YYYYMMDDTHHMMSSmmm>Z.fdb` name.
fn pre_migration_ts(name: &str) -> Option<&str> {
    let rest = name.strip_prefix("pre-migrate-")?.strip_suffix(".fdb")?;
    let mut parts = rest.splitn(3, '-');
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let (from, to, ts) = (parts.next()?, parts.next()?, parts.next()?);
    let b = ts.as_bytes();
    let ok = digits(from)
        && digits(to)
        && b.len() == 19
        && digits(&ts[..8])
        && b[8] == b'T'
        && digits(&ts[9..18])
        && b[18] == b'Z';
    ok.then_some(ts)
}

/// Deletes all but the newest `keep` backups in `dir` whose names `key` accepts (sorted by
/// that key), never `new` (the one just taken, which counts towards `keep`). Other names are
/// left alone.
fn prune(
    dir: &Path,
    new: &str,
    keep: usize,
    key: &dyn Fn(&str) -> Option<String>,
) -> Result<(), StoreError> {
    let mut others: Vec<(String, String)> = fs::read_dir(dir)
        .map_err(StoreError::io(dir))?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name != new)
        .filter_map(|name| Some((key(&name)?, name)))
        .collect();
    others.sort();
    let excess = others.len().saturating_sub(keep.saturating_sub(1));
    let mut result = Ok(());
    for (_, name) in others.into_iter().take(excess) {
        let path = dir.join(name);
        if let Err(e) = fs::remove_file(&path) {
            result = Err(StoreError::io(&path)(e));
        }
    }
    result
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// UTC `YYYYMMDDTHHMMSSmmmZ` for unix milliseconds `ms`: fixed width, so names sort by time.
pub(crate) fn timestamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms.rem_euclid(1000)
    )
}

/// Proleptic Gregorian (year, month, day) of `days` since 1970-01-01 (H. Hinnant's algorithm).
pub(crate) fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_utc_and_fixed_width() {
        assert_eq!(timestamp(0), "19700101T000000000Z");
        // 2026-11-04 10:15:30.042 UTC.
        assert_eq!(timestamp(1_793_787_330_042), "20261104T101530042Z");
        // A leap day.
        assert_eq!(timestamp(951_782_400_000), "20000229T000000000Z");
    }

    #[test]
    fn strict_names_only() {
        assert_eq!(
            pre_migration_ts("pre-migrate-0-12-20261104T101530042Z.fdb"),
            Some("20261104T101530042Z")
        );
        for name in [
            "pre-migrate-0-1-20261104T101530042Z.fdb.tmp",
            "pre-migrate--1-20261104T101530042Z.fdb",
            "pre-migrate-0-1-2026110XT101530042Z.fdb",
            "pre-migrate-0-1-20261104T10153004Z.fdb",
            "pre-migrate-0-1-20261104-101530042Z.fdb",
            "pre-migrate-0-1-junk.fdb",
            "pre-migrate-a-1-20261104T101530042Z.fdb",
        ] {
            assert_eq!(pre_migration_ts(name), None, "{name}");
        }
    }

    #[test]
    fn strict_daily_names_only() {
        assert_eq!(daily_date("daily-2026-11-04.fdb"), Some("2026-11-04"));
        for name in [
            "daily-2026-11-4.fdb",
            "daily-2026-11-04.fdb.tmp",
            "daily-notes.fdb",
            "daily-2026_11_04.fdb",
        ] {
            assert_eq!(daily_date(name), None, "{name}");
        }
    }

    #[test]
    fn prune_keeps_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "pre-migrate-0-1-20261104T101530042Z.fdb",
            "pre-migrate-1-2-20261101T000000000Z.fdb",
            "pre-migrate-0-1-20261103T000000000Z.fdb",
            "pre-migrate-0-1-20261105T000000000Z.fdb",
            "daily-2026-11-01.fdb",
        ] {
            fs::write(dir.path().join(name), b"").unwrap();
        }
        prune(
            dir.path(),
            "pre-migrate-0-1-20261105T000000000Z.fdb",
            3,
            &|n: &str| pre_migration_ts(n).map(str::to_string),
        )
        .unwrap();
        let mut left: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "daily-2026-11-01.fdb",
                "pre-migrate-0-1-20261103T000000000Z.fdb",
                "pre-migrate-0-1-20261104T101530042Z.fdb",
                "pre-migrate-0-1-20261105T000000000Z.fdb",
            ]
        );
    }
}
