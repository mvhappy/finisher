//! Database backups (sub-project 4 spec §3.1, §3.8): `VACUUM INTO` a temp file in
//! `<project-data>/backups/`, fsync it, rename it into place. Never a raw file copy, which
//! would miss the WAL.

use std::fs::{self, File};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use fin_core::CoreError;
use rusqlite::{Connection, params};

use crate::StoreError;
use crate::paths::sync_dir;

/// How many `pre-migrate-*` backups are kept.
const KEEP_PRE_MIGRATION: usize = 3;

/// A consistent copy of the database behind `conn` at `dest` (which must not exist), durable
/// once this returns.
pub(crate) fn backup_to(conn: &Connection, dest: &Path) -> Result<(), StoreError> {
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
        File::open(&tmp)
            .and_then(|f| f.sync_all())
            .map_err(StoreError::io(&tmp))?;
        if dest.exists() {
            return Err(CoreError::new(
                "IO_ERROR",
                format!("backup {} already exists", dest.display()),
            )
            .into());
        }
        fs::rename(&tmp, dest).map_err(StoreError::io(dest))?;
        sync_dir(dir).map_err(StoreError::io(dir))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Backs the database up to `<project_data>/backups/pre-migrate-<from>-<to>-<ts>.fdb` and
/// keeps only the newest [`KEEP_PRE_MIGRATION`] such backups.
pub(crate) fn pre_migration(
    conn: &Connection,
    project_data: &Path,
    from: u32,
    to: u32,
) -> Result<(), StoreError> {
    let dir = project_data.join("backups");
    fs::create_dir_all(&dir).map_err(StoreError::io(&dir))?;
    let mut ms = now_ms();
    let dest = loop {
        let dest = dir.join(format!("pre-migrate-{from}-{to}-{}.fdb", timestamp(ms)));
        if !dest.exists() {
            break dest;
        }
        ms += 1;
    };
    backup_to(conn, &dest)?;
    prune(&dir, "pre-migrate-", KEEP_PRE_MIGRATION)
}

/// Deletes all but the newest `keep` `<prefix>…-<ts>.fdb` files in `dir`, by timestamp.
fn prune(dir: &Path, prefix: &str, keep: usize) -> Result<(), StoreError> {
    let mut found: Vec<(String, String)> = fs::read_dir(dir)
        .map_err(StoreError::io(dir))?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|name| {
            let stem = name.strip_prefix(prefix)?.strip_suffix(".fdb")?;
            let ts = stem.rsplit('-').next()?.to_string();
            Some((ts, name))
        })
        .collect();
    found.sort();
    let excess = found.len().saturating_sub(keep);
    for (_, name) in found.into_iter().take(excess) {
        let path = dir.join(name);
        fs::remove_file(&path).map_err(StoreError::io(&path))?;
    }
    Ok(())
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
fn civil_from_days(days: i64) -> (i64, i64, i64) {
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
        prune(dir.path(), "pre-migrate-", 3).unwrap();
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
