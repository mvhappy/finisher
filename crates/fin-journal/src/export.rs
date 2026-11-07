//! Export and the upload view (plan D, Decisions 30 and 31). Without `as_upload`, an export
//! is what is stored, content included: the user's own data. With it, it is exactly what a
//! future uploader may ship now:
//! - closed files only (never the file being appended to);
//! - an entry is kept only if its stamped purposes, the **current** consent and the
//!   requested purposes share one, and its project (if any) is `default` now (not `never`,
//!   not unknown);
//! - content is removed unless training is stamped, current and requested, and blob
//!   references go with their content;
//! - the stamp shows the purposes that view keeps, never more.

use std::collections::{BTreeMap, VecDeque};

use fin_core::events::{CollectPolicy, Envelope, Purposes, kind, redact};

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use crate::Journal;
use crate::control::Control;
use crate::files;
use crate::handle::Handle;
use crate::read::{Entries, Filter};

/// An export's entries, oldest first.
pub struct Export {
    inner: Box<dyn Iterator<Item = Envelope> + Send>,
}

impl std::fmt::Debug for Export {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Export")
    }
}

impl Iterator for Export {
    type Item = Envelope;

    fn next(&mut self) -> Option<Envelope> {
        self.inner.next()
    }
}

/// The upload view of one entry under the consent and policies in force now.
fn view(
    mut e: Envelope,
    want: Purposes,
    gate: Purposes,
    policies: &BTreeMap<String, Option<CollectPolicy>>,
) -> Option<Envelope> {
    let p = e.consent.purposes.intersect(gate).intersect(want);
    if p.is_empty() {
        return None;
    }
    if let Some(project) = e.ctx.as_ref().and_then(|c| c.project.as_ref())
        && policies.get(project.as_str()) != Some(&Some(CollectPolicy::Default))
    {
        return None;
    }
    let spec = kind(e.kind.as_str(), e.kv)?;
    redact(spec, &mut e.payload, p);
    e.consent.purposes = p;
    Some(e)
}

/// The closed files, from the writer (after a flush). If it does not answer, the files on
/// disk less the active one, or less the newest when none is known to be active.
fn closed_files(h: &Handle) -> Vec<PathBuf> {
    let (ack, rx) = mpsc::sync_channel(1);
    if h.control(Control::Closed(ack))
        && let Ok(names) = rx.recv_timeout(Duration::from_secs(10))
    {
        return names.into_iter().map(|n| h.dir.join(n)).collect();
    }
    let mut listed = files::paths(&h.dir);
    let active = h.sh.lock_status().active.clone();
    match active {
        Some(a) => listed.retain(|p| p.file_name().and_then(|n| n.to_str()) != Some(a.as_str())),
        None => {
            listed.pop();
        }
    }
    listed
}

impl Journal {
    /// The entries matching `filter`, oldest first (with `limit`, the newest that many).
    /// `as_upload: Some(purposes)` gives the upload view (module docs). Flushes first, so
    /// it blocks like [`Journal::read`]: call it off the JS thread.
    pub fn export(&self, filter: &Filter, as_upload: Option<Purposes>) -> Export {
        let Some(want) = as_upload else {
            return Export {
                inner: Box::new(self.read(filter)),
            };
        };
        let Some(h) = &self.h else {
            return Export {
                inner: Box::new(std::iter::empty()),
            };
        };
        let closed = closed_files(h);
        let gate = self.gate().unwrap_or(Purposes::NONE);
        let policies = h.sh.lock_status().policies.clone();
        let unlimited = Filter {
            limit: None,
            ..filter.clone()
        };
        let it =
            Entries::new(closed, &unlimited).filter_map(move |e| view(e, want, gate, &policies));
        let inner: Box<dyn Iterator<Item = Envelope> + Send> = match filter.limit {
            None => Box::new(it),
            Some(n) => {
                let mut newest = VecDeque::with_capacity(n.min(4096));
                for e in it {
                    if newest.len() == n {
                        newest.pop_front();
                    }
                    if n > 0 {
                        newest.push_back(e);
                    }
                }
                Box::new(newest.into_iter())
            }
        };
        Export { inner }
    }
}
