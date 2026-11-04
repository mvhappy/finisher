//! Hydrating cloud-sync placeholders (spec §3.6 step 3): reading a placeholder's whole file
//! makes the sync client fetch it; the next scan then sees a real file.
//!
//! Only placeholders under `.finisher/segments/` are read: that is what import consumes.
//! Snapshots are read by rebuild (Task 12), which hydrates them by reading them. Legacy
//! iCloud stubs (`.<name>.icloud`) are separate files that reading does not hydrate, so they
//! are skipped. At most [`BUDGET`] bytes are read per pass, in path order starting after the
//! last path the previous pass read, wrapping around ([`next_pass`]). A file that does not
//! fit the rest of the budget waits; because passes take turns, a placeholder that stays a
//! placeholder after a read (a slow sync client) cannot keep the budget from the ones after
//! it. A segment is at most [`SEGMENT_CAP`], below the budget, so each one gets read.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::scan::{self, Entry, NotArrivedWhy, SEGMENT_CAP};

/// Bytes read to hydrate placeholders in one pass.
pub(crate) const BUDGET: u64 = 16 << 20;
// Every segment fits one pass.
const _: () = assert!(SEGMENT_CAP < BUDGET);

/// Which of `candidates` (`(path, length)`, in order) fit `budget`.
fn plan(candidates: &[(PathBuf, u64)], budget: u64) -> Vec<PathBuf> {
    let mut left = budget;
    let mut out = vec![];
    for (path, len) in candidates {
        if *len <= left {
            left -= len;
            out.push(path.clone());
        }
    }
    out
}

/// The candidates (sorted by path) to read this pass: [`plan`] over them in order starting
/// after `after`, wrapping around.
fn next_pass(candidates: &[(PathBuf, u64)], after: Option<&Path>, budget: u64) -> Vec<PathBuf> {
    let start = after.map_or(0, |a| candidates.partition_point(|(p, _)| p.as_path() <= a));
    let turn: Vec<(PathBuf, u64)> = candidates[start..]
        .iter()
        .chain(&candidates[..start])
        .cloned()
        .collect();
    plan(&turn, budget)
}

/// Reads the placeholders of `entries` that fit the budget, taking turns from `cursor` (the
/// last path read, updated); returns those read.
pub(super) fn run(
    project_dir: &Path,
    entries: &[Entry],
    cursor: &mut Option<PathBuf>,
) -> Vec<PathBuf> {
    let root = project_dir.join(".finisher").join("segments");
    let candidates: Vec<(PathBuf, u64)> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::NotArrived {
                path,
                why: NotArrivedWhy::Placeholder,
            } if path.starts_with(&root) => Some(path),
            _ => None,
        })
        .filter(|p| {
            !p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.') && n.ends_with(".icloud"))
        })
        // The directory entry's own metadata: a placeholder reports its full logical size.
        .filter_map(|p| Some((p.clone(), fs::symlink_metadata(p).ok()?.len())))
        .filter(|(_, len)| *len <= SEGMENT_CAP)
        .collect();
    let mut candidates = candidates;
    candidates.sort();
    let turn = next_pass(&candidates, cursor.as_deref(), BUDGET);
    if let Some(last) = turn.last() {
        *cursor = Some(last.clone());
    }
    turn.into_iter()
        .filter(|p| read_through(p).is_ok())
        .inspect(|p| scan::hydrated(p))
        .collect()
}

/// Reads `path` to its end, keeping nothing.
fn read_through(path: &Path) -> io::Result<u64> {
    io::copy(&mut fs::File::open(path)?, &mut io::sink())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_keeps_to_the_budget_and_lets_smaller_files_through() {
        let c = |n: &str, len| (PathBuf::from(n), len);
        let got = plan(&[c("a", 6), c("b", 6), c("c", 6), c("d", 4), c("e", 1)], 16);
        assert_eq!(got, [PathBuf::from("a"), "b".into(), "d".into()]);
        assert!(plan(&[c("big", 17)], 16).is_empty());
    }

    #[test]
    fn passes_take_turns_so_a_large_placeholder_later_in_order_is_read() {
        // Placeholders that stay placeholders after a read (a slow sync client): without
        // turns, `a` and `b` would take the whole budget on every pass.
        let c = |n: &str, len| (PathBuf::from(n), len);
        let cands = [c("a", 8), c("b", 8), c("c", 9)];
        let mut cursor = None;
        let mut seen = vec![];
        for _ in 0..3 {
            let read = next_pass(&cands, cursor.as_deref(), 16);
            cursor = read.last().cloned().or(cursor);
            seen.extend(read);
        }
        assert!(seen.contains(&PathBuf::from("c")), "{seen:?}");
        // The pass after `c` starts again from the front.
        assert_eq!(
            next_pass(&cands, Some(Path::new("c")), 16),
            [PathBuf::from("a"), "b".into()]
        );
        assert_eq!(BUDGET, 16 * 1024 * 1024);
    }
}
