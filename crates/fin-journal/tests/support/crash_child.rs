//! The child of `crash.rs`: records content-sized entries as fast as it can, rotating often,
//! until the parent kills it.

use std::path::Path;
use std::time::Duration;

use fin_core::events::Source;
use fin_journal::{Journal, JournalOptions};

pub const ENV: &str = "FIN_JOURNAL_CRASH_CHILD";

pub fn run(dir: &Path) -> ! {
    let opts = JournalOptions {
        max_file_bytes: 256 * 1024,
        flush_bytes: 8 * 1024,
        flush_every: Duration::from_millis(5),
        ..JournalOptions::default()
    };
    let j = Journal::open(dir, super::training(), opts);
    let mut i = 0u64;
    loop {
        j.record(
            Source::FinNode,
            None,
            super::commit_with_intent(1 + (i % 40) as usize),
        );
        i += 1;
        if i.is_multiple_of(64) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
