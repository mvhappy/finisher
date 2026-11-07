//! Index placements under rebase (spec §4.3, `rebase::places`), checked by an independent
//! oracle. Three ordered lists (a timeline's tracks, an item's effects, the font fallback
//! order) and six routes: a stale commit, a merge, cherry-picks, an undo, a redo, a revert.
//! Alice's one to three records chain index ops through placeholders (an add at an index,
//! then moves of it and placements next to it); bob edits the list on main in between (adds
//! at an index or the end, removals, moves).
//!
//! **The oracle** never reads the engine's anchors. For each op alice's transaction applies
//! (its ops as written, or the stored inverse an undo, redo or revert takes), in its own frame
//! and at the head as it is right before the op:
//! - a placement of x at index i must keep x's frame order relative to every member of the
//!   frame list that is still at the head and that no other author moved since: x after the
//!   ones before i, before the ones from i on. Counting the head positions that do so, exactly
//!   one is the placement; none or several is a conflict, `SAME_PARAM`;
//! - a move of a member someone removed or moved, a removal of a member someone removed, a
//!   placement of a member that is there already: a conflict of any reason.
//!
//! The engine must give that order, or refuse when the oracle says so.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::memlog::MemLog;
use common::*;
use fin_core::history::{LogRec, LogView};
use fin_core::{
    CoreError, EffectAdd, EffectMove, EffectRemove, FontFormat, FontPin, FontPinOp, FontSource,
    FontUnpin, Op, Project, TrackAdd, TrackKind, TrackMove, TrackRemove,
};
use proptest::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Tracks,
    Effects,
    Fonts,
}

#[derive(Debug, Clone, Copy)]
enum Route {
    Stale,
    Merge,
    Pick,
    Undo,
    Redo,
    Revert,
}

/// An edit as generated (indexes taken modulo what the list allows when it is made).
#[derive(Debug, Clone)]
enum Gen {
    Add(Option<usize>),
    Remove(usize),
    /// Move one of the record's own new members (by creation order).
    MoveNew(usize, usize),
    Move(usize, usize),
}

/// A list op on member names (ids, or placeholders not bound).
#[derive(Debug, Clone)]
enum LOp {
    Append(String),
    Place(String, usize),
    Move(String, usize),
    Remove(String),
}

type Names = BTreeMap<String, String>;

fn key(family: &str) -> String {
    format!("{family}|400|normal")
}

fn face(family: &str) -> FontPin {
    FontPin {
        family: family.into(),
        weight: 400,
        italic: false,
        face: 0,
        sha256: "a".repeat(64),
        size: 1000,
        format: FontFormat::Ttf,
        scripts: vec!["Latn".into()],
        source: FontSource::Catalog {
            entry: family.to_lowercase(),
            style: "regular".into(),
            catalog: "2026.11.1".into(),
        },
    }
}

fn genesis(kind: Kind) -> Project {
    match kind {
        Kind::Effects => with_clips(&[2]),
        _ => project(),
    }
}

fn list(p: &Project, kind: Kind) -> Vec<String> {
    match kind {
        Kind::Tracks => p.timelines[0].tracks.iter().map(|t| t.id.clone()).collect(),
        Kind::Effects => {
            let c1 = p.timelines[0].tracks[0].items.iter().find(|i| i.id == "c1");
            c1.map_or(Vec::new(), |c| {
                c.common.effects.iter().map(|e| e.id.clone()).collect()
            })
        }
        Kind::Fonts => p.fonts.fallback.clone(),
    }
}

/// A new member's name: a placeholder, or (fonts, keyed by family) its key.
fn fresh(kind: Kind, base: &str) -> String {
    match kind {
        Kind::Fonts => key(base),
        _ => format!("${base}"),
    }
}

/// An add of `name` (from [`fresh`]) at `index` (the end when `None`) into a list of `len`.
fn add(kind: Kind, name: &str, index: Option<usize>, len: usize) -> Op {
    let index = index.map(|i| i.min(len) as u32);
    match kind {
        Kind::Tracks => TrackAdd {
            id: name.into(),
            timeline: "tl1".into(),
            kind: TrackKind::Video,
            index,
            name: None,
        }
        .into(),
        Kind::Effects => EffectAdd {
            item: "c1".into(),
            id: name.into(),
            kind: "blur".into(),
            at: index,
            params: BTreeMap::new(),
        }
        .into(),
        Kind::Fonts => FontPinOp {
            key: name.into(),
            pin: face(name.trim_end_matches("|400|normal")),
            fallback: Some(index.unwrap_or(len as u32)),
        }
        .into(),
    }
}

