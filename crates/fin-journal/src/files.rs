//! The journal's files (plan D, Decisions 16 and 17): `j-<yyyymmdd>-<ulid>.jsonl`, the date
//! being the UTC date of the file's first entry and the ULID its timestamp; 0600 files in a
//! 0700 directory; torn-tail repair; the age and size caps over whole files, oldest first.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use fin_core::events::{ENVELOPE_VERSION, Envelope};
use ulid::Ulid;

/// One journal file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileMeta {
    pub name: String,
    pub id: Ulid,
    /// The UTC day number (days since the epoch) of its first entry.
    pub day: u64,
    /// Its first and last entry's `ts`.
    pub start_ts: u64,
    pub end_ts: u64,
    pub bytes: u64,
}

pub(crate) const DAY_MS: u64 = 86_400_000;

/// `yyyymmdd` of a UTC day number (Howard Hinnant's `civil_from_days`).
pub(crate) fn yyyymmdd(day: u64) -> String {
    let z = day as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}")
}

pub(crate) fn file_name(day: u64, id: Ulid) -> String {
    format!("j-{}-{id}.jsonl", yyyymmdd(day))
}

/// The id of a journal file name, or `None` for any other file.
pub(crate) fn parse_name(name: &str) -> Option<Ulid> {
    let rest = name.strip_prefix("j-")?.strip_suffix(".jsonl")?;
    let (date, id) = rest.split_once('-')?;
    if date.len() != 8 || !date.bytes().all(|b| b.is_ascii_digit()) || id.len() != 26 {
        return None;
    }
    let id = Ulid::from_string(id).ok()?;
    (id.to_string() == rest[9..]).then_some(id)
}

/// The journal files in `dir`, oldest first; none when it does not exist.
pub(crate) fn list(dir: &Path) -> io::Result<Vec<(String, Ulid, u64)>> {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for e in rd {
        let e = e?;
        let Some(name) = e.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(id) = parse_name(&name) else {
            continue;
        };
        match e.metadata() {
            Ok(m) if m.is_file() => out.push((name, id, m.len())),
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The journal file paths in `dir`, oldest first (for readers; errors read as empty).
pub(crate) fn paths(dir: &Path) -> Vec<PathBuf> {
    list(dir)
        .unwrap_or_default()
        .into_iter()
        .map(|(n, _, _)| dir.join(n))
        .collect()
}

/// The `ts` of the last whole line of `path` that parses as an entry of this envelope
/// version (one a later build wrote is passed over, as the reader passes it over).
pub(crate) fn last_ts(path: &Path, len: u64) -> Option<u64> {
    const TAIL: u64 = 128 * 1024;
    let mut f = File::open(path).ok()?;
    let from = len.saturating_sub(TAIL);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    f.take(TAIL).read_to_end(&mut buf).ok()?;
    buf.split(|b| *b == b'\n')
        .rev()
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_slice::<Envelope>(l).ok())
        .find(|e| e.v == ENVELOPE_VERSION)
        .map(|e| e.ts)
}

/// Truncates `path` after its last `\n` (a torn last line from a crash). Returns the new
/// length.
pub(crate) fn repair_tail(path: &Path, len: u64) -> io::Result<u64> {
    if len == 0 {
        return Ok(0);
    }
    let mut f = OpenOptions::new().read(true).write(true).open(path)?;
    let mut keep = len;
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let from = keep.saturating_sub(chunk.len() as u64);
        let n = (keep - from) as usize;
        f.seek(SeekFrom::Start(from))?;
        f.read_exact(&mut chunk[..n])?;
        if let Some(i) = chunk[..n].iter().rposition(|b| *b == b'\n') {
            keep = from + i as u64 + 1;
            break;
        }
        if from == 0 {
            keep = 0;
            break;
        }
        keep = from;
    }
    if keep < len {
        f.set_len(keep)?;
        f.sync_all()?;
    }
    Ok(keep)
}

/// Creates `dir` (0700 on unix) if it is missing.
pub(crate) fn ensure_dir(dir: &Path) -> io::Result<()> {
    let mut b = DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    // The directories this call makes, deepest first: each gets 0700 whatever the process
    // umask was meanwhile (finisherd narrows it to 0177 while it makes its socket, which would
    // leave no search bit). One that existed keeps its mode: an unwritable one suspends the
    // journal.
    let made: Vec<&Path> = dir
        .ancestors()
        .take_while(|d| !d.as_os_str().is_empty() && !d.exists())
        .collect();
    b.create(dir)?;
    if !fs::metadata(dir)?.is_dir() {
        return Err(io::Error::new(ErrorKind::NotADirectory, "not a directory"));
    }
    #[cfg(unix)]
    for d in made.iter().rev() {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(d)?.permissions().mode() & 0o777 != 0o700 {
            fs::set_permissions(d, fs::Permissions::from_mode(0o700))?;
        }
    }
    #[cfg(not(unix))]
    let _ = made;
    Ok(())
}

/// Creates a new file (0600 on unix) for appending.
pub(crate) fn create(path: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.append(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

/// Opens an existing file for appending (an empty newest file, reused).
pub(crate) fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().append(true).open(path)
}

/// Fsyncs a file; a filesystem that cannot (`EINVAL`, `ENOTSUP`) has nothing to sync.
pub(crate) fn sync_file(f: &File) -> io::Result<()> {
    lenient(f.sync_all())
}

/// Fsyncs a directory so a create, rename or delete in it is durable; a no-op on Windows.
#[cfg(unix)]
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    lenient(File::open(dir)?.sync_all())
}

#[cfg(not(unix))]
pub(crate) fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

fn lenient(r: io::Result<()>) -> io::Result<()> {
    match r {
        Err(e) if matches!(e.kind(), ErrorKind::InvalidInput | ErrorKind::Unsupported) => Ok(()),
        r => r,
    }
}

/// Removes the temp files a crash during a rewrite left (`.<name>.tmp`); best effort.
pub(crate) fn remove_temps(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(n) = name.to_str() else { continue };
        if n.starts_with('.') && n.ends_with(".tmp") {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// Attempts of a call that failed transiently ([`is_transient`]), this far apart: a rename or
/// delete on Windows races a reader, an antivirus scan or a pending delete (as fin-store's
/// `fsio::retry`; 20 × 25 ms here, on the writer thread).
const RETRIES: u32 = 20;
const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(25);

/// An error worth retrying shortly: an interrupted call, and on Windows access denied (5),
/// a sharing (32) or lock (33) violation; on unix `EBUSY`.
pub(crate) fn is_transient(e: &io::Error) -> bool {
    #[cfg(windows)]
    const CODES: &[i32] = &[5, 32, 33];
    #[cfg(unix)]
    const CODES: &[i32] = &[16];
    #[cfg(not(any(unix, windows)))]
    const CODES: &[i32] = &[];
    matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock)
        || e.raw_os_error().is_some_and(|c| CODES.contains(&c))
}

/// `f`, retried while it fails transiently.
pub(crate) fn retry<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut attempt = 0;
    loop {
        match f() {
            Err(e) if attempt < RETRIES && is_transient(&e) => {
                attempt += 1;
                std::thread::sleep(RETRY_DELAY);
            }
            r => return r,
        }
    }
}

/// `fs::rename` with [`retry`].
pub(crate) fn rename(from: &Path, to: &Path) -> io::Result<()> {
    retry(|| fs::rename(from, to))
}

/// `fs::remove_file` with [`retry`]; a missing file is fine.
pub(crate) fn remove(path: &Path) -> io::Result<()> {
    match retry(|| fs::remove_file(path)) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}
