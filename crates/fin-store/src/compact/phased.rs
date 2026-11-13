//! A compaction run through a [`StoreAccess`] (module docs of `compact`, "When" and
//! "Locking"): its folder I/O between the steps that have the store.

use std::collections::HashSet;
use std::time::Instant;

use super::manifest::Manifest;
use super::planned::{Planned, Point};
use super::state::{self, ObservePlan, Observed, Seen, State};
use super::{CompactReport, OVERTAKEN, REPLAN, RETRY, Step, TRIGGER, finish, pack, view};
use crate::names::ManifestName;
use crate::{Store, StoreAccess, StoreError, db};

/// Non-pack segments of `branch` that no present pack replaces, from the last import's scan.
fn unpacked(store: &Store, branch: &str) -> usize {
    let segs: Vec<_> = store
        .import
        .segrefs()
        .filter(|s| s.header().branch == branch)
        .collect();
    let replaced: HashSet<&str> = segs
        .iter()
        .filter(|s| s.header().pack)
        .flat_map(|s| s.header().replaces.iter().map(|r| r.hash.as_str()))
        .collect();
    segs.iter()
        .filter(|s| !s.header().pack && !replaced.contains(s.hash()))
        .count()
}

/// Why compaction cannot run now (read-only, not holding); forgets the manifests seen then.
fn cannot_run(store: &mut Store) -> Option<String> {
    if let Some(r) = store.read_only() {
        store.compact.seen.clear();
        return Some(format!("the project is read-only ({})", r.code()));
    }
    // In memory only: every write and delete takes the full check first.
    if let Err(e) = store.export.lease.quick_check() {
        store.compact.seen.clear();
        return Some(e.message);
    }
    None
}

/// Whether phase 1 should plan `branch` now.
fn branch_due(store: &Store, branch: &str, now: Instant, force: bool) -> bool {
    let pending = store.compact.seen.values().any(|s| s.blocks(branch));
    let due = force
        || (unpacked(store, branch) > TRIGGER
            && store
                .compact
                .retry_plan
                .get(branch)
                .is_none_or(|t| now >= *t));
    !pending && due
}

/// The manifests whose grace period is over (and whose retry time has come).
fn ready(store: &Store, now: Instant) -> Vec<(ManifestName, Manifest)> {
    store
        .compact
        .seen
        .iter()
        .filter_map(|(n, s)| {
            let m = s.manifest.as_ref()?;
            let ready = matches!(s.state, State::Grace | State::Waiting(_))
                && s.observed.is_some_and(|t| {
                    now.saturating_duration_since(t) >= state::grace_of(store, Some(m))
                })
                && s.retry_at.is_none_or(|t| now >= t);
            ready.then(|| (n.clone(), m.clone()))
        })
        .collect()
}

/// The start of a run, with the store: why it cannot run, or what it started against.
fn start(
    store: &mut Store,
    force: bool,
) -> Result<Result<(Planned, ObservePlan, Instant), String>, StoreError> {
    // It may rewrite the folder and the `segments` rows: an import read before is stale.
    store.phase.io_gen += 1;
    if let Some(why) = cannot_run(store) {
        return Ok(Err(why));
    }
    let now = store.ctl.now();
    if force {
        store.compact.retry_plan.clear();
        state::set_abandoned(store, &Default::default())?;
        for s in store.compact.seen.values_mut() {
            s.retry_at = None;
            if matches!(s.state, State::Abandoned(_)) {
                s.state = State::Grace;
            }
        }
    }
    Ok(Ok((Planned::new(store), ObservePlan::new(store), now)))
}

