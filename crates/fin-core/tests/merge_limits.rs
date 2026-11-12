//! Merges limited to what was reviewed (sub-project 5 spec §8.4; plan 5E Decisions 6, 7):
//! `include` (origin roots) and `sourceUpTo` (a seq on the source branch itself) bound what a
//! merge copies; what they leave out is `notIncluded`, and what depends on it is
//! `DEPENDS_ON_SKIPPED`. The explain sink (the review plan) reports one status per entry, and
//! it never disagrees with the plan.

mod common;

use std::collections::BTreeSet;

use common::memlog::MemLog;
use common::*;
use fin_core::history::{
    EntryStatus, LogView, MergeLimits, MergePlan, ReviewEntry, plan_merge_limited,
};
use fin_core::rebase::reasons;
use fin_core::{CoreError, Edge, Op, Project, Rational};
use proptest::prelude::*;

/// `c1` [0,10), `c2` [10,20), `c3` [20,30) on `t1`, asset `a1`.
fn genesis() -> Project {
    with_clips(&[10, 10, 10])
}

fn ins(at: i64, dur: i64) -> Op {
    insert("$x", "t1", secs(at), "a1", Rational::zero(), secs(dur))
}

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

/// `br1` forked from `main` at 0 with three independent edits `e1`–`e3` (each deletes one of
/// `c1`–`c3`).
fn three_edits() -> MemLog {
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    for (tx, clip) in [("e1", "c1"), ("e2", "c2"), ("e3", "c3")] {
        log.commit("br1", tx, "alice", vec![delete(clip)]);
    }
    log
}

/// Plans `from` into `into` under `limits` (and collects the explain entries) without
/// appending anything.
fn plan(
    log: &MemLog,
    from: &str,
    into: &str,
    limits: MergeLimits<'_>,
    skip: bool,
) -> (Result<MergePlan, CoreError>, Vec<ReviewEntry>) {
    let mut entries = Vec::new();
    let mut sink = |e: ReviewEntry| entries.push(e);
    let r = plan_merge_limited(log, from, into, "carol", "m", skip, limits, Some(&mut sink));
    (r, entries)
}

/// Plans and appends the copies.
fn merge(log: &mut MemLog, from: &str, into: &str, limits: MergeLimits<'_>) -> MergePlan {
    let p = plan_merge_limited(log, from, into, "carol", "m", false, limits, None).unwrap();
    for planned in p.planned.clone() {
        log.append(into, planned);
    }
    p
}

fn clip_ids(p: &Project) -> Vec<String> {
    starts(p, 0).into_iter().map(|(id, _)| id).collect()
}

fn statuses(entries: &[ReviewEntry]) -> Vec<(&str, EntryStatus)> {
    entries
        .iter()
        .map(|e| (e.tx_id.as_str(), e.status))
        .collect()
}

#[test]
fn include_copies_only_named_origins() {
    let mut log = three_edits();
    let include = set(&["e1", "e3"]);
    let p = merge(
        &mut log,
        "br1",
        "main",
        MergeLimits {
            include: Some(&include),
            source_up_to: None,
        },
    );
    assert_eq!(p.merged, ["e1", "e3"]);
    assert_eq!(p.not_included, ["e2"]);
    assert_eq!(clip_ids(&log.state("main")), ["c2"]);
    // What was left out is offered again by a plain merge.
    let rest = merge(&mut log, "br1", "main", MergeLimits::default());
    assert_eq!(rest.merged, ["e2"]);
    assert!(rest.not_included.is_empty());
    assert_eq!(log.state("main"), log.state("br1"));
}

