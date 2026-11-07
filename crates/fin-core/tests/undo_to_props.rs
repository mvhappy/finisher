//! `undo --to` is the undos in a row (plan 4B Task 15 review): after any history of edits by
//! two authors, undos, redos and checkpoints, one `undo --to` record names the units that the
//! same number of plain undos take, leaves the same state, is reversed by one redo as they
//! are by as many redos, and a stale edit based anywhere in the history (between two of its
//! units too) lands as it does over the plain undos.

mod common;

use common::memlog::MemLog;
use common::*;
use fin_core::history::LogView;
use fin_core::{
    Edge, InsertMode, ItemBody, ItemInsert, ItemSplit, ItemTrim, MemIds, Op, Project, Rational,
    TrimMode, apply_tx, compose,
};
use proptest::prelude::*;

/// `PROPTEST_CASES` overrides the per-block default.
fn config(default: u32) -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    ProptestConfig {
        cases,
        ..ProptestConfig::default()
    }
}

/// `c1`..`c4`, 10 s each, on `t1`, asset `a1`.
fn genesis() -> Project {
    with_clips(&[10, 10, 10, 10])
}

#[derive(Debug, Clone, Copy)]
enum Act {
    /// Author (0 alice, 1 bob), kind (0 delete, 1 insert, 2 trim), which clip or edge.
    Edit(u8, u8, usize),
    Undo,
    Redo,
    Checkpoint,
}

fn act() -> impl Strategy<Value = Act> {
    acts_of(3)
}

/// Edits of kinds `0..kinds` ([`ops_for`]).
fn acts_of(kinds: u8) -> impl Strategy<Value = Act> {
    prop_oneof![
        3 => (0u8..2, 0u8..kinds, 0usize..6).prop_map(|(a, k, i)| Act::Edit(a, k, i)),
        2 => Just(Act::Undo),
        1 => Just(Act::Redo),
        1 => Just(Act::Checkpoint),
    ]
}

fn op_for(p: &Project, kind: u8, k: usize) -> Option<Op> {
    let items = &p.timelines[0].tracks[0].items;
    match kind {
        0 => items.get(k % items.len().max(1)).map(|c| delete(&c.id)),
        1 => {
            let at = items.get(k % (items.len() + 1)).map_or_else(
                || {
                    items
                        .last()
                        .map_or(Rational::zero(), |c| (c.start + c.duration).unwrap())
                },
                |c| c.start,
            );
            Some(insert("$x", "t1", at, "a1", Rational::zero(), secs(3)))
        }
        _ => items
            .get(k % items.len().max(1))
            .filter(|c| c.duration > secs(2))
            .map(|c| {
                trim(
                    &c.id,
                    [Edge::End, Edge::Start][k % 2],
                    [secs(-1), secs(1)][k % 2],
                )
            }),
    }
}

/// [`op_for`], plus edits whose inverses hold several ops (plan 4C Task 8 review B1): kind 3
/// an overwrite inside an item (`compose::overwrite`: a split, then the insert), 4 a split,
/// 5 an overwrite trim extending an end over the next item, 6 a roll of a cut. `None` when it
/// would not apply.
fn ops_for(p: &Project, kind: u8, k: usize) -> Option<Vec<Op>> {
    let items = &p.timelines[0].tracks[0].items;
    let it = items.get(k % items.len().max(1));
    let ops = match kind {
        0..=2 => vec![op_for(p, kind, k)?],
        3 => {
            let it = it?;
            let body = ItemBody::Clip {
                asset: "a1".into(),
                source_in: Rational::zero(),
                speed: secs(1),
            };
            let at = (it.start + secs(1)).ok()?;
            compose::overwrite(p, "t1", at, secs(2), body, "$x")
                .ok()?
                .ops
        }
        4 => {
            let it = it?;
            vec![
                ItemSplit {
                    item: it.id.clone(),
                    offset: it.duration.checked_div(secs(2)).ok()?,
                    id: "$r".into(),
                }
                .into(),
            ]
        }
        5 | 6 => vec![
            ItemTrim {
                item: it?.id.clone(),
                edge: Edge::End,
                delta: if kind == 5 { secs(3) } else { secs(-1) },
                mode: if kind == 5 {
                    TrimMode::Overwrite
                } else {
                    TrimMode::Roll
                },
                alone: false,
            }
            .into(),
        ],
        _ => vec![
            ItemInsert {
                id: "$o".into(),
                track: "t1".into(),
                at: it.map_or(Rational::zero(), |c| c.start),
                duration: secs(2),
                mode: InsertMode::Overwrite,
                body: ItemBody::Clip {
                    asset: "a1".into(),
                    source_in: Rational::zero(),
                    speed: secs(1),
                },
            }
            .into(),
        ],
    };
    apply_tx(p, &tx(ops.clone()), &mut MemIds::after(p))
        .is_ok()
        .then_some(ops)
}

