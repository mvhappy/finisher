//! Reading the journal back (plan D, Decisions 3 and 17): every `j-*` file oldest first, one
//! entry per line. A line that does not parse (a torn tail, a hand edit) is skipped and
//! counted; so is an entry whose kind or version this build does not know.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use fin_core::events::{Envelope, kind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Which entries to read. Empty fields match everything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Filter {
    /// Entries with `ts` ≥ this (ms since the epoch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub since: Option<u64>,
    /// Entries with `ts` < this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub until: Option<u64>,
    /// Entries whose `ctx.project` is this id (in any case).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub project: Option<String>,
    /// Entries of these kinds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
    /// Only the newest `limit` matches (still oldest first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub limit: Option<usize>,
}

impl Filter {
    pub fn matches(&self, e: &Envelope) -> bool {
        self.since.is_none_or(|s| e.ts >= s)
            && self.until.is_none_or(|u| e.ts < u)
            && self.project.as_deref().is_none_or(|p| {
                e.ctx
                    .as_ref()
                    .and_then(|c| c.project.as_ref())
                    .is_some_and(|x| x.as_str().eq_ignore_ascii_case(p))
            })
            && (self.kinds.is_empty() || self.kinds.iter().any(|k| k == e.kind.as_str()))
    }
}

/// The entries of a read, oldest first. [`Entries::skipped`] and [`Entries::unknown`] count
/// what was passed over so far (all of it once the iterator is exhausted, or at once when the
/// filter has a `limit`).
#[derive(Debug)]
pub struct Entries {
    files: VecDeque<PathBuf>,
    lines: Option<BufReader<File>>,
    filter: Filter,
    buf: Vec<u8>,
    ready: Option<VecDeque<Envelope>>,
    skipped: u64,
    unknown: u64,
}

impl Entries {
    pub(crate) fn new(files: Vec<PathBuf>, filter: &Filter) -> Entries {
        let mut e = Entries {
            files: files.into(),
            lines: None,
            filter: filter.clone(),
            buf: Vec::new(),
            ready: None,
            skipped: 0,
            unknown: 0,
        };
        if let Some(n) = filter.limit {
            let mut newest = VecDeque::with_capacity(n.min(4096));
            while let Some(x) = e.next_match() {
                if newest.len() == n {
                    newest.pop_front();
                }
                if n > 0 {
                    newest.push_back(x);
                }
            }
            e.ready = Some(newest);
        }
        e
    }

    pub(crate) fn empty() -> Entries {
        Entries::new(Vec::new(), &Filter::default())
    }

    /// Lines that did not parse as an entry.
    pub fn skipped(&self) -> u64 {
        self.skipped
    }

    /// Entries of a kind or version this build does not know.
    pub fn unknown(&self) -> u64 {
        self.unknown
    }

    fn next_line(&mut self) -> Option<bool> {
        loop {
            if self.lines.is_none() {
                let path = self.files.pop_front()?;
                // A file the writer removed since it was listed is simply gone.
                self.lines = File::open(path).ok().map(BufReader::new);
                continue;
            }
            self.buf.clear();
            let r = self.lines.as_mut()?.read_until(b'\n', &mut self.buf);
            match r {
                Ok(0) | Err(_) => self.lines = None,
                Ok(_) => return Some(self.buf.last() == Some(&b'\n')),
            }
        }
    }

    fn next_match(&mut self) -> Option<Envelope> {
        while let Some(whole) = self.next_line() {
            let parsed = whole
                .then(|| serde_json::from_slice::<Envelope>(&self.buf).ok())
                .flatten();
            let Some(e) = parsed else {
                self.skipped += 1;
                continue;
            };
            if kind(e.kind.as_str(), e.kv).is_none() {
                self.unknown += 1;
                continue;
            }
            if self.filter.matches(&e) {
                return Some(e);
            }
        }
        None
    }
}

impl Iterator for Entries {
    type Item = Envelope;

    fn next(&mut self) -> Option<Envelope> {
        match &mut self.ready {
            Some(q) => q.pop_front(),
            None => self.next_match(),
        }
    }
}
