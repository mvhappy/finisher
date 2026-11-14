//! A commit (6B-1 plan Decision 10, Task 13): a finished job's `out/<key>/` into the cache,
//! **rename, then insert**.
//!
//! 1. An entry already indexed under `key` wins: the job's copy is left where it is (5C removes
//!    the job directory), the projects named are added to the entry and its meta is answered.
//! 2. `out/<key>/meta.json` is read and checked (`adopt::read_meta`: `fin-shm`'s open, a strict
//!    parse, the key recomputes and equals `key`), and the directory must hold exactly the
//!    files it lists.
//! 3. The volume must have room for the entry and [`RESERVE`] (`CACHE_IO {reason: full,
//!    freeBytes, neededBytes}`, which fails the job without a retry, Decision 12).
//! 4. Each file is moved by `fin_shm::commit_output` (its checks, with the listed size as the
//!    cap) into a staging directory the host made, and hashed there (size and BLAKE3 against
//!    `meta.json`): the worker has exited and nothing else writes the staging directory.
//! 5. The staging directory is renamed to `<key[0..2]>/<key>/`. A directory already there
//!    without a row (a commit that crashed after its rename) is kept when it passes
//!    `adopt::verify_dir` (first wins), else replaced.
//! 6. Only then is the row inserted (with the projects, pinned for the open ones). A crash
//!    between 5 and 6 leaves a valid directory without a row, which the next open adopts.
//!
//! Any failure after step 3 is `CACHE_IO {reason: full}` when the volume has since run out of
//! room; otherwise the step's own error (`SHM_REFUSED`, `PLAN_MISMATCH`, `CACHE_IO`).

// The cache's own directories under server data, never a library path.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use fin_core::CoreError;
use fin_media::cache::{CacheKey, EntryMeta, META_FILE};
use serde_json::json;

use super::adopt::{META_CAP, check_hashes, check_listing, mismatch, read_meta, verify_dir};
use super::evict::{entry_dir, remove_any, volume_space};
use super::index::cache_io;
use super::state::Cache;

/// Room left on the volume beyond an entry for the index to grow.
pub const RESERVE: u64 = 16 << 20;

/// `CACHE_IO {reason: full}`: the volume cannot take the entry.
pub fn volume_full(free: u64, needed: u64) -> CoreError {
    CoreError::new("CACHE_IO", "the media cache's volume is full")
        .with_details(json!({ "reason": "full", "freeBytes": free, "neededBytes": needed }))
        .with_fix(
            "free space on the volume that holds the server data, or lower media.cacheMaxGiB \
             in settings.json and run `fin cache gc`",
        )
}

fn io(what: &str, e: std::io::Error) -> CoreError {
    cache_io("io", format!("{what}: {e}"))
}

/// The job output directory with its job directory's parent resolved once (as
/// `fin_shm::JobDir::resolve`): a link at or below the job directory is still refused by every
/// open.
fn resolve_source(job_out: &Path, key: &CacheKey) -> Result<PathBuf, CoreError> {
    let (Some(job), Some(out)) = (job_out.parent(), job_out.file_name()) else {
        return Err(cache_io(
            "io",
            "the job output directory needs a job directory",
        ));
    };
    let job = fin_shm::JobDir::resolve(job)?;
    Ok(job.root.join(out).join(key.as_str()))
}

