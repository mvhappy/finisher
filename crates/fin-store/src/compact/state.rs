//! What compaction remembers between runs: the manifests seen and their grace periods (in
//! memory: a grace period is never carried over a restart or a lost lease), and the
//! abandoned ones (in the database's `meta`, `compactionAbandoned`: an abandoned manifest is
//! never acted on again, across restarts, until `fin project compact` checks it again).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;

#[cfg(any(test, feature = "test-hooks"))]
use super::CompactFail;
use super::grace;
use super::manifest::{self, Manifest};
use crate::names::ManifestName;
use crate::{Store, StoreError, db, fsio};

const ABANDONED_KEY: &str = "compactionAbandoned";

/// What one compaction run did (serialised camelCase; `waiting` and `abandoned` as
/// `[name, why]` pairs).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactReport {
    /// Why nothing ran (not holding the lease, read-only).
    pub skipped: Option<String>,
    /// Manifests written (phase 1).
    pub manifests: Vec<String>,
    pub packs_written: u32,
    /// Segments the new manifests replace.
    pub replaced: u32,
    /// Manifests finished (phase 2) and the segment files deleted.
    pub finished: Vec<String>,
    pub deleted: u32,
    /// `(manifest or branch, why)`: steps that wait and are retried.
    pub waiting: Vec<(String, String)>,
    /// `(manifest, why)`: abandoned this run.
    pub abandoned: Vec<(String, String)>,
}

/// One manifest as status shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingCompaction {
    pub manifest: String,
    pub branch: Option<String>,
    pub writer_id: Option<String>,
    /// `arriving` (its packs or snapshot are not all here yet), `grace`, `waiting`,
    /// `abandoned` or `unreadable`.
    pub state: String,
    pub reason: Option<String>,
    /// Unix ms (this machine's clock, informational) when the grace period ends, once it runs.
    pub grace_ends_ms: Option<i64>,
}

/// Compaction in `fin project status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionStatus {
    /// Manifests seen while holding the lease.
    pub pending: Vec<PendingCompaction>,
    /// Unix ms of the last run that wrote or deleted something.
    pub last_run_ms: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum State {
    Grace,
    Waiting(String),
    Abandoned(String),
    Unreadable(String),
}

pub(super) struct Seen {
    pub(super) manifest: Option<Manifest>,
    /// When its packs and snapshot were first all seen present (the grace period runs from
    /// then); `None` until they are.
    pub(super) observed: Option<Instant>,
    pub(super) grace_ends_ms: Option<i64>,
    pub(super) state: State,
    pub(super) retry_at: Option<Instant>,
}

impl Seen {
    /// A manifest this server just wrote, its packs and snapshot in place.
    pub(super) fn written(m: Manifest, now: Instant, ends_ms: i64) -> Seen {
        Seen {
            manifest: Some(m),
            observed: Some(now),
            grace_ends_ms: Some(ends_ms),
            state: State::Grace,
            retry_at: None,
        }
    }

    /// Whether it keeps phase 1 off `branch`: readable for that branch and not abandoned, or
    /// unreadable (its branch is unknown).
    pub(super) fn blocks(&self, branch: &str) -> bool {
        match (&self.manifest, &self.state) {
            (_, State::Abandoned(_)) => false,
            (None, _) => true,
            (Some(m), _) => m.branch == branch,
        }
    }
}

/// Compaction state held by the `Store`.
#[derive(Default)]
pub(crate) struct CompactState {
    pub(super) seen: BTreeMap<ManifestName, Seen>,
    /// Phase 1 of a branch that had to wait is retried after this.
    pub(super) retry_plan: HashMap<String, Instant>,
    pub(super) last_run_ms: Option<i64>,
    pub(super) last_error: Option<String>,
    /// Test hook: a grace period replacing [`grace`].
    pub(crate) grace: Option<Duration>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fail_at: Option<CompactFail>,
    #[cfg(any(test, feature = "test-hooks"))]
    #[allow(clippy::type_complexity)]
    pub(super) on_delete: Option<Box<dyn FnMut(&Path) + Send>>,
}

/// The abandoned manifests recorded in the database: name → why.
pub(super) fn abandoned(store: &Store) -> Result<BTreeMap<String, String>, StoreError> {
    Ok(db::meta(&store.conn, ABANDONED_KEY)?
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default())
}

pub(super) fn set_abandoned(
    store: &Store,
    all: &BTreeMap<String, String>,
) -> Result<(), StoreError> {
    db::set_meta(&store.conn, ABANDONED_KEY, &serde_json::to_string(all)?)
}

/// The grace period of `m` here: the longer of this folder's and the writer's.
pub(super) fn grace_of(store: &Store, m: Option<&Manifest>) -> Duration {
    store.compact.grace.unwrap_or_else(|| {
        let kind = store.storage_kind();
        grace(kind).max(grace(m.map_or(kind, Manifest::writer_kind)))
    })
}

/// The regular, downloaded files directly in `dir`.
fn present_files(dir: &Path) -> HashSet<String> {
    // Nothing is present through a link (`crate::reserved`).
    if crate::reserved::no_links(dir).is_err() {
        return HashSet::new();
    }
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.metadata()
                .is_ok_and(|m| m.is_file() && !fsio::is_placeholder(&e.path(), &m))
        })
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