#[test]
fn up_to_bounds_source_branch_only() {
    // br1: e1 (seq 1); br2 forks from br1 at 1: e2 (seq 2), e3 (seq 3).
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    log.commit("br1", "e1", "alice", vec![delete("c1")]);
    log.fork("br2", "br1", 1);
    log.commit("br2", "e2", "alice", vec![delete("c2")]);
    log.commit("br2", "e3", "alice", vec![delete("c3")]);
    let up_to = |n| MergeLimits {
        include: None,
        source_up_to: Some(n),
    };

    // Below the source's fork, an ancestor's candidate is older than the fork and passes.
    let (zero, _) = plan(&log, "br2", "main", up_to(0), false);
    let zero = zero.unwrap();
    assert_eq!(zero.merged, ["e1"]);
    assert_eq!(zero.not_included, ["e2", "e3"]);

    let two = merge(&mut log, "br2", "main", up_to(2));
    assert_eq!(two.merged, ["e1", "e2"]);
    assert_eq!(two.not_included, ["e3"]);
    assert_eq!(clip_ids(&log.state("main")), ["c3"]);
}

#[test]
fn not_included_reported() {
    // br2 brought e1 in from br1 (copy `mb-1`, whose root is e1), then made e2 and e3.
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    log.fork("br2", "main", 0);
    log.commit("br1", "e1", "alice", vec![delete("c1")]);
    log.merge("br1", "br2", "alice", "mb", false).unwrap();
    log.commit("br2", "e2", "alice", vec![delete("c2")]);
    log.commit("br2", "e3", "alice", vec![delete("c3")]);
    let include = set(&["e2"]);
    let limits = MergeLimits {
        include: Some(&include),
        source_up_to: None,
    };
    let (p, entries) = plan(&log, "br2", "main", limits, false);
    let p = p.unwrap();
    assert_eq!(p.merged, ["e2"]);
    // Origin roots, oldest first: the copy is reported by the root it stands for.
    assert_eq!(p.not_included, ["e1", "e3"]);
    assert_eq!(
        statuses(&entries),
        [
            ("mb-1", EntryStatus::Excluded),
            ("e2", EntryStatus::WillMerge),
            ("e3", EntryStatus::Excluded),
        ]
    );
    assert_eq!(entries[0].origin, "e1");
    assert_eq!(entries[0].reason.as_deref(), Some(reasons::NOT_INCLUDED));
    assert_eq!(entries[1].reason, None);
    assert_eq!((entries[1].branch.as_str(), entries[1].seq), ("br2", 2));
    assert_eq!(entries[1].ops_count, 1);
    assert_eq!(entries[1].author, "alice");
}

#[test]
fn included_depending_on_excluded_conflicts() {
    // e1 trims c3 and inserts X (c4) after it; e2 trims X; e3 is independent.
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    let trim_c3 = trim("c3", Edge::End, secs(-2));
    log.commit("br1", "e1", "alice", vec![trim_c3, ins(28, 2)]);
    log.commit("br1", "e2", "alice", vec![trim("c4", Edge::End, secs(-1))]);
    log.commit("br1", "e3", "alice", vec![delete("c1")]);
    let include = set(&["e2", "e3"]);
    let limits = || MergeLimits {
        include: Some(&include),
        source_up_to: None,
    };
    let before = log.state("main");
    let (err, entries) = plan(&log, "br1", "main", limits(), false);
    let err = err.unwrap_err();
    assert_eq!(err.code, "CONFLICT");
    let c = &err.details["conflicts"][0];
    assert_eq!(
        (
            c["txId"].as_str(),
            c["reason"].as_str(),
            c["againstTx"].as_str()
        ),
        (Some("e2"), Some(reasons::DEPENDS_ON_SKIPPED), Some("e1"))
    );
    assert_eq!(
        statuses(&entries),
        [
            ("e1", EntryStatus::Excluded),
            ("e2", EntryStatus::DependsOnSkipped),
            ("e3", EntryStatus::WillMerge),
        ]
    );
    assert_eq!(entries[1].blame.as_deref(), Some("e1"));
    assert_eq!(log.state("main"), before);
    let (skipped, _) = plan(&log, "br1", "main", limits(), true);
    let skipped = skipped.unwrap();
    assert_eq!(skipped.merged, ["e3"]);
    assert_eq!(skipped.not_included, ["e1"]);

    // An undo whose unit is not included depends on it too.
    let mut log = three_edits();
    log.undo("br1", "alice", "u1").unwrap();
    let include = set(&["u1"]);
    let limits = MergeLimits {
        include: Some(&include),
        source_up_to: None,
    };
    let err = plan(&log, "br1", "main", limits, false).0.unwrap_err();
    let c = &err.details["conflicts"][0];
    assert_eq!(
        (
            c["txId"].as_str(),
            c["reason"].as_str(),
            c["againstTx"].as_str()
        ),
        (Some("u1"), Some(reasons::DEPENDS_ON_SKIPPED), Some("e3"))
    );

    // And an included record that depends on a dropped one.
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    let trim_c3 = trim("c3", Edge::End, secs(-2));
    log.commit("br1", "e1", "alice", vec![trim_c3, ins(28, 2)]);
    log.commit("br1", "e2", "alice", vec![trim("c4", Edge::End, secs(-1))]);
    log.drop_rec("main", "d1", "br1", &["e1"], &[]);
    let include = set(&["e2"]);
    let limits = MergeLimits {
        include: Some(&include),
        source_up_to: None,
    };
    let (err, entries) = plan(&log, "br1", "main", limits, false);
    assert_eq!(
        err.unwrap_err().details["conflicts"][0]["reason"],
        reasons::DEPENDS_ON_SKIPPED
    );
    assert_eq!(
        statuses(&entries),
        [
            ("e1", EntryStatus::Dropped),
            ("e2", EntryStatus::DependsOnSkipped)
        ]
    );
}