/// One compaction run through `access` (module docs), doing at most `units` steps (a
/// manifest's phase 2 or a branch's phase 1); what is left stays due for the next run. `Err`
/// only for a database failure (or an injected fault); folder problems are in the report and
/// status. `None`: `access` lost the store between two steps.
pub(crate) fn run_with<A: StoreAccess>(
    access: &mut A,
    now_ms: i64,
    force: bool,
    mut units: usize,
) -> Result<Option<CompactReport>, StoreError> {
    let mut report = CompactReport::default();
    let (planned, observe, now) = match with!(access, |s| start(s, force))? {
        Ok(started) => started,
        Err(why) => {
            report.skipped = Some(why);
            return Ok(Some(report));
        }
    };
    let read = observe.read();
    planned.at(Point::CompactList);
    match with!(access, |s| state::observe(s, &planned, read, now, now_ms))? {
        Observed::Listed => {}
        Observed::Unlisted => return Ok(Some(report)),
        Observed::Overtaken => {
            report.skipped = Some(OVERTAKEN.into());
            return Ok(Some(report));
        }
    }
    let mut stopped = false;
    for (name, m) in with!(access, |s| ready(s, now)) {
        if units == 0 {
            break;
        }
        units -= 1;
        let mut deleted = 0;
        let step = finish::run(access, &planned, &name, &m, &mut deleted)?;
        report.deleted += deleted;
        let Some(step) = step else {
            return Ok(None);
        };
        if with!(access, |s| phase2_done(s, &mut report, name, step, now))? {
            stopped = true;
            break;
        }
    }
    for branch in with!(access, |s| db::branches(&s.conn))? {
        if units == 0 || stopped {
            break;
        }
        if !with!(access, |s| branch_due(s, &branch, now, force)) {
            continue;
        }
        units -= 1;
        let Some(stop) = phase1(access, &planned, &mut report, &branch, now, now_ms)? else {
            return Ok(None);
        };
        if stop {
            break;
        }
    }
    // Also after a stop: what was done before it counts.
    with!(access, |s| {
        let worked =
            !report.manifests.is_empty() || !report.finished.is_empty() || report.deleted > 0;
        if worked {
            s.compact.last_run_ms = Some(now_ms);
        }
        s.compact.last_error = report
            .waiting
            .last()
            .or(report.abandoned.last())
            .map(|(what, why)| format!("{what}: {why}"));
    });
    Ok(Some(report))
}

/// The cheap part of a run (`Store::tick` under `TickOptions { compaction: false }`): observes
/// the manifests (their grace periods start here as in a run) and tells whether a run would
/// do anything: a manifest past its grace, or a branch to plan. Writes nothing to the folder;
/// lists and reads `compaction/` without the store. `None`: `access` lost the store.
pub(crate) fn due_with<A: StoreAccess>(
    access: &mut A,
    now_ms: i64,
) -> Result<Option<bool>, StoreError> {
    let started = with!(access, |s| {
        cannot_run(s)
            .is_none()
            .then(|| (Planned::new(s), ObservePlan::new(s), s.ctl.now()))
    });
    let Some((planned, observe, now)) = started else {
        return Ok(Some(false));
    };
    let read = observe.read();
    planned.at(Point::CompactList);
    with!(access, |s| -> Result<bool, StoreError> {
        if state::observe(s, &planned, read, now, now_ms)? != Observed::Listed {
            return Ok(false);
        }
        if !ready(s, now).is_empty() {
            return Ok(true);
        }
        for branch in db::branches(&s.conn)? {
            if branch_due(s, &branch, now, false) {
                return Ok(true);
            }
        }
        Ok(false)
    })
    .map(Some)
}

/// Notes what phase 2 of manifest `name` did (with the store). `true`: the run stops (the
/// lease is lost, or the step was overtaken).
fn phase2_done(
    store: &mut Store,
    report: &mut CompactReport,
    name: ManifestName,
    step: Step<()>,
    now: Instant,
) -> Result<bool, StoreError> {
    let label = name.to_string();
    match step {
        Step::Done(()) => {
            store.compact.seen.remove(&name);
            report.finished.push(label);
        }
        Step::Gone => {
            store.compact.seen.remove(&name);
        }
        Step::Wait(why) => {
            if let Some(s) = store.compact.seen.get_mut(&name) {
                s.state = State::Waiting(why.clone());
                s.retry_at = Some(now + RETRY);
            }
            report.waiting.push((label, why));
        }
        Step::Abandon(why) => {
            let mut all = state::abandoned(store)?;
            all.insert(label.clone(), why.clone());
            state::set_abandoned(store, &all)?;
            if let Some(s) = store.compact.seen.get_mut(&name) {
                s.state = State::Abandoned(why.clone());
            }
            report.abandoned.push((label, why));
        }
        Step::Stopped(why) => {
            report.skipped = Some(why);
            store.compact.seen.clear();
            return Ok(true);
        }
        Step::Overtaken => {
            report.skipped = Some(OVERTAKEN.into());
            return Ok(true);
        }
    }
    Ok(false)
}

