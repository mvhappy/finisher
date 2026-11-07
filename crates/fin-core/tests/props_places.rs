//! Index placements carried by their neighbours (spec §4.3, `rebase::places`), under a stale
//! rebase and a merge, with index ops chained through placeholders in one transaction: a
//! `track.add $x` at an index, then ops that name `$x` as the member they move or as a
//! neighbour of a later placement. Bob commits list edits on main (adds at an index or at the
//! end, removals, moves); alice's transaction, written against the base, is committed stale or
//! made on a side branch and merged. An independent list model on names (base ids, bob's ids,
//! alice's placeholders) applies the rule: each placement goes between the neighbours its
//! index names in alice's own frame, next to the survivor when one is gone, at its end when an
//! end place's neighbour is gone; another member between them, both gone, or a non-empty list
//! where an empty one was is a conflict. The real planner must agree: the same order, or a
//! `SAME_PARAM` refusal (`REMOVED`/`NOT_FOUND` when alice moves a track bob removed).

mod common;

use common::memlog::MemLog;
use common::*;
use fin_core::{Op, TrackAdd, TrackKind, TrackMove, TrackRemove};
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Bob {
    Add(Option<usize>),
    Remove(usize),
    Move(usize, usize),
}

#[derive(Debug, Clone)]
enum Alice {
    Add(Option<usize>),
    /// Move one of alice's own new tracks (by its order of creation), or a base track.
    MoveNew(usize, usize),
    MoveOld(usize, usize),
}

fn add(id: &str, index: Option<usize>) -> Op {
    TrackAdd {
        id: id.into(),
        timeline: "tl1".into(),
        kind: TrackKind::Video,
        index: index.map(|i| i as u32),
        name: None,
    }
    .into()
}

fn mv(track: &str, index: usize) -> Op {
    TrackMove {
        track: track.into(),
        index: index as u32,
    }
    .into()
}

fn names(log: &MemLog, b: &str, rename: &[(String, String)]) -> Vec<String> {
    let p = log.state(b);
    p.timelines[0]
        .tracks
        .iter()
        .map(|t| {
            rename
                .iter()
                .find(|(_, id)| *id == t.id)
                .map_or_else(|| t.id.clone(), |(n, _)| n.clone())
        })
        .collect()
}

/// The rule (module docs) on names: the index in `head` (without the member), or `None`.
fn resolve(head: &[String], before: Option<&String>, after: Option<&String>) -> Option<usize> {
    let at = |m: Option<&String>| m.and_then(|m| head.iter().position(|x| x == m));
    match (before, after) {
        (None, None) => head.is_empty().then_some(0),
        (Some(_), None) => match at(before) {
            Some(i) if i + 1 == head.len() => Some(head.len()),
            None => Some(head.len()),
            Some(_) => None,
        },
        (None, Some(_)) => match at(after) {
            Some(0) | None => Some(0),
            Some(_) => None,
        },
        (Some(_), Some(_)) => match (at(before), at(after)) {
            (Some(b), Some(a)) if a == b + 1 => Some(a),
            (Some(b), None) => Some(b + 1),
            (None, Some(a)) => Some(a),
            _ => None,
        },
    }
}

/// What the model expects: the final order on names, or a conflict (`true`: a refusal of any
/// reason is right, alice's moved track being gone).
enum Want {
    Order(Vec<String>),
    Conflict { gone: bool },
}

/// `ops`: alice's ops as made, each with its name and (a move) the member it moves; `moved`:
/// the tracks bob moved (two moves of one track meet on its `index`: `SAME_PARAM`).
fn model(
    base: &[String],
    head: &[String],
    ops: &[(String, Alice, String)],
    bob_moved: &[String],
) -> Want {
    let (mut f, mut h) = (base.to_vec(), head.to_vec());
    for (name, op, moved) in ops {
        let (member, index) = match op {
            Alice::Add(None) => {
                f.push(name.clone());
                h.push(name.clone());
                continue;
            }
            Alice::Add(Some(i)) => (name.clone(), *i),
            Alice::MoveNew(_, i) | Alice::MoveOld(_, i) => (moved.clone(), *i),
        };
        let rest: Vec<String> = f.iter().filter(|x| **x != member).cloned().collect();
        let i = index.min(rest.len());
        let hrest: Vec<String> = h.iter().filter(|x| **x != member).cloned().collect();
        if matches!(op, Alice::MoveOld(..)) && !h.contains(&member) {
            return Want::Conflict { gone: true };
        }
        if matches!(op, Alice::MoveOld(..)) && bob_moved.contains(&member) {
            return Want::Conflict { gone: false };
        }
        let Some(j) = resolve(&hrest, i.checked_sub(1).map(|k| &rest[k]), rest.get(i)) else {
            return Want::Conflict { gone: false };
        };
        f = rest;
        f.insert(i, member.clone());
        h = hrest;
        h.insert(j, member);
    }
    Want::Order(h)
}

/// The member a move names: one of alice's new tracks made so far, or a base track.
fn ops_member(op: &Alice, m: usize, base: &[String], made: &[(String, Alice, String)]) -> String {
    let new: Vec<&String> = made
        .iter()
        .filter(|(_, o, _)| matches!(o, Alice::Add(_)))
        .map(|(n, _, _)| n)
        .collect();
    match op {
        Alice::MoveNew(..) => new[m % new.len()].clone(),
        _ => base[m % base.len()].clone(),
    }
}