fn remove(kind: Kind, member: &str) -> Op {
    match kind {
        Kind::Tracks => TrackRemove {
            track: member.into(),
            with_items: true,
        }
        .into(),
        Kind::Effects => EffectRemove {
            item: "c1".into(),
            effect: member.into(),
        }
        .into(),
        Kind::Fonts => FontUnpin::new(member).into(),
    }
}

fn mv(kind: Kind, member: &str, to: usize) -> Op {
    match kind {
        Kind::Tracks => TrackMove {
            track: member.into(),
            index: to as u32,
        }
        .into(),
        _ => EffectMove {
            item: "c1".into(),
            effect: member.into(),
            to: to as u32,
        }
        .into(),
    }
}

/// The list op `op` makes, its ids named through `names` (placeholder → id).
fn list_op(op: &Op, names: &Names) -> Option<LOp> {
    let n = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_string());
    let at = |i: u32| i as usize;
    Some(match op {
        Op::TrackAdd(a) => match a.index {
            Some(i) => LOp::Place(n(&a.id), at(i)),
            None => LOp::Append(n(&a.id)),
        },
        Op::TrackRemove(r) => LOp::Remove(n(&r.track)),
        Op::TrackRestore(r) => LOp::Place(n(&r.track.id), at(r.index)),
        Op::TrackMove(m) => LOp::Move(n(&m.track), at(m.index)),
        Op::EffectAdd(a) => match a.at {
            Some(i) => LOp::Place(n(&a.id), at(i)),
            None => LOp::Append(n(&a.id)),
        },
        Op::EffectRemove(r) => LOp::Remove(n(&r.effect)),
        Op::EffectRestore(r) => LOp::Place(n(&r.effect.id), at(r.index)),
        Op::EffectMove(m) => LOp::Move(n(&m.effect), at(m.to)),
        Op::FontPin(p) => LOp::Place(p.key.clone(), at(p.fallback?)),
        Op::FontUnpin(u) if !u.fallback_only => LOp::Remove(u.key.clone()),
        _ => return None,
    })
}

fn lops(ops: &[Op], names: &Names) -> Vec<LOp> {
    ops.iter().filter_map(|o| list_op(o, names)).collect()
}

/// What the oracle expects.
#[derive(Debug)]
enum Want {
    Order(Vec<String>),
    /// A conflict; `same_param`: of that reason.
    Conflict {
        same_param: bool,
    },
}

/// The oracle (module docs): `ops` from `frame`, at `head`; `moved`: what other authors moved.
fn oracle(frame: &[String], head: &[String], ops: &[LOp], moved: &BTreeSet<String>) -> Want {
    let (mut f, mut h) = (frame.to_vec(), head.to_vec());
    let any = Want::Conflict { same_param: false };
    for op in ops {
        let (m, i) = match op {
            LOp::Append(m) => {
                f.push(m.clone());
                h.push(m.clone());
                continue;
            }
            LOp::Remove(m) => {
                if !f.contains(m) {
                    continue;
                }
                if !h.contains(m) {
                    return any;
                }
                f.retain(|x| x != m);
                h.retain(|x| x != m);
                continue;
            }
            LOp::Move(m, _) if !h.contains(m) || moved.contains(m) => return any,
            LOp::Place(m, _) if h.contains(m) => return any,
            LOp::Move(m, i) | LOp::Place(m, i) => (m.clone(), *i),
        };
        let fr: Vec<String> = f.iter().filter(|x| **x != m).cloned().collect();
        let i = i.min(fr.len());
        let hr: Vec<String> = h.iter().filter(|x| **x != m).cloned().collect();
        // Every surviving, unmoved frame member: whether it comes before x, its head position.
        let keep: Vec<(bool, usize)> = fr
            .iter()
            .enumerate()
            .filter(|(_, s)| !moved.contains(*s))
            .filter_map(|(k, s)| hr.iter().position(|y| y == s).map(|p| (k < i, p)))
            .collect();
        let fits: Vec<usize> = (0..=hr.len())
            .filter(|&p| {
                keep.iter()
                    .all(|&(before, q)| if before { q < p } else { q >= p })
            })
            .collect();
        let [p] = fits[..] else {
            return Want::Conflict { same_param: true };
        };
        f = fr;
        f.insert(i, m.clone());
        h = hr;
        h.insert(p, m);
    }
    Want::Order(h)
}

