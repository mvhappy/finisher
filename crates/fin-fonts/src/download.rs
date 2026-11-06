//! One resumable download (4C Decision 32) into `<sha256>.part`, with `<sha256>.part.json`
//! `{url, received}` beside it. A later attempt (in this process or another) at the same URL
//! asks for the rest with `Range`; a server answering `200` restarts it. The body may never
//! exceed the expected size, and the finished file must hash to the expected SHA-256.
//! The caller holds the file's fetch lock.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cache::{FontCache, hash_file};
use crate::http::{Failure, Http};

/// `part.json` is rewritten after this many bytes.
const JSON_EVERY: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct PartInfo {
    url: String,
    received: u64,
}

/// What to fetch and where it must end up.
pub(crate) struct Target<'a> {
    pub url: &'a str,
    pub sha256: &'a str,
    pub size: u64,
}

fn io_failure(e: &io::Error) -> Failure {
    Failure::new("io", e.to_string(), false)
}

fn write_json(path: &Path, url: &str, received: u64) {
    let info = PartInfo {
        url: url.to_string(),
        received,
    };
    if let Ok(bytes) = serde_json::to_vec(&info) {
        let _ = fs::write(path, bytes);
    }
}

/// How much of `part` an earlier attempt at `url` left: the smaller of `part.json`'s
/// `received` and the file's length, or 0 when `part.json` names another URL.
fn resumable(part: &Path, json: &Path, url: &str) -> u64 {
    let info: Option<PartInfo> = fs::read(json)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let len = fs::metadata(part).map_or(0, |m| m.len());
    match info {
        Some(i) if i.url == url => i.received.min(len),
        _ => 0,
    }
}

fn discard(part: &Path, json: &Path) {
    let _ = fs::remove_file(part);
    let _ = fs::remove_file(json);
}

/// Downloads `t` into the cache's `<sha256>.part` and returns its path once it is complete
/// and verified. `progress(received, size)` follows the body.
pub(crate) fn download(
    http: &Http,
    cache: &FontCache,
    t: &Target<'_>,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, Failure> {
    let part = cache.part_path(t.sha256);
    let json = cache.part_json_path(t.sha256);
    let mut received = resumable(&part, &json, t.url);
    let resumed = received > 0;
    if received < t.size {
        received = fetch_rest(http, &part, &json, t, received, progress)?;
    }
    if received != t.size {
        discard(&part, &json);
        return Err(Failure::new(
            "tooLarge",
            format!("{} is larger than {} bytes", t.url, t.size),
            false,
        ));
    }
    match hash_file(&part) {
        Ok(h) if h == t.sha256 => Ok(part),
        Ok(h) => {
            discard(&part, &json);
            // A resumed file may hold an earlier, different body: start afresh once more.
            Err(Failure::new(
                "hashMismatch",
                format!("{} has SHA-256 {h}, not {}", t.url, t.sha256),
                resumed,
            ))
        }
        Err(e) => Err(io_failure(&e)),
    }
}

/// Requests the bytes from `received` on and appends them; returns the new length.
fn fetch_rest(
    http: &Http,
    part: &Path,
    json: &Path,
    t: &Target<'_>,
    received: u64,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<u64, Failure> {
    let body = http.get(t.url, received)?;
    if body.start > 0 && body.total != Some(t.size) {
        // Some CDNs range over another representation (a compressed one): start afresh.
        discard(part, json);
        return Err(Failure::new(
            "badRange",
            format!(
                "{} ranges over {:?} bytes, not {}",
                t.url, body.total, t.size
            ),
            true,
        ));
    }
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(part)
        .map_err(|e| io_failure(&e))?;
    // A `200` restarts from zero; a `206` continues at `received`.
    let mut received = body.start;
    file.set_len(received).map_err(|e| io_failure(&e))?;
    file.seek(SeekFrom::Start(received))
        .map_err(|e| io_failure(&e))?;
    if body.length.is_some_and(|n| received + n > t.size) {
        drop(file);
        discard(part, json);
        return Err(Failure::new(
            "tooLarge",
            format!("{} sends more than {} bytes", t.url, t.size),
            false,
        ));
    }
    write_json(json, t.url, received);
    progress(received, t.size);
    let result = copy_limited(body.reader, &mut file, &mut received, t, json, progress);
    let synced = file.sync_all();
    drop(file);
    write_json(json, t.url, received);
    match result {
        Ok(()) if received > t.size => Ok(received),
        Ok(()) if received < t.size => Err(Failure::new(
            "truncated",
            format!("{} ended after {received} of {} bytes", t.url, t.size),
            true,
        )),
        Ok(()) => synced.map(|()| received).map_err(|e| io_failure(&e)),
        Err(f) => Err(f),
    }
}

/// Copies at most `size + 1` bytes (one more proves the body too long).
fn copy_limited(
    mut reader: Box<dyn Read + Send>,
    file: &mut File,
    received: &mut u64,
    t: &Target<'_>,
    json: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), Failure> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut since_json = 0;
    loop {
        let room = (t.size + 1).saturating_sub(*received);
        if room == 0 {
            return Ok(());
        }
        let want = buf.len().min(usize::try_from(room).unwrap_or(usize::MAX));
        let n = match reader.read(&mut buf[..want]) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(Failure::new(
                    "network",
                    format!("{}: {e} after {received} bytes", t.url),
                    true,
                ));
            }
        };
        file.write_all(&buf[..n]).map_err(|e| io_failure(&e))?;
        *received += n as u64;
        since_json += n as u64;
        if since_json >= JSON_EVERY {
            since_json = 0;
            write_json(json, t.url, *received);
        }
        progress((*received).min(t.size), t.size);
    }
}