fn bob_op() -> impl Strategy<Value = Bob> {
    prop_oneof![
        proptest::option::of(0usize..6).prop_map(Bob::Add),
        (0usize..6).prop_map(Bob::Remove),
        (0usize..6, 0usize..6).prop_map(|(k, to)| Bob::Move(k, to)),
    ]
}

fn alice_op() -> impl Strategy<Value = Alice> {
    prop_oneof![
        3 => proptest::option::of(0usize..7).prop_map(Alice::Add),
        2 => (0usize..4, 0usize..7).prop_map(|(m, to)| Alice::MoveNew(m, to)),
        1 => (0usize..6, 0usize..7).prop_map(|(m, to)| Alice::MoveOld(m, to)),
    ]
}

/// Runs one case; `merge`: alice's transaction made on `br1` and merged, else committed stale.
fn run(extra: usize, bob: &[Bob], alice: &[Alice], merge: bool) -> Result<(), TestCaseError> {
    let mut log = MemLog::new(project());
    let setup: Vec<Op> = (0..extra).map(|i| add(&format!("$s{i}"), None)).collect();
    if !setup.is_empty() {
        log.commit("main", "s0", "setup", setup);
    }
    let base_seq = log.head_seq("main");
    let base = names(&log, "main", &[]);
    if merge {
        log.fork("br1", "main", base_seq);
    }
    // Bob's edits, one record each, resolved against the state they meet.
    let mut moved = Vec::new();
    for (n, op) in bob.iter().enumerate() {
        let now = names(&log, "main", &[]);
        let op = match op {
            Bob::Add(i) => add(&format!("$b{n}"), i.map(|i| i.min(now.len()))),
            Bob::Remove(_) if now.len() < 2 => continue,
            Bob::Remove(k) => TrackRemove {
                track: now[k % now.len()].clone(),
                with_items: false,
            }
            .into(),
            Bob::Move(k, to) => {
                moved.push(now[k % now.len()].clone());
                mv(&now[k % now.len()], to % now.len())
            }
        };
        log.commit("main", &format!("b{n}"), "bob", vec![op]);
    }
    let head = names(&log, "main", &[]);
    // Alice's transaction, against the base, naming her new tracks by placeholder.
    let mut ops = Vec::new();
    let mut named: Vec<(String, Alice, String)> = Vec::new();
    let mut frame = base.clone();
    for (n, op) in alice.iter().enumerate() {
        let name = format!("$x{n}");
        let real = match op {
            Alice::Add(i) => {
                let i = i.map(|i| i.min(frame.len()));
                let o = add(&name, i);
                match i {
                    Some(i) => frame.insert(i, name.clone()),
                    None => frame.push(name.clone()),
                }
                named.push((name.clone(), Alice::Add(i), String::new()));
                o
            }
            Alice::MoveNew(m, to) | Alice::MoveOld(m, to) => {
                let new = named.iter().any(|(_, o, _)| matches!(o, Alice::Add(_)));
                if matches!(op, Alice::MoveNew(..)) && !new {
                    continue;
                }
                let member = ops_member(op, *m, &base, &named);
                let to = to % frame.len();
                frame.retain(|x| *x != member);
                frame.insert(to, member.clone());
                let kept = match op {
                    Alice::MoveNew(..) => Alice::MoveNew(*m, to),
                    _ => Alice::MoveOld(*m, to),
                };
                let o = mv(&member, to);
                named.push((name.clone(), kept, member));
                o
            }
        };
        ops.push(real);
    }
    if ops.is_empty() {
        return Ok(());
    }
    let got = if merge {
        let rec = log.commit("br1", "a1", "alice", ops);
        log.merge("br1", "main", "carol", "g1", false)
            .map(|_| rec.record.bindings)
    } else {
        log.commit_at("main", "a1", "alice", base_seq, ops)
            .map(|rec| rec.record.bindings)
    };
    let what = format!("base {base:?} head {head:?} alice {named:?}");
    match (model(&base, &head, &named, &moved), got) {
        (Want::Order(want), Ok(bindings)) => {
            let rename: Vec<(String, String)> = bindings.into_iter().collect();
            prop_assert_eq!(names(&log, "main", &rename), want, "{}", what);
        }
        (Want::Order(want), Err(e)) => {
            prop_assert!(false, "{what}: want {want:?}, got {e:?}");
        }
        (Want::Conflict { gone }, Err(e)) => {
            let reason = e.details["reason"].as_str().unwrap_or(e.code.as_str());
            prop_assert!(
                gone || reason == "SAME_PARAM",
                "{what}: conflict {reason}: {e:?}"
            );
        }
        (Want::Conflict { .. }, Ok(_)) => {
            let now = names(&log, "main", &[]);
            prop_assert!(false, "{what}: want a conflict, got {now:?}");
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(prop_config(256))]
    #[test]
    fn chained_index_ops_follow_their_neighbours(
        extra in 0usize..4,
        bob in proptest::collection::vec(bob_op(), 0..4),
        alice in proptest::collection::vec(alice_op(), 1..5),
        merge in any::<bool>(),
    ) {
        run(extra, &bob, &alice, merge)?;
    }
}