/// Alice's record from `gens`, made against `now` (the list as she sees it); `n` names its new
/// members.
fn alice_ops(kind: Kind, now: &[String], gens: &[Gen], n: usize) -> Vec<Op> {
    let mut l = now.to_vec();
    let mut ops = Vec::new();
    let mut new: Vec<String> = Vec::new();
    for (j, g) in gens.iter().enumerate() {
        match g {
            Gen::Add(i) => {
                let name = fresh(kind, &format!("A{n}x{j}"));
                ops.push(add(kind, &name, *i, l.len()));
                match (kind, i) {
                    (Kind::Fonts, _) | (_, Some(_)) => {
                        let at = i.unwrap_or(l.len()).min(l.len());
                        l.insert(at, name.clone());
                    }
                    _ => l.push(name.clone()),
                }
                new.push(name);
            }
            Gen::Remove(k) => {
                if l.len() < 2 {
                    continue;
                }
                let m = l[k % l.len()].clone();
                if new.contains(&m) {
                    continue;
                }
                l.retain(|x| *x != m);
                ops.push(remove(kind, &m));
            }
            Gen::MoveNew(..) | Gen::Move(..) if kind == Kind::Fonts || l.is_empty() => {}
            Gen::MoveNew(k, to) | Gen::Move(k, to) => {
                let m = match g {
                    Gen::MoveNew(..) if new.is_empty() => continue,
                    Gen::MoveNew(..) => new[k % new.len()].clone(),
                    _ => l[k % l.len()].clone(),
                };
                let to = to % l.len();
                l.retain(|x| *x != m);
                l.insert(to, m.clone());
                ops.push(mv(kind, &m, to));
            }
        }
    }
    ops
}

/// Bob's edits on main, one record each: the members he moved.
fn bob(log: &mut MemLog, kind: Kind, gens: &[Gen]) -> BTreeSet<String> {
    let mut moved = BTreeSet::new();
    for (n, g) in gens.iter().enumerate() {
        let now = list(&log.state("main"), kind);
        let op = match g {
            Gen::Add(i) => add(kind, &fresh(kind, &format!("B{n}")), *i, now.len()),
            Gen::Remove(_) if now.len() < 2 => continue,
            Gen::Remove(k) => remove(kind, &now[k % now.len()]),
            Gen::MoveNew(..) | Gen::Move(..) if kind == Kind::Fonts || now.is_empty() => continue,
            Gen::MoveNew(k, to) | Gen::Move(k, to) => {
                let m = now[k % now.len()].clone();
                moved.insert(m.clone());
                mv(kind, &m, to % now.len())
            }
        };
        log.commit("main", &format!("b{n}"), "bob", vec![op]);
    }
    moved
}

fn check(
    what: &str,
    want: Want,
    got: Result<(), CoreError>,
    now: Vec<String>,
) -> Result<(), TestCaseError> {
    match (want, got) {
        (Want::Order(want), Ok(())) => prop_assert_eq!(now, want, "{}", what),
        (Want::Order(want), Err(e)) => prop_assert!(false, "{what}: want {want:?}, got {e:?}"),
        (Want::Conflict { same_param }, Err(e)) => {
            let reason = e.details["reason"].as_str().unwrap_or(e.code.as_str());
            prop_assert!(
                !same_param || reason == "SAME_PARAM",
                "{what}: {reason}: {e:?}"
            );
        }
        (Want::Conflict { .. }, Ok(())) => {
            prop_assert!(false, "{what}: want a conflict, got {now:?}")
        }
    }
    Ok(())
}