/// Runs `acts` (alice undoes and redoes; refusals are skipped); returns the checkpoint seqs,
/// 0 first.
fn build(log: &mut MemLog, acts: &[Act]) -> Vec<u64> {
    let mut cps = vec![0];
    for (n, a) in acts.iter().enumerate() {
        let id = format!("p{n}");
        match *a {
            Act::Edit(au, kind, k) => {
                if let Some(ops) = ops_for(&log.state("main"), kind, k) {
                    let author = if au == 0 { "alice" } else { "bob" };
                    log.commit("main", &id, author, ops);
                }
            }
            Act::Undo => drop(log.undo("main", "alice", &id)),
            Act::Redo => drop(log.redo("main", "alice", &id)),
            Act::Checkpoint => cps.push(log.checkpoint("main", &id, "alice", &format!("v{n}"))),
        }
    }
    cps
}

fn outcome<T>(r: Result<T, fin_core::CoreError>) -> Result<(), String> {
    r.map(|_| ()).map_err(|e| e.code)
}

/// One `undo --to` against the undos in a row it stands for (see the module docs); stale
/// edits of kinds `0..kinds`.
#[allow(clippy::too_many_arguments)]
fn undo_to_vs_undos(
    acts: &[Act],
    cpi: usize,
    redo: bool,
    tail_bob: bool,
    base_pick: u64,
    skind: u8,
    sk: usize,
) -> Result<(), TestCaseError> {
    let (mut one, mut many) = (MemLog::new(genesis()), MemLog::new(genesis()));
    let cps = build(&mut one, acts);
    build(&mut many, acts);
    let cp = cps[cpi % cps.len()];
    let pre = one.head_seq("main");
    let Ok(u) = one.undo_to("main", "alice", cp, "ut") else {
        return Ok(());
    };
    let names = u.record.undoes.clone().unwrap();
    for (i, n) in names.iter().enumerate() {
        let r = many.undo("main", "alice", &format!("ua{i}"));
        prop_assert!(r.is_ok(), "undo {i} in a row failed: {:?}", r.err());
        prop_assert_eq!(r.unwrap().record.undoes.unwrap(), vec![n.clone()]);
    }
    prop_assert_eq!(one.state("main"), many.state("main"), "after undo");
    if redo {
        one.redo("main", "alice", "rr").unwrap();
        for i in 0..names.len() {
            many.redo("main", "alice", &format!("ra{i}")).unwrap();
        }
        prop_assert_eq!(one.state("main"), many.state("main"), "after redo");
    }
    if tail_bob && let Some(op) = op_for(&one.state("main"), 1, sk) {
        one.commit("main", "tb", "bob", vec![op.clone()]);
        many.commit("main", "tb", "bob", vec![op]);
    }
    // A stale edit by carol at any base up to the history before the `undo --to`.
    let base = base_pick % (pre + 1);
    let at_base = LogView::state_at(&one, "main", base).unwrap();
    if let Some(ops) = ops_for(&at_base, skind, sk) {
        let a = outcome(one.commit_at("main", "s", "carol", base, ops.clone()));
        let b = outcome(many.commit_at("main", "s", "carol", base, ops));
        prop_assert_eq!(&a, &b, "stale outcome at base {}", base);
        prop_assert_eq!(
            starts(&one.state("main"), 0),
            starts(&many.state("main"), 0),
            "stale at base {}",
            base
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(config(1000))]

    #[test]
    fn undo_to_matches_undos_in_a_row_for_stale_edits(
        acts in prop::collection::vec(act(), 1..12),
        cpi in 0usize..4,
        redo in any::<bool>(),
        tail_bob in any::<bool>(),
        base_pick in 0u64..64,
        skind in 0u8..3,
        sk in 0usize..6,
    ) {
        undo_to_vs_undos(&acts, cpi, redo, tail_bob, base_pick, skind, sk)?;
    }
}

proptest! {
    #![proptest_config(config(1500))]

    /// Plan 4C Task 8 review B1: the same over edits whose inverses hold several ops
    /// (overwrites, splits, overwrite trims, rolls), so a partial `undo --to` is cut at
    /// inverse op boundaries.
    #[test]
    fn undo_to_matches_undos_in_a_row_over_multi_op_inverses(
        acts in prop::collection::vec(acts_of(8), 1..12),
        cpi in 0usize..4,
        redo in any::<bool>(),
        tail_bob in any::<bool>(),
        base_pick in 0u64..64,
        skind in 0u8..8,
        sk in 0usize..6,
    ) {
        undo_to_vs_undos(&acts, cpi, redo, tail_bob, base_pick, skind, sk)?;
    }
}