#[test]
fn empty_include_merges_nothing() {
    let log = three_edits();
    let none = BTreeSet::new();
    let limits = MergeLimits {
        include: Some(&none),
        source_up_to: None,
    };
    let (p, entries) = plan(&log, "br1", "main", limits, false);
    let p = p.unwrap();
    assert!(p.planned.is_empty() && p.merged.is_empty());
    assert_eq!(p.not_included, ["e1", "e2", "e3"]);
    assert!(entries.iter().all(|e| e.status == EntryStatus::Excluded));
}

#[test]
fn explain_reports_present_records_and_conflicts() {
    // e1 is merged already; e2 is new; e3 trims c3, which main deleted since.
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    log.commit("br1", "e1", "alice", vec![delete("c1")]);
    log.commit("br1", "e2", "alice", vec![ins(0, 1)]);
    log.commit("br1", "e3", "alice", vec![trim("c3", Edge::End, secs(-1))]);
    let include = set(&["e1"]);
    merge(
        &mut log,
        "br1",
        "main",
        MergeLimits {
            include: Some(&include),
            source_up_to: None,
        },
    );
    log.commit("main", "t1", "bob", vec![delete("c3")]);
    let (p, entries) = plan(&log, "br1", "main", MergeLimits::default(), true);
    let p = p.unwrap();
    assert_eq!(
        statuses(&entries),
        [
            ("e1", EntryStatus::AlreadyPresent),
            ("e2", EntryStatus::WillMerge),
            ("e3", EntryStatus::Conflict),
        ]
    );
    assert_eq!(p.merged, ["e2"]);
    assert_eq!(entries[2].reason.as_deref(), Some(reasons::REMOVED));
    assert_eq!(entries[2].blame.as_deref(), Some("t1"));
}

/// One step of a random history: an edit on a branch (an insert, a delete or a trim of the
/// `k`-th clip), or a merge of `br1` into `br2`.
#[derive(Debug, Clone, Copy)]
enum Step {
    Insert(usize),
    Delete(usize, usize),
    Trim(usize, usize),
    MergeIn,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        (0usize..3).prop_map(Step::Insert),
        ((0usize..3), (0usize..4)).prop_map(|(b, k)| Step::Delete(b, k)),
        ((0usize..3), (0usize..4)).prop_map(|(b, k)| Step::Trim(b, k)),
        Just(Step::MergeIn),
    ]
}

const BRANCHES: [&str; 3] = ["main", "br1", "br2"];