/// Notes every manifest in `names` and forgets the rest. A manifest's grace period starts at
/// the first run that finds all its packs and its snapshot present (on a sync folder the
/// manifest may arrive first); one that names something no compaction may act on is
/// abandoned at once; the database's abandoned list is pruned to the manifests still there.
pub(super) fn observe(
    store: &mut Store,
    dir: &Path,
    names: &[ManifestName],
    now: Instant,
    now_ms: i64,
) -> Result<(), StoreError> {
    let mut gone = abandoned(store)?;
    let before = gone.len();
    gone.retain(|n, _| names.iter().any(|m| m.to_string() == *n));
    if gone.len() != before {
        set_abandoned(store, &gone)?;
    }
    store.compact.seen.retain(|n, _| names.contains(n));
    for name in names {
        let label = name.to_string();
        if store
            .compact
            .seen
            .get(name)
            .is_none_or(|s| s.manifest.is_none())
        {
            let (manifest, state) = match manifest::read(&dir.join(&label)) {
                manifest::Read::Ok(m) => {
                    let state = match (gone.get(&label), m.invalid(&store.project_id)) {
                        (Some(why), _) => State::Abandoned(why.clone()),
                        (None, Some(why)) => {
                            gone.insert(label.clone(), why.clone());
                            set_abandoned(store, &gone)?;
                            State::Abandoned(why)
                        }
                        (None, None) => State::Grace,
                    };
                    (Some(m), state)
                }
                manifest::Read::Gone => continue,
                manifest::Read::Unreadable(why) => (None, State::Unreadable(why)),
            };
            store.compact.seen.insert(
                name.clone(),
                Seen {
                    manifest,
                    observed: None,
                    grace_ends_ms: None,
                    state,
                    retry_at: None,
                },
            );
        }
        let s = &store.compact.seen[name];
        let Some(m) = s.manifest.as_ref().filter(|_| s.observed.is_none()) else {
            continue;
        };
        if matches!(s.state, State::Abandoned(_)) {
            continue;
        }
        let dot = store.project_dir.join(".finisher");
        let segs = present_files(&dot.join("segments").join(&m.branch));
        let complete = m.packs.iter().all(|p| segs.contains(p))
            && m.snapshot
                .as_ref()
                .is_none_or(|n| present_files(&dot.join("snapshots").join(&m.branch)).contains(n));
        if complete {
            let g = grace_of(store, Some(m));
            let s = store.compact.seen.get_mut(name).expect("inserted above");
            s.observed = Some(now);
            s.grace_ends_ms =
                Some(now_ms.saturating_add(i64::try_from(g.as_millis()).unwrap_or(i64::MAX)));
        }
    }
    Ok(())
}

impl CompactState {
    pub(crate) fn status(&self) -> CompactionStatus {
        let pending = self
            .seen
            .iter()
            .map(|(n, s)| {
                let (state, reason) = match &s.state {
                    State::Grace if s.observed.is_none() => ("arriving", None),
                    State::Grace => ("grace", None),
                    State::Waiting(w) => ("waiting", Some(w.clone())),
                    State::Abandoned(w) => ("abandoned", Some(w.clone())),
                    State::Unreadable(w) => ("unreadable", Some(w.clone())),
                };
                PendingCompaction {
                    manifest: n.to_string(),
                    branch: s.manifest.as_ref().map(|m| m.branch.clone()),
                    writer_id: s.manifest.as_ref().map(|m| m.writer_id.clone()),
                    state: state.to_string(),
                    reason,
                    grace_ends_ms: s.grace_ends_ms,
                }
            })
            .collect();
        CompactionStatus {
            pending,
            last_run_ms: self.last_run_ms,
            last_error: self.last_error.clone(),
        }
    }
}

impl Store {
    /// Compacts now (`fin project compact`): phase 1 of every branch with at least two
    /// segments worth packing, whatever their number, and phase 2 of every manifest past its
    /// grace period (`force` never shortens it). Abandoned manifests are checked again. Only
    /// the lease holder compacts; otherwise the report says why nothing ran.
    pub fn compact(&mut self, force: bool) -> Result<CompactReport, StoreError> {
        super::run(self, crate::commit::now_ms(), force)
    }

    /// One step of the compaction a tick found due ([`Store::due_work`]): one manifest's
    /// phase 2 or one branch's phase 1. A later tick finds the rest due.
    pub fn run_due_compaction_step(&mut self) -> Result<CompactReport, StoreError> {
        self.due.compaction = false;
        super::run_bounded(self, crate::commit::now_ms(), false, 1)
    }

    /// Compaction as status shows it.
    pub fn compaction_status(&self) -> CompactionStatus {
        self.compact.status()
    }

    /// Test hook: the next compaction stops at `point` with `TEST_FAULT`.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn fail_next_compaction_at(&mut self, point: CompactFail) {
        self.compact.fail_at = Some(point);
    }

    /// Test hook: the grace period between the phases, replacing the spec's.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn set_compaction_grace_for_test(&mut self, grace: Duration) {
        self.compact.grace = Some(grace);
    }

    /// Test hook: `f` runs after each replaced segment phase 2 deletes.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn on_compaction_delete_for_test(&mut self, f: Box<dyn FnMut(&Path) + Send>) {
        self.compact.on_delete = Some(f);
    }
}
