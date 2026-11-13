//! Reading an entry's bytes for HTTP (6B-1 plan Task 17; spec §7.3): the one read of a payload
//! file outside a commit. A name the entry's row does not list is `None` (the answer for a name
//! that is not there, so a caller learns nothing of the directory); a listed one is opened
//! through `fin_shm::checked_open` (no link at any segment, one hard link, regular, at most the
//! listed size), then read at an offset: a link planted in an entry, or a file grown after its
//! commit, is `SHM_REFUSED` and never served.

// The cache's own directories under server data, never a library path.
#![allow(clippy::disallowed_methods)]

use fin_core::CoreError;
use fin_media::cache::{CacheKey, valid_file_name};

use super::index::cache_io;
use super::state::Cache;

/// The most bytes one [`Cache::read_file`] returns; a caller streams a file in reads of this size.
pub const MAX_READ: u64 = 1 << 20;

#[cfg(unix)]
fn read_at(file: &std::fs::File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

#[cfg(windows)]
fn read_at(file: &std::fs::File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

impl Cache {
    /// Up to `len` (at most [`MAX_READ`]) bytes of the entry file `file` from `offset`; empty
    /// at or past its end; `None` when there is no such entry or the entry lists no such file.
    /// Fewer bytes than asked inside the file mean the file is shorter than listed (the caller
    /// stops: it never serves a half file as a whole).
    pub fn read_file(
        &self,
        key: &str,
        file: &str,
        offset: u64,
        len: u64,
    ) -> Result<Option<Vec<u8>>, CoreError> {
        let Ok(key) = CacheKey::parse(key) else {
            return Ok(None);
        };
        if !valid_file_name(file) {
            return Ok(None);
        }
        let Some(meta) = self.index.get(key.as_str())? else {
            return Ok(None);
        };
        let Some(listed) = meta.files.iter().find(|f| f.name == file) else {
            return Ok(None);
        };
        let size = listed.bytes;
        if offset >= size {
            return Ok(Some(Vec::new()));
        }
        let want = len.min(MAX_READ).min(size - offset);
        let path = self.entry_dir(&key).join(file);
        let open = fin_shm::checked_open(&path, size)?;
        let mut data = vec![0u8; want as usize];
        let mut got = 0usize;
        while got < data.len() {
            let n = read_at(&open, &mut data[got..], offset + got as u64)
                .map_err(|e| cache_io("io", format!("an entry file could not be read: {e}")))?;
            if n == 0 {
                break;
            }
            got += n;
        }
        data.truncate(got);
        Ok(Some(data))
    }
}