/// A branch's fresh read checked with the store: the view, the segment hashes and names an
/// abandoned compaction holds, and what planning needs from the store.
type Checked = (view::View, HashSet<String>, HashSet<String>, pack::Ctx);

/// Phase 1 of `branch`. `Some(true)`: stop (the lease is lost, or the step was overtaken).
fn phase1<A: StoreAccess>(
    access: &mut A,
    planned: &Planned,
    report: &mut CompactReport,
    branch: &str,
    now: Instant,
    now_ms: i64,
) -> Result<Option<bool>, StoreError> {
    let vplan = with!(access, |s| view::plan(s, branch))?;
    // Without the store: the full lease check, the fresh read.
    if let super::Fence::Lost(_) = planned.check(0) {
        return Ok(Some(true));
    }
    let scanned = vplan.scan();
    planned.at(Point::CompactRead);
    let wait = |store: &mut Store, report: &mut CompactReport, why: String| {
        store
            .compact
            .retry_plan
            .insert(branch.to_string(), now + RETRY);
        report.waiting.push((branch.to_string(), why));
    };
    // With it: the read against the database now, what an abandoned compaction holds, and
    // what planning needs from the store.
    let checked = with!(access, |s| -> Result<Option<Option<Checked>>, StoreError> {
        if !planned.fresh(s) {
            return Ok(None);
        }
        let v = match scanned.map(|sc| sc.finish(s)) {
            Ok(r) => r?,
            Err(why) => Err(why),
        };
        let v = match v {
            Ok(v) => v,
            Err(why) => {
                wait(s, report, why);
                return Ok(Some(None));
            }
        };
        // What an abandoned compaction of this branch holds is left alone.
        let (mut protected, mut protected_names) = (HashSet::new(), HashSet::new());
        for seen in s.compact.seen.values() {
            if let Some(m) = seen.manifest.as_ref().filter(|m| m.branch == branch) {
                protected_names.extend(m.replaced.iter().cloned());
                for p in &m.packs {
                    if let Some(seg) = v.by_name.get(p) {
                        protected.extend(seg.header().replaces.iter().map(|r| r.hash.clone()));
                    }
                }
            }
        }
        Ok(Some(Some((
            v,
            protected,
            protected_names,
            pack::Ctx::new(s)?,
        ))))
    })?;
    let (v, protected, protected_names, ctx) = match checked {
        None => {
            report.skipped = Some(OVERTAKEN.into());
            return Ok(Some(true));
        }
        Some(None) => return Ok(Some(false)),
        Some(Some(checked)) => checked,
    };
    // Without it: the packs encoded and hashed (each pack's row and the manifest are written
    // only if nothing overtook the run meanwhile, `pack::write`).
    let Some(plan) = pack::plan(ctx, branch, &v, &protected, &protected_names)? else {
        // Overtaken meanwhile: the next run plans the branch afresh, without the delay.
        with!(access, |s| if planned.fresh(s) {
            s.compact
                .retry_plan
                .insert(branch.to_string(), now + REPLAN);
        });
        return Ok(Some(false));
    };
    let packs = u32::try_from(plan.packs.len()).unwrap_or(u32::MAX);
    let replaced: usize = plan.packs.iter().map(|p| p.replaced.len()).sum();
    let Some(step) = pack::write(access, planned, branch, plan, now_ms)? else {
        return Ok(None);
    };
    Ok(Some(with!(access, |s| match step {
        Step::Done((name, m)) => {
            report.manifests.push(name.to_string());
            report.packs_written += packs;
            report.replaced += u32::try_from(replaced).unwrap_or(u32::MAX);
            // Overtaken after the write: the next run's listing notes it afresh.
            if planned.fresh(s) {
                let g = state::grace_of(s, Some(&m));
                let ends = now_ms.saturating_add(i64::try_from(g.as_millis()).unwrap_or(i64::MAX));
                s.compact.seen.insert(name, Seen::written(m, now, ends));
            }
            false
        }
        Step::Wait(why) | Step::Abandon(why) => {
            wait(s, report, why);
            false
        }
        Step::Stopped(why) => {
            report.skipped = Some(why);
            true
        }
        Step::Overtaken | Step::Gone => {
            report.skipped = Some(OVERTAKEN.into());
            true
        }
    })))
}
