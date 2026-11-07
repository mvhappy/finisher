//! The child of `crash.rs`: records content-sized entries as fast as it can, rotating often,
//! until the parent kills it. It writes [`READY`]'s file after its first flush, so the kill
//! never comes before anything is on disk. With [`TEAR`] set it then tears its next write
//! (half a buffer, inside a line) and writes [`TORN`]'s file once the writer is parked there.

use std::path::{Path, PathBuf};
use std::time::Duration;

use fin_core::events::Source;
use fin_journal::{Journal, JournalOptions};

pub const ENV: &str = "FIN_JOURNAL_CRASH_CHILD";
pub const TEAR: &str = "FIN_JOURNAL_CRASH_TEAR";

/// The marker files beside the journal directory.
pub fn ready(dir: &Path) -> PathBuf {
    dir.with_extension("ready")
}

pub fn torn(dir: &Path) -> PathBuf {
    dir.with_extension("torn")
}

pub fn run(dir: &Path) -> ! {
    let opts = JournalOptions {
        max_file_bytes: 256 * 1024,
        flush_bytes: 8 * 1024,
        flush_every: Duration::from_millis(5),
        ..JournalOptions::default()
    };
    let tear = std::env::var_os(TEAR).is_some();
    let j = Journal::open(dir, super::training(), opts);
    let mut i = 0u64;
    let mut ready = false;
    loop {
        j.record(
            Source::FinNode,
            None,
            super::commit_with_intent(1 + (i % 40) as usize),
        );
        i += 1;
        if !ready && i >= 16 && j.flush().is_ok() {
            std::fs::write(self::ready(dir), b"").unwrap();
            ready = true;
            if tear {
                j.faults().unwrap().tear_writes();
            }
        }
        if ready && tear && j.faults().unwrap().torn() {
            std::fs::write(self::torn(dir), b"").unwrap();
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        if i.is_multiple_of(64) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