/// `main`, `br1` forked at 0, `br2` forked from `br1` after its first record; then `steps`
/// (edits that do not apply are left out). The source txIds made, in order.
fn history(steps: &[Step]) -> MemLog {
    let mut log = MemLog::new(genesis());
    log.fork("br1", "main", 0);
    log.commit("br1", "x0", "alice", vec![ins(0, 1)]);
    log.fork("br2", "br1", 1);
    for (n, s) in steps.iter().enumerate() {
        let id = format!("x{}", n + 1);
        let clip_of = |log: &MemLog, b: &str, k: usize| {
            let ids = clip_ids(&log.state(b));
            (!ids.is_empty()).then(|| ids[k % ids.len()].clone())
        };
        match *s {
            Step::Insert(b) => {
                let _ = log.try_commit(BRANCHES[b], &id, "alice", vec![ins(0, 1)]);
            }
            Step::Delete(b, k) => {
                if let Some(c) = clip_of(&log, BRANCHES[b], k) {
                    let _ = log.try_commit(BRANCHES[b], &id, "alice", vec![delete(&c)]);
                }
            }
            Step::Trim(b, k) => {
                if let Some(c) = clip_of(&log, BRANCHES[b], k) {
                    let t = trim(&c, Edge::End, secs(-1));
                    let _ = log.try_commit(BRANCHES[b], &id, "alice", vec![t]);
                }
            }
            Step::MergeIn => {
                let _ = log.merge("br1", "br2", "alice", &format!("mi{n}"), true);
            }
        }
    }
    log
}

proptest! {
    #![proptest_config(prop_config(64))]

    /// No merge copies an origin outside `include` or a source-branch record above
    /// `sourceUpTo`; every candidate is merged, not included, excluded or a conflict; and the
    /// explain entries agree with the plan.
    #[test]
    fn prop_include_bound(
        steps in prop::collection::vec(step(), 0..10),
        pick in prop::collection::vec(any::<bool>(), 12),
        up_to in prop::option::of(0u64..12),
        limited in any::<bool>(),
    ) {
        let log = history(&steps);
        let path = log.records("br2", 1, log.head_seq("br2")).unwrap();
        let mut all: Vec<String> = log.records("br1", 0, 1).unwrap().iter()
            .chain(&path)
            .map(|r| r.record.tx_id.clone())
            .collect();
        all.extend(log.records("br1", 1, log.head_seq("br1")).unwrap().iter().map(|r| r.record.tx_id.clone()));
        let include: BTreeSet<String> = all
            .iter()
            .zip(pick.iter().cycle())
            .filter(|(_, p)| **p)
            .map(|(t, _)| t.clone())
            .collect();
        let limits = MergeLimits {
            include: limited.then_some(&include),
            source_up_to: up_to,
        };
        let (p, entries) = plan(&log, "br2", "main", limits, true);
        let p = p.unwrap();
        for e in &entries {
            if e.status == EntryStatus::WillMerge {
                prop_assert!(!limited || include.contains(&e.origin), "{e:?}");
                if let Some(n) = up_to {
                    prop_assert!(e.branch != "br2" || e.seq <= n, "{e:?}");
                }
            }
        }
        let will: Vec<&str> = entries
            .iter()
            .filter(|e| e.status == EntryStatus::WillMerge)
            .map(|e| e.tx_id.as_str())
            .collect();
        prop_assert_eq!(&will, &p.merged.iter().map(String::as_str).collect::<Vec<_>>());
        let not_included: Vec<&str> = entries
            .iter()
            .filter(|e| e.reason.as_deref() == Some(reasons::NOT_INCLUDED))
            .map(|e| e.origin.as_str())
            .collect();
        let mut seen = BTreeSet::new();
        let not_included: Vec<&str> = not_included.into_iter().filter(|o| seen.insert(*o)).collect();
        prop_assert_eq!(not_included, p.not_included.iter().map(String::as_str).collect::<Vec<_>>());
        let conflicts = entries
            .iter()
            .filter(|e| matches!(e.status, EntryStatus::Conflict | EntryStatus::DependsOnSkipped))
            .count();
        prop_assert_eq!(conflicts, p.conflicts.len());
        // Oldest first, along the source path.
        let order: Vec<(u64, &str)> = entries.iter().map(|e| (e.seq, e.branch.as_str())).collect();
        let mut sorted = order.clone();
        sorted.sort_by_key(|(seq, _)| *seq);
        prop_assert_eq!(order, sorted);
    }
}