impl Cache {
    /// Commits `<job_out>/<key>/` (module docs); the entry's meta.
    pub fn commit(
        &mut self,
        job_out: &Path,
        key: &str,
        projects: &[String],
    ) -> Result<EntryMeta, CoreError> {
        let key = CacheKey::parse(key)?;
        let projects = super::state::check_projects(projects)?;
        if let Some(meta) = self.index.get(key.as_str())? {
            let open = self.open_projects();
            self.index.add_projects(key.as_str(), &projects, &open)?;
            self.index.touch(key.as_str(), self.now())?;
            return Ok(meta);
        }
        let src = resolve_source(job_out, &key)?;
        let (meta, meta_len) = read_meta(&src)?;
        if meta.key != key.as_str() {
            return Err(CoreError::new(
                "PLAN_MISMATCH",
                "the job's meta.json names another cache key",
            )
            .with_details(json!({ "reason": "cacheKey", "key": key.as_str(), "got": meta.key })));
        }
        check_listing(&src, &meta)?;
        let bytes = meta
            .bytes()?
            .checked_add(meta_len)
            .ok_or_else(|| mismatch("the entry's size overflows", META_FILE))?;
        let needed = bytes.saturating_add(RESERVE);
        let free = self.free_bytes_precheck()?;
        if free < needed {
            return Err(volume_full(free, needed));
        }
        match self.place(&src, &key, &meta, bytes, &projects) {
            Ok(meta) => {
                // Over the cap: evict the least recently used, never what was just committed.
                let cap = self.cap;
                let _ = self.evict(cap, Some(key.as_str()));
                Ok(meta)
            }
            Err(e) => match volume_space(&self.root) {
                Ok((_, free)) if free < needed => Err(volume_full(free, needed)),
                _ => Err(e),
            },
        }
    }

    /// Steps 4–6 of the module docs.
    fn place(
        &mut self,
        src: &Path,
        key: &CacheKey,
        meta: &EntryMeta,
        bytes: u64,
        projects: &[String],
    ) -> Result<EntryMeta, CoreError> {
        self.seq += 1;
        let staging =
            self.root
                .join(".staging")
                .join(format!("{key}-{}-{}", std::process::id(), self.seq));
        std::fs::create_dir_all(&staging).map_err(|e| io("cannot make a staging directory", e))?;
        let placed = self.stage_and_rename(src, &staging, key, meta, bytes);
        let _ = remove_any(&staging);
        let (meta, bytes) = placed?;
        #[cfg(any(test, feature = "test-hooks"))]
        if self.faults.stop_after_rename {
            return Err(CoreError::new(
                "INTERNAL",
                "test hook: stopped after the rename",
            ));
        }
        let open = self.open_projects();
        let now = self.now();
        if let Err(e) = self.index.insert(&meta, bytes, now, projects, &open) {
            // The row did not go in: the directory goes too (the volume may be full).
            let _ = remove_any(&entry_dir(&self.root, key));
            return Err(e);
        }
        Ok(meta)
    }

    fn stage_and_rename(
        &mut self,
        src: &Path,
        staging: &Path,
        key: &CacheKey,
        meta: &EntryMeta,
        bytes: u64,
    ) -> Result<(EntryMeta, u64), CoreError> {
        let caps = meta
            .files
            .iter()
            .map(|f| (f.name.as_str(), f.bytes))
            .chain([(META_FILE, META_CAP)]);
        for (name, cap) in caps {
            fin_shm::commit_output(&src.join(name), &staging.join(name), cap)?;
        }
        // The copy now in the host's own directory is what is checked and kept.
        let (staged, _) = read_meta(staging)?;
        if &staged != meta {
            return Err(
                CoreError::new("PLAN_MISMATCH", "meta.json changed while it was moved")
                    .with_details(json!({ "reason": "cacheEntry", "file": META_FILE })),
            );
        }
        check_listing(staging, &staged)?;
        check_hashes(staging, &staged)?;
        sync_dir(staging);
        let dest = entry_dir(&self.root, key);
        let shard = dest.parent().expect("an entry directory has a shard");
        if std::fs::symlink_metadata(shard).is_ok_and(|m| !m.is_dir()) {
            let _ = remove_any(shard);
        }
        std::fs::create_dir_all(shard).map_err(|e| io("cannot make a cache shard", e))?;
        if std::fs::symlink_metadata(&dest).is_ok() {
            // A directory without a row: a commit that crashed after its rename. First wins
            // when it is a whole entry; anything else is replaced.
            if let Ok(existing) = verify_dir(&dest, key) {
                return Ok(existing);
            }
            remove_any(&dest).map_err(|e| io("cannot replace a damaged entry", e))?;
        }
        std::fs::rename(staging, &dest).map_err(|e| io("cannot move the entry into place", e))?;
        sync_dir(shard);
        Ok((staged, bytes))
    }
}

/// Makes a directory's entries durable where the OS allows it (not on Windows).
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}
