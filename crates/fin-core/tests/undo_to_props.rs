//! `undo --to` is the undos in a row (plan 4B Task 15 review): after any history of edits by
//! two authors, undos, redos and checkpoints, one `undo --to` record names the units that the
//! same number of plain undos take, leaves the same state, is reversed by one redo as they
//! are by as many redos, and a stale edit based anywhere in the history (between two of its
//! units too) lands as it does over the plain undos.

mod common;

use common::gen4c::{G, Shape, World, base4c, g_edit, shape};
use common::memlog::MemLog;
use common::*;
use fin_core::history::LogView;
use fin_core::{
    Curve, Edge, InsertMode, ItemBody, ItemInsert, ItemMove, ItemSplit, ItemTrim, MemIds, Op,
    Project, Rational, TransitionKind, TransitionRemove, TransitionSet, TrimMode, apply_tx,
    compose,
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

fn dissolve(id: &str, a: &str, b: &str) -> Op {
    TransitionSet {
        id: id.into(),
        track: "t1".into(),
        between: [a.into(), b.into()],
        kind: TransitionKind::Dissolve,
        duration: Rational::new(1, 2).unwrap(),
        curve: Curve::Linear,
    }
    .into()
}

/// [`genesis`] with source before every clip and a dissolve at each cut (plan 4C Task 10).
fn genesis_with_transitions() -> Project {
    let p = with_sourced(&[(10, 5), (10, 15), (10, 25), (10, 35)]);
    let cuts = (1..4)
        .map(|i| dissolve(&format!("$t{i}"), &format!("c{i}"), &format!("c{}", i + 1)))
        .collect();
    apply(&p, cuts).project
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
        // Plan 4C Task 10: a transition at the cut after the item, the removal of one, and
        // an overwrite move by a second (its inverse carries the transition at a cut it keeps).
        8 => {
            let it = it?;
            let end = (it.start + it.duration).ok()?;
            let next = items.iter().find(|c| c.start == end)?;
            vec![dissolve("$tr", &it.id, &next.id)]
        }
        9 => {
            let ts = &p.timelines[0].tracks[0].transitions;
            vec![
                TransitionRemove {
                    transition: ts.get(k % ts.len().max(1))?.id.clone(),
                }
                .into(),
            ]
        }
        10 => {
            let it = it?;
            vec![
                ItemMove {
                    item: it.id.clone(),
                    track: "t1".into(),
                    at: (it.start + secs([1, -1][k % 2])).ok()?,
                    mode: InsertMode::Overwrite,
                }
                .into(),
            ]
        }
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

/// One `undo --to` against the undos in a row it stands for (see the module docs), from
/// `genesis`; stale edits of kinds `0..kinds`.
#[allow(clippy::too_many_arguments)]
fn undo_to_vs_undos(
    genesis: fn() -> Project,
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
        undo_to_vs_undos(genesis, &acts, cpi, redo, tail_bob, base_pick, skind, sk)?;
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
        undo_to_vs_undos(genesis, &acts, cpi, redo, tail_bob, base_pick, skind, sk)?;
    }
}

proptest! {
    #![proptest_config(config(1000))]

    /// Plan 4C Task 10: the same with transitions at the cuts, which the other edits prune
    /// (their restores follow each op's inverse) or carry (removed before an inverse of several
    /// ops, restored after it), so inverse op counts include them.
    #[test]
    fn undo_to_matches_undos_in_a_row_with_transitions(
        acts in prop::collection::vec(acts_of(11), 1..12),
        cpi in 0usize..4,
        redo in any::<bool>(),
        tail_bob in any::<bool>(),
        base_pick in 0u64..64,
        skind in 0u8..11,
        sk in 0usize..6,
    ) {
        undo_to_vs_undos(
            genesis_with_transitions, &acts, cpi, redo, tail_bob, base_pick, skind, sk,
        )?;
    }
}

// ---------------------------------------------------------------------------------------------
// Plan 4C Task 26: the same over every 4C edit.

#[derive(Debug, Clone)]
enum Act4 {
    /// An edit by alice (`false`) or bob: abstract 4C ops realised at the head.
    Edit(bool, Vec<G>),
    Undo,
    Redo,
    Checkpoint,
}

fn act4() -> impl Strategy<Value = Act4> {
    prop_oneof![
        4 => (any::<bool>(), prop::collection::vec(g_edit(), 1..4))
            .prop_map(|(bob, gs)| Act4::Edit(bob, gs)),
        2 => Just(Act4::Undo),
        1 => Just(Act4::Redo),
        1 => Just(Act4::Checkpoint),
    ]
}

/// The ops `gs` realise at `p` (client ops only), `None` when none applies.
fn realise4(p: &Project, gs: &[G]) -> Option<Vec<Op>> {
    let mut w = World::new(p, MemIds::after(p), false);
    w.run(gs);
    (!w.ops.is_empty()).then_some(w.ops)
}

fn build4(log: &mut MemLog, acts: &[Act4]) -> Vec<u64> {
    let mut cps = vec![0];
    for (n, a) in acts.iter().enumerate() {
        let id = format!("p{n}");
        match a {
            Act4::Edit(bob, gs) => {
                if let Some(ops) = realise4(&log.state("main"), gs) {
                    let author = if *bob { "bob" } else { "alice" };
                    log.commit("main", &id, author, ops);
                }
            }
            Act4::Undo => drop(log.undo("main", "alice", &id)),
            Act4::Redo => drop(log.redo("main", "alice", &id)),
            Act4::Checkpoint => cps.push(log.checkpoint("main", &id, "alice", &format!("v{n}"))),
        }
    }
    cps
}

/// One `undo --to` against the undos in a row over 4C edits from [`base4c`] (see the module
/// docs), then a stale 4C edit at `base_pick`.
fn undo_to_vs_undos_4c(
    shape: &Shape,
    acts: &[Act4],
    cpi: usize,
    redo: bool,
    base_pick: u64,
    stale: &[G],
) -> Result<(), TestCaseError> {
    let genesis = base4c(shape);
    let (mut one, mut many) = (MemLog::new(genesis.clone()), MemLog::new(genesis));
    let cps = build4(&mut one, acts);
    build4(&mut many, acts);
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
    let base = base_pick % (pre + 1);
    let at_base = LogView::state_at(&one, "main", base).unwrap();
    if let Some(ops) = realise4(&at_base, stale) {
        let a = outcome(one.commit_at("main", "s", "carol", base, ops.clone()));
        let b = outcome(many.commit_at("main", "s", "carol", base, ops));
        prop_assert_eq!(&a, &b, "stale outcome at base {}", base);
        prop_assert_eq!(
            one.state("main"),
            many.state("main"),
            "stale at base {}",
            base
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(config(256))]

    /// Every 4C edit (transactions of one to three, so inverses of several ops and of composed
    /// edits): one `undo --to` equals the undos in a row, one redo equals as many redos, and a
    /// stale 4C edit at any base lands as it does over the plain undos.
    #[test]
    fn undo_to_matches_undos_in_a_row_for_every_4c_edit(
        shape in shape(),
        acts in prop::collection::vec(act4(), 1..10),
        cpi in 0usize..4,
        redo in any::<bool>(),
        base_pick in 0u64..64,
        stale in prop::collection::vec(g_edit(), 1..3),
    ) {
        undo_to_vs_undos_4c(&shape, &acts, cpi, redo, base_pick, &stale)?;
    }
}

/// The shrunk case of an intermittent failure: alice re-pins the font asset's face as it is
/// pinned (a no-op: its inverse holds no ops), then slides the caption; carol's stale
/// overwrite move of the caption is based between the two. The `undo --to` names both units
/// with only the slide in the interval, so its tail part (the no-op's inverse) is empty; it
/// must still be cut, its first part mirroring the slide as the plain undo of it does, or the
/// move meets the slide as `SAME_PARAM`.
#[test]
fn undo_to_with_a_no_op_unit_before_the_stale_base_mirrors_the_rest() {
    let shape = Shape {
        lens: vec![vec![], vec![], vec![]],
        linked: false,
        transitions: false,
        captions: 1,
        effects: false,
        pinned: true,
    };
    let acts = [
        Act4::Edit(
            false,
            vec![G::FontPin {
                which: 3,
                fallback: None,
            }],
        ),
        Act4::Edit(false, vec![G::Slide { t: 2, k: 0, n: 1 }]),
    ];
    let stale = [G::Move {
        t: 2,
        k: 0,
        to: 2,
        at: None,
        off: -6,
        overwrite: true,
    }];
    undo_to_vs_undos_4c(&shape, &acts, 0, false, 55, &stale).unwrap();
}
