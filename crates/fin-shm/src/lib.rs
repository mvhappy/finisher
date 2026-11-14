//! The mapped-file bulk channel (sub-project 6 spec §3.4, §18): job-directory layout, rings,
//! and the open and mapping rules. Frames and PCM never go over the stdio envelope; they go
//! through files in the job directory that 5C creates at a fixed size (cp §6.4).
//!
//! **The rules, implemented once here** for media workers, codec plugin helpers, the host's
//! output move and sub-project 8's plugin host:
//! - **Every open** ([`checked_open`]) walks the path one segment at a time without following
//!   a link at *any* segment (unix: `openat(O_NOFOLLOW)` from the previous directory's
//!   descriptor; Windows: `FILE_FLAG_OPEN_REPARSE_POINT` per segment, each held open without
//!   `FILE_SHARE_DELETE` so no ancestor can be swapped during the walk, and a reparse point
//!   refused); the file must be a regular file (no FIFO, socket, device or directory) with one
//!   link, at most `cap` bytes. A refusal is `SHM_REFUSED {file, reason}` ([`Reason`]).
//! - **Paths are canonical.** A link anywhere in the path is refused, so callers pass paths
//!   under a canonical job directory ([`JobDir::resolve`] resolves the host's own part once).
//! - **The host never maps a file a less-trusted process can still write**:
//!   [`RingReader::open_untrusted`] reads with bounded `pread` into the caller's buffer, so a
//!   truncation is a typed error (`truncated`), never a `SIGBUS`; only
//!   [`RingReader::open_sealed`] (the writer has exited) maps read-only. A [`RingWriter`] maps
//!   its own file.
//! - **Rings** ([`layout`]): a 32-byte header (`FINSHM01`, version, slot size, slot count), a
//!   slot table (`seq`, `len` per slot) and fixed-size slots, in one file of its final size.
//!   Each slot is a sequence lock: the writer makes `seq` odd, writes `len` and the payload,
//!   then makes it even with release ordering; a reader copies, re-reads `seq` and discards a
//!   copy whose `seq` changed ([`SlotState::Busy`]). The file's size must equal the header's.
//! - **Outputs** leave `out/` through [`commit_output`]: the checks, then a rename from the
//!   checked directory (or a bounded copy across volumes), then a check that what arrived is
//!   the file that was checked. A writer that may still be alive (a warm worker) is never
//!   given the destination's inode: [`commit_output_copy`] copies from the checked handle into
//!   a host-created temporary file and renames that.

mod checks;
mod layout;
mod open;
mod ring;

#[cfg(feature = "test-hooks")]
pub mod hooks;
#[cfg(not(feature = "test-hooks"))]
mod hooks;

use std::path::{Path, PathBuf};

use fin_core::CoreError;

pub use checks::Reason;
pub use layout::{
    DATA_ALIGN, HEADER_BYTES, MAGIC, MAX_SLOTS, RingLayout, RingSpec, SLOT_ENTRY_BYTES, VERSION,
    decode_header, encode_header,
};
pub use open::{
    checked_open, commit_output, commit_output_copy, create_new_output, create_output,
    create_output_dir, write_input,
};
pub use ring::{RingReader, RingWriter, SlotState};

/// The error code of every refusal (sub-project 6 spec §17, exit 5).
pub const SHM_REFUSED: &str = "SHM_REFUSED";

/// A job's directory, `<server-data>/jobs/<jobId>/` (sub-project 6 spec §3.2 item 3): `in/`
/// (written by the host before `start`, read-only to the worker), `out/` (the worker's) and
/// `helpers/<id>/out/` (each helper's only writable place).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobDir {
    pub root: PathBuf,
}

impl JobDir {
    /// The job directory at `root`, its parent (the host's `jobs/`) canonicalised once, so a
    /// link the host itself has above server data (macOS `/var`, a symlinked home) is resolved,
    /// while a link at the job directory or below it is refused by every open.
    pub fn resolve(root: &Path) -> Result<JobDir, CoreError> {
        let (Some(parent), Some(name)) = (root.parent(), root.file_name()) else {
            return Err(open::refusal(
                root,
                Reason::NotAbsolute,
                "a job directory needs a parent",
            ));
        };
        let parent = std::fs::canonicalize(parent).map_err(|e| open::io_refusal(root, &e))?;
        Ok(JobDir {
            root: parent.join(name),
        })
    }

    /// `in/`: the host's inputs (`snapshot.json`, `manifest.json`).
    pub fn input(&self) -> PathBuf {
        self.root.join("in")
    }

    /// `out/`: the worker's outputs and the packet rings it writes for a helper.
    pub fn output(&self) -> PathBuf {
        self.root.join("out")
    }

    /// `helpers/<id>/out/`: a helper's only writable place. `id` is `[A-Za-z0-9_-]{1,64}`.
    pub fn helper_output(&self, id: &str) -> Result<PathBuf, CoreError> {
        let ok = (1..=64).contains(&id.len())
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !ok {
            return Err(open::refusal(
                Path::new(id),
                Reason::BadName,
                "a helper id is [A-Za-z0-9_-]{1,64}",
            ));
        }
        Ok(self.root.join("helpers").join(id).join("out"))
    }
}
