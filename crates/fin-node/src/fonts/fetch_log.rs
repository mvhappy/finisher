//! `fonts.fetch` (plan D, Decision 27): one entry per font file fetch, from the fetcher's
//! state changes (and `fonts.fetch` asking for a file already cached). The catalog entry's
//! family is keyed-hashed: a pinned family may be a user's local font, and the catalog is not
//! in `fin-core`'s tables.
//!
//! - `Fetching` then `Cached`: `ok`, with the file's size;
//! - `Cached` without a download seen (another fetcher placed it), or asked for while cached:
//!   `cached`;
//! - `Failed`: `failed`, `FONT_FETCH_FAILED`.
//!
//! `micros` runs from the first `Queued` or `Fetching` seen. Nothing is tracked while the
//! journal is off.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use fin_core::events::Code;
use fin_core::events::Source;
use fin_core::events::payloads::{FetchOutcome, FontsFetch};
use fin_fonts::{FileState, FontCatalog};

use crate::engine::lock;

pub(crate) struct FetchLog {
    server_data: PathBuf,
    /// Fetches under way: since when, and whether a download was seen.
    seen: Mutex<HashMap<String, (Instant, bool)>>,
}

impl FetchLog {
    pub(crate) fn new(server_data: &Path) -> FetchLog {
        FetchLog {
            server_data: server_data.to_path_buf(),
            seen: Mutex::new(HashMap::new()),
        }
    }

    fn journal(&self) -> Option<fin_journal::Journal> {
        let j = crate::journal::for_server(&self.server_data);
        j.gate().map(|_| j)
    }

    /// A state change the fetcher published for `sha`.
    pub(crate) fn changed(&self, catalog: &FontCatalog, sha: &str, state: &FileState) {
        let Some(j) = self.journal() else {
            lock(&self.seen).remove(sha);
            return;
        };
        let done = |seen: Option<(Instant, bool)>| {
            seen.map_or(0, |(t, _)| {
                u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX)
            })
        };
        match state {
            FileState::Queued | FileState::Fetching { .. } => {
                let fetching = matches!(state, FileState::Fetching { .. });
                let mut seen = lock(&self.seen);
                let e = seen
                    .entry(sha.to_string())
                    .or_insert((Instant::now(), false));
                e.1 |= fetching;
            }
            FileState::Cached => {
                let seen = lock(&self.seen).remove(sha);
                let downloaded = seen.is_some_and(|(_, d)| d);
                let (outcome, bytes) = match downloaded {
                    true => (FetchOutcome::Ok, size(catalog, sha)),
                    false => (FetchOutcome::Cached, 0),
                };
                record(&j, catalog, sha, outcome, bytes, done(seen), None);
            }
            FileState::Failed { .. } => {
                let seen = lock(&self.seen).remove(sha);
                let code = Code::parse("FONT_FETCH_FAILED");
                record(&j, catalog, sha, FetchOutcome::Failed, 0, done(seen), code);
            }
            FileState::Missing => {
                lock(&self.seen).remove(sha);
            }
        }
    }

    /// `fonts.fetch` asked for `sha`, which is cached already.
    pub(crate) fn already_cached(&self, catalog: &FontCatalog, sha: &str) {
        if let Some(j) = self.journal() {
            record(&j, catalog, sha, FetchOutcome::Cached, 0, 0, None);
        }
    }
}

fn size(catalog: &FontCatalog, sha: &str) -> u64 {
    catalog.by_sha(sha).map_or(0, |(_, s)| s.file.size)
}

fn record(
    j: &fin_journal::Journal,
    catalog: &FontCatalog,
    sha: &str,
    outcome: FetchOutcome,
    bytes: u64,
    micros: u64,
    code: Option<Code>,
) {
    let Some((entry, _)) = catalog.by_sha(sha) else {
        return;
    };
    let Some(family) = j.hash(&entry.family) else {
        return;
    };
    let p = FontsFetch {
        entry: family,
        // In the catalog built into this binary (not one a test build loaded instead).
        bundled: FontCatalog::builtin().by_sha(sha).is_some(),
        bytes,
        micros,
        outcome,
        code,
    };
    j.record(Source::FinNode, None, p.into());
}