fn run(
    kind: Kind,
    extra: usize,
    recs: &[Vec<Gen>],
    bobs: &[Gen],
    route: Route,
) -> Result<(), TestCaseError> {
    let mut log = MemLog::new(genesis(kind));
    let setup: Vec<Op> = (0..=extra)
        .map(|i| add(kind, &fresh(kind, &format!("S{i}")), None, i))
        .collect();
    log.commit("main", "s0", "setup", setup);
    let base_seq = log.head_seq("main");
    let base = list(&log.state("main"), kind);
    let what = format!("{kind:?} {route:?} base {base:?} bob {bobs:?}");
    let now = |log: &MemLog| list(&log.state("main"), kind);
    match route {
        Route::Stale => {
            let ops = alice_ops(kind, &base, &recs.concat(), 0);
            if ops.is_empty() {
                return Ok(());
            }
            let moved = bob(&mut log, kind, bobs);
            let head = now(&log);
            let got = log.commit_at("main", "a0", "alice", base_seq, ops.clone());
            let names = got
                .as_ref()
                .map(|r| r.record.bindings.clone())
                .unwrap_or_default();
            let want = oracle(&base, &head, &lops(&ops, &names), &moved);
            let what = format!("{what} head {head:?} ops {ops:?}");
            check(&what, want, got.map(|_| ()), now(&log))
        }
        Route::Merge | Route::Pick | Route::Revert => {
            log.fork("br1", "main", base_seq);
            let mut per: Vec<(String, Vec<LOp>)> = Vec::new();
            for (n, gens) in recs.iter().enumerate() {
                let ops = alice_ops(kind, &list(&log.state("br1"), kind), gens, n);
                if !ops.is_empty() {
                    let rec = log.commit("br1", &format!("a{n}"), "alice", ops);
                    per.push((
                        rec.record.tx_id.clone(),
                        lops(&rec.record.resolved, &Names::new()),
                    ));
                }
            }
            if per.is_empty() {
                return Ok(());
            }
            if let Route::Revert = route {
                log.merge("br1", "main", "carol", "g1", false).unwrap();
                let frame = now(&log);
                let group: Vec<LogRec> = log
                    .records("main", base_seq, log.head_seq("main"))
                    .unwrap()
                    .into_iter()
                    .filter(|r| r.record.merge_id.as_deref() == Some("g1"))
                    .collect();
                let moved = bob(&mut log, kind, bobs);
                let head = now(&log);
                let inv: Vec<LOp> = group
                    .iter()
                    .rev()
                    .flat_map(|r| lops(&r.record.inverse, &Names::new()))
                    .collect();
                let got = log.revert("g1", "main", "carol", "v1").map(|_| ());
                let what = format!("{what} frame {frame:?} head {head:?} inverse {inv:?}");
                let want = oracle(&frame, &head, &inv, &moved);
                return check(&what, want, got, now(&log));
            }
            let moved = bob(&mut log, kind, bobs);
            let head = now(&log);
            let what = format!("{what} head {head:?} alice {per:?}");
            if let Route::Merge = route {
                let all: Vec<LOp> = per.iter().flat_map(|(_, l)| l.iter().cloned()).collect();
                let want = oracle(&base, &head, &all, &moved);
                let got = log.merge("br1", "main", "carol", "g1", false).map(|_| ());
                return check(&what, want, got, now(&log));
            }
            // Picks in order, until one is refused.
            let mut done: Vec<LOp> = Vec::new();
            for (tx, l) in &per {
                let mut all = done.clone();
                all.extend(l.iter().cloned());
                let want = oracle(&base, &head, &all, &moved);
                let got = log
                    .pick(tx, "main", "carol", &format!("p-{tx}"))
                    .map(|_| ());
                let refused = got.is_err();
                check(&what, want, got, now(&log))?;
                if refused {
                    break;
                }
                done = all;
            }
            Ok(())
        }
        Route::Undo | Route::Redo => {
            let mut last = None;
            for (n, gens) in recs.iter().enumerate() {
                let ops = alice_ops(kind, &now(&log), gens, n);
                if !ops.is_empty() {
                    last = Some(log.commit("main", &format!("a{n}"), "alice", ops));
                }
            }
            let Some(mut unit) = last else {
                return Ok(());
            };
            if let Route::Redo = route {
                unit = log.undo("main", "alice", "u0").unwrap();
            }
            let frame = now(&log);
            let moved = bob(&mut log, kind, bobs);
            let head = now(&log);
            let inv = lops(&unit.record.inverse, &Names::new());
            let got = match route {
                Route::Undo => log.undo("main", "alice", "u1").map(|_| ()),
                _ => log.redo("main", "alice", "r1").map(|_| ()),
            };
            let what = format!("{what} frame {frame:?} head {head:?} inverse {inv:?}");
            check(&what, oracle(&frame, &head, &inv, &moved), got, now(&log))
        }
    }
}

fn gen_op() -> impl Strategy<Value = Gen> {
    prop_oneof![
        3 => proptest::option::of(0usize..7).prop_map(Gen::Add),
        2 => (0usize..7).prop_map(Gen::Remove),
        2 => (0usize..4, 0usize..7).prop_map(|(m, to)| Gen::MoveNew(m, to)),
        2 => (0usize..7, 0usize..7).prop_map(|(m, to)| Gen::Move(m, to)),
    ]
}

proptest! {
    #![proptest_config(prop_config(256))]
    #[test]
    fn index_ops_keep_their_order_with_unmoved_neighbours(
        kind in prop_oneof![Just(Kind::Tracks), Just(Kind::Effects), Just(Kind::Fonts)],
        extra in 0usize..4,
        recs in proptest::collection::vec(proptest::collection::vec(gen_op(), 1..4), 1..4),
        bobs in proptest::collection::vec(gen_op(), 0..4),
        route in prop_oneof![
            Just(Route::Stale),
            Just(Route::Merge),
            Just(Route::Pick),
            Just(Route::Undo),
            Just(Route::Redo),
            Just(Route::Revert),
        ],
    ) {
        run(kind, extra, &recs, &bobs, route)?;
    }
}
