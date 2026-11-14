//! The child of `purge.rs`'s kill test: purges [`super::P1`] from the journal the parent
//! wrote, parking the writer for good after two file rewrites, and writes [`parked`]'s file
//! once it has, so the parent kills it mid-purge. With [`AT_RENAME`] it parks between the
//! first rewrite's temp file and its rename.

use std::path::{Path, PathBuf};
use std::time::Duration;

use fin_journal::{Journal, PurgeSelector};

pub const ENV: &str = "FIN_JOURNAL_PURGE_CHILD";
/// Set: park between the first rewrite's temp file and its rename instead.
pub const AT_RENAME: &str = "FIN_JOURNAL_PURGE_AT_RENAME";

pub fn parked(dir: &Path) -> PathBuf {
    dir.with_extension("parked")
}

pub fn run(dir: &Path) -> ! {
    let c = super::clock();
    let j = Journal::open(dir, super::diagnostics(), super::opts(&c));
    j.flush().unwrap();
    if std::env::var_os(AT_RENAME).is_some() {
        j.faults().unwrap().park_before_purge_rename();
    } else {
        j.faults().unwrap().park_purge_after(2);
    }
    let purge = j.clone();
    std::thread::spawn(move || purge.purge(PurgeSelector::Project(super::P1.into())));
    while !j.faults().unwrap().torn() {
        std::thread::sleep(Duration::from_millis(2));
    }
    std::fs::write(parked(dir), b"").unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
