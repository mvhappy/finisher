//! Is the file at our own database path damaged (rebuild it) or someone else's (refuse)?
//!
//! - Not an SQLite header (short, or bad magic): damaged.
//! - A valid header with a foreign, non-zero `application_id`: refused (`NOT_A_PROJECT_DB`,
//!   by `migrations::check`), never rebuilt over.
//! - Our `application_id`: `DB_TOO_NEW` is refused; any other failure to read the schema or
//!   `meta` (`NOT_A_PROJECT_DB` included: the header says it is ours) is damage.
//! - `application_id` 0 (a sub-project 2 database, or any SQLite file): refused unless SQLite
//!   itself reports corruption.
//! - Then `PRAGMA quick_check`, opened read-only (a read-only connection never checkpoints a
//!   WAL into a file that may be kept as the damaged copy).

use std::io::Read;
use std::path::Path;

use rusqlite::{Connection, ErrorCode, OpenFlags};

use crate::StoreError;
use crate::migrations::{self, APPLICATION_ID};

const MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// `Some(problem)`: the database at `path` is ours and damaged. `Err`: refused, or the file
/// cannot be read at all.
pub(crate) fn damaged(path: &Path) -> Result<Option<String>, StoreError> {
    let mut head = [0u8; 100];
    let n = {
        let mut f = std::fs::File::open(path).map_err(StoreError::io(path))?;
        let mut n = 0;
        while n < head.len() {
            match f.read(&mut head[n..]).map_err(StoreError::io(path))? {
                0 => break,
                k => n += k,
            }
        }
        n
    };
    if n < head.len() || &head[..16] != MAGIC {
        return Ok(Some("not an SQLite database header".to_string()));
    }
    let app = u32::from_be_bytes([head[68], head[69], head[70], head[71]]);
    let ours = i64::from(app) == i64::from(APPLICATION_ID);
    match migrations::check(path) {
        Ok(_) => {}
        Err(e) if e.code() == "DB_TOO_NEW" => return Err(e),
        Err(e) if ours && e.code() != "IO_ERROR" => return Ok(Some(problem(&e))),
        Err(StoreError::Db(e)) if is_corruption(&e) => return Ok(Some(e.to_string())),
        Err(e) => return Err(e),
    }
    quick_check(path)
}

fn problem(e: &StoreError) -> String {
    match e {
        StoreError::Core(c) => format!("{}: {}", c.code, c.message),
        other => other.to_string(),
    }
}

/// `Some(problem)` when `PRAGMA quick_check` fails (or SQLite reports the file corrupt while
/// checking).
pub(crate) fn quick_check(path: &Path) -> Result<Option<String>, StoreError> {
    let checked = open_read_only(path).and_then(|c| {
        let mut stmt = c.prepare("PRAGMA quick_check(20)")?;
        let rows: Vec<String> = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    });
    match checked {
        Ok(rows) if rows == ["ok"] => Ok(None),
        Ok(rows) => Ok(Some(rows.join("; "))),
        Err(e) if is_corruption(&e) => Ok(Some(e.to_string())),
        Err(e) => Err(e.into()),
    }
}

/// A read-only connection (never checkpoints, never switches the journal mode).
pub(crate) fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let c = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(c)
}

/// SQLite says the file is damaged.
pub(crate) fn is_corruption(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _)
        if matches!(f.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase))
}
