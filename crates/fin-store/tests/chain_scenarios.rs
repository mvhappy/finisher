//! Chain resolution: multi-server scenarios and malformed input (review cases R1–R11).

mod chain_support;

use chain_support::*;

fn reasons(r: &Resolution) -> Vec<(String, QuarantineReason)> {
    r.quarantine
        .iter()
        .flat_map(|(s, q)| s.txs().iter().map(move |m| (m.tx_id.clone(), q.clone())))
        .collect()
}

fn descends_from(r: &Resolution, tx: &str) -> bool {
    reasons(r).iter().any(|(id, q)| {
        id == tx && matches!(q, QuarantineReason::Malformed(w) if w.starts_with("descends from"))
    })
}

fn pending(r: &Resolution) -> Vec<String> {
    r.pending
        .iter()
        .flat_map(|s| s.txs().iter().map(|m| m.tx_id.clone()))
        .collect()
}

// R1: descendants of a segment that is malformed by its link are set aside, not dropped.
#[test]
fn descendants_of_a_malformed_segment_are_quarantined() {
    let s1 = next(None, 1, &["a", "b"]);
    let first = &s1.txs()[0];
    let bad = build(
        1,
        Some(s1.hash().to_string()),
        vec![rec("c", first.seq + 1, Some(&first.tx_hash), "")],
        vec![],
        0,
    );
    let child = next(Some(&bad), 1, &["d", "e"]);
    let grandchild = next(Some(&child), 1, &["f"]);
    let r = resolve(&[s1, bad.clone(), child, grandchild], &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a", "b"]));
    assert!(
        r.quarantine
            .iter()
            .any(|(s, q)| s == &bad && matches!(q, QuarantineReason::Malformed(_)))
    );
    for tx in ["d", "e", "f"] {
        assert!(descends_from(&r, tx), "{tx}: {r:#?}");
    }
    assert!(r.waiting.is_empty() && r.pending.is_empty());
    no_fork(&r);
}

// R1b, R1c: a child of an unparseable segment never waits on that present hash.
#[test]
fn child_of_an_unparseable_segment_does_not_wait_on_it() {
    let s1 = next(None, 1, &["a"]);
    let last = &s1.txs()[0];
    let c = rec("c", last.seq + 1, Some(&last.tx_hash), "");
    let c_hash = blake3_of(&c);
    let bad = build(
        1,
        Some(s1.hash().to_string()),
        vec![c, br#"{"seq":3}"#.to_vec()],
        vec![],
        0,
    );
    assert!(bad.parse_error().is_some());
    let child = build(
        1,
        Some(bad.hash().to_string()),
        vec![rec("d", 3, Some(&c_hash), "")],
        vec![],
        0,
    );
    let hidden = blake3_of(br#"{"seq":3}"#);
    let child2 = build(
        1,
        Some(bad.hash().to_string()),
        vec![rec("e", 4, Some(&hidden), "")],
        vec![],
        0,
    );
    let r = resolve(&[s1, bad, child, child2], &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a"]));
    assert!(r.waiting.is_empty(), "{r:#?}");
    assert!(descends_from(&r, "d") && descends_from(&r, "e"), "{r:#?}");
}

// R2: a server without the lease never keeps its tail against a folder that moved on.
#[test]
fn tail_without_lease_never_wins() {
    // A wrote s1@1; B took over @2 and committed b1 (unexported); stale A wrote x@1; B stalled
    // and C took over @3, applied x and wrote nothing. B, now following:
    let s1 = next(None, 1, &["a"]);
    let x = next(Some(&s1), 1, &["x"]);
    let b1 = next(Some(&s1), 2, &["b1"]);
    let kb = without_lease(with_tail(known(&[&s1], LOCAL), &b1, 2));
    let r = resolve(&[s1.clone(), x.clone()], &kb);
    assert!(r.local_tail_diverged, "{r:#?}");
    assert_eq!(applied(&r), ids(&["x"]));
    assert!(r.quarantine.is_empty());
    no_fork(&r);
    // On sync storage the lease is advisory: see `sync_tail_without_lease_is_a_fork`.
    // With nothing new in the folder the tail just stays.
    let r = resolve(std::slice::from_ref(&s1), &kb);
    assert!(!r.local_tail_diverged && r.apply.is_empty());
    // Had B kept the tail and later exported it at a new epoch, C would see a fork, not
    // silently diverge.
    let b1e = next(Some(&s1), 4, &["b1"]);
    let r = resolve(&[s1.clone(), x.clone(), b1e], &known(&[&s1, &x], LOCAL));
    assert_eq!(r.fork.unwrap().reason, ForkReason::AppliedLineLost);
}

// R3: three servers. C@3 built y on the stale x@1; B's t@2 continues s1 too. C's line holds
// the newest epoch, so it wins and t is set aside.
#[test]
fn line_with_the_newest_epoch_wins_over_a_middle_holder() {
    let s1 = next(None, 1, &["a"]);
    let x = next(Some(&s1), 1, &["x"]);
    let t = next(Some(&s1), 2, &["t"]);
    let y = next(Some(&x), 3, &["y"]);
    let all = [s1.clone(), x.clone(), t.clone(), y.clone()];
    let r = resolve(&all, &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a", "x", "y"]));
    assert_eq!(
        r.quarantine,
        vec![(t.clone(), QuarantineReason::StaleEpoch)]
    );
    no_fork(&r);
    let r = resolve(&all, &known(&[&s1, &x, &y], LOCAL));
    assert!(r.apply.is_empty());
    assert_eq!(
        r.quarantine,
        vec![(t.clone(), QuarantineReason::StaleEpoch)]
    );
    let r = resolve(&all, &known(&[&s1, &t], LOCAL));
    assert_eq!(r.fork.unwrap().reason, ForkReason::AppliedLineLost);
}

// R5: a tail committed at an older epoch loses to a holder that came between.
#[test]
fn old_epoch_tail_loses_to_intermediate_holder() {
    let s1 = next(None, 1, &["a"]);
    let u = next(Some(&s1), 3, &["u"]);
    let t1 = next(Some(&s1), 2, &["t1"]);
    let r = resolve(&[s1.clone(), u], &with_tail(known(&[&s1], LOCAL), &t1, 2));
    assert!(r.local_tail_diverged);
    assert_eq!(applied(&r), ids(&["u"]));
}

// R6: a stale writer keeps writing after the holder imported part of its output.
#[test]
fn stale_writer_after_holder_import() {
    let s1 = next(None, 1, &["a"]);
    let x1 = next(Some(&s1), 1, &["x1"]);
    let x2 = next(Some(&x1), 1, &["x2"]);
    let x3 = next(Some(&x2), 1, &["x3"]);
    let b1 = next(Some(&x1), 2, &["b1"]);
    let input = [s1.clone(), x1.clone(), x2.clone(), x3.clone()];
    let r = resolve(&input, &with_tail(known(&[&s1, &x1], LOCAL), &b1, 2));
    assert!(r.apply.is_empty() && !r.local_tail_diverged);
    let mut q = quarantined(&r);
    q.sort();
    assert_eq!(q, ids(&["x2", "x3"]));
    // A, whose DB has x1..x3, later sees B's exported b1.
    let mut all = input.to_vec();
    all.push(b1);
    let r = resolve(&all, &known(&[&s1, &x1, &x2, &x3], LOCAL));
    assert_eq!(r.fork.unwrap().reason, ForkReason::AppliedLineLost);
}

// R7: the tail diverges inside its exported part.
#[test]
fn divergence_inside_the_exported_tail() {
    let s1 = next(None, 1, &["a"]);
    let s2 = next(Some(&s1), 2, &["c"]);
    let s3 = next(Some(&s2), 2, &["d"]);
    let w = next(Some(&s2), 3, &["w"]);
    let k = with_tail(with_tail(known(&[&s1], LOCAL), &s2, 2), &s3, 2);
    let r = resolve(&[s1, s2.clone(), s3.clone(), w], &k);
    assert_eq!(
        r.mark_exported,
        vec![("c".to_string(), s2.hash().to_string())]
    );
    assert!(r.local_tail_diverged);
    assert_eq!(r.quarantine, vec![(s3, QuarantineReason::StaleEpoch)]);
    assert_eq!(applied(&r), ids(&["w"]));
}

// R8 and the planted "any pack stands in for its predecessor" bug: a pack that lists an
// absent segment without holding its records must not stand in for it. (Its `replaces`
// entry names the segment's last record, which it does not hold: malformed at once.)
#[test]
fn pack_that_lacks_a_replaced_record_does_not_stand_in() {
    let s1 = next(None, 1, &["a", "b"]);
    let s2 = next(Some(&s1), 1, &["c"]);
    let s3 = next(Some(&s2), 1, &["d"]);
    let liar = build(
        2,
        None,
        s1.records().map(<[u8]>::to_vec).collect(),
        vec![rep(&s1), rep(&s2)],
        5,
    );
    let r = resolve(&[liar.clone(), s3.clone()], &Known::empty(LOCAL));
    assert!(applied(&r).is_empty(), "{r:#?}");
    assert_eq!(r.waiting, vec![s2.hash().to_string()]);
    assert_eq!(pending(&r), ids(&["d"]));
    assert!(
        matches!(&r.quarantine[..], [(s, QuarantineReason::Malformed(_))] if s.hash() == liar.hash())
    );

    // s2's record is present, but only in another file (a re-export): the pack lists s2 and
    // does not hold it, so s3 still waits for s2 itself.
    let s2b = build(
        1,
        Some(s1.hash().to_string()),
        s2.records().map(<[u8]>::to_vec).collect(),
        vec![],
        9,
    );
    let r = resolve(&[liar, s2b, s3], &Known::empty(LOCAL));
    assert!(r.waiting.contains(&s2.hash().to_string()));
    assert!(!applied(&r).contains(&"d".to_string()) && pending(&r).contains(&"d".to_string()));
    no_fork(&r);
}

// R9 and the planted "tail unexported whenever non-empty" bug: a fully exported tail is no
// longer a competing line.
#[test]
fn fully_exported_tail_then_any_child_applies() {
    let s1 = next(None, 1, &["a"]);
    let s2 = next(Some(&s1), 2, &["c", "d"]);
    let u = next(Some(&s2), 3, &["u"]);
    let x = next(Some(&s2), 1, &["x"]);
    for kind in [LOCAL, SYNC] {
        let mut k = with_tail(known(&[&s1], kind), &s2, 2);
        for (child, name) in [(&u, "u"), (&x, "x")] {
            for lease in [true, false] {
                k.holds_lease = lease;
                let r = resolve(&[s1.clone(), s2.clone(), child.clone()], &k);
                assert_eq!(r.mark_exported.len(), 2);
                assert!(!r.local_tail_diverged, "{kind:?} {name} {lease}: {r:#?}");
                assert_eq!(applied(&r), ids(&[name]));
                assert!(r.quarantine.is_empty());
                no_fork(&r);
            }
        }
    }
}

// R10: the whole branch so far is this server's crashed export; a stale sibling loses.
#[test]
fn exported_first_segment_beats_stale_sibling() {
    let s1 = next(None, 2, &["a"]);
    let x = next(None, 1, &["x"]);
    let r = resolve(
        &[s1.clone(), x.clone()],
        &with_tail(Known::empty(LOCAL), &s1, 2),
    );
    assert_eq!(
        r.mark_exported,
        vec![("a".to_string(), s1.hash().to_string())]
    );
    assert_eq!(r.quarantine, vec![(x, QuarantineReason::StaleEpoch)]);
    assert!(r.apply.is_empty() && !r.local_tail_diverged);
}

// R11: equal-epoch siblings after the DB head.
#[test]
fn equal_epoch_siblings_after_head_fork() {
    let s1 = next(None, 1, &["a"]);
    let s2 = next(Some(&s1), 1, &["b"]);
    let x = next(Some(&s1), 1, &["x"]);
    let r = resolve(&[s1.clone(), s2, x], &known(&[&s1], LOCAL));
    assert_eq!(r.fork.as_ref().unwrap().reason, ForkReason::EqualEpoch);
    let mut p = pending(&r);
    p.sort();
    assert_eq!(p, ids(&["b", "x"]));
}

// Only 4B branches may start at a fork point: on main, a first segment names no parent.
#[test]
fn main_start_with_a_parent_record_is_malformed() {
    let fork_point = blake3_of(b"elsewhere");
    let bad = build(1, None, vec![rec("a", 5, Some(&fork_point), "")], vec![], 0);
    let r = resolve(std::slice::from_ref(&bad), &Known::empty(LOCAL));
    assert!(r.apply.is_empty());
    assert!(matches!(&r.quarantine[..], [(s, QuarantineReason::Malformed(_))] if s == &bad));

    let branch = build_on(
        "br1",
        1,
        None,
        vec![rec("a", 5, Some(&fork_point), "")],
        vec![],
        0,
    );
    let r = resolve(std::slice::from_ref(&branch), &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a"]));
}

// The DB's head is not in the folder: nothing is decided, everything is pending.
#[test]
fn head_missing_leaves_everything_pending() {
    let s1 = next(None, 1, &["a"]);
    let s2 = next(Some(&s1), 1, &["b"]);
    let s3 = next(Some(&s2), 1, &["c"]);
    let r = resolve(&[s1.clone(), s3], &known(&[&s1, &s2], LOCAL));
    assert_eq!(r.waiting, vec![s2.hash().to_string()]);
    assert_eq!(pending(&r), ids(&["c"]));
}

// Spec §3.5: on sync storage two offline writers can both hold epochs, so a tail without the
// lease against a folder line is a fork for the user to settle, never set aside silently.
#[test]
fn sync_tail_without_lease_is_a_fork() {
    let s1 = next(None, 1, &["a"]);
    let x = next(Some(&s1), 1, &["x"]);
    let b1 = next(Some(&s1), 2, &["b1"]);
    let k = without_lease(with_tail(known(&[&s1], SYNC), &b1, 2));
    let r = resolve(&[s1.clone(), x.clone()], &k);
    assert!(!r.local_tail_diverged, "{r:#?}");
    assert!(r.apply.is_empty() && r.quarantine.is_empty());
    let fork = r.fork.expect("fork");
    assert_eq!(fork.reason, ForkReason::SyncFolder);
    assert_eq!(fork.sides.last(), Some(&ForkSide::LocalTail { epoch: 2 }));
    assert_eq!(fork.sides.len(), 2);
    // Nothing new in the folder: no fork, the tail stays.
    let r = resolve(std::slice::from_ref(&s1), &k);
    assert!(r.fork.is_none() && !r.local_tail_diverged);
}

// Without the lease on network/local storage, the tail is set aside and the folder's own
// lines can still tie: both outcomes come in one Resolution.
#[test]
fn no_lease_tail_diverges_and_folder_lines_fork_together() {
    let s1 = next(None, 1, &["a"]);
    let x = next(Some(&s1), 2, &["x"]);
    let y = next(Some(&s1), 2, &["y"]);
    let b1 = next(Some(&s1), 1, &["b1"]);
    let k = without_lease(with_tail(known(&[&s1], LOCAL), &b1, 1));
    let r = resolve(&[s1, x, y], &k);
    assert!(r.local_tail_diverged);
    assert_eq!(r.fork.expect("fork").reason, ForkReason::EqualEpoch);
    assert!(r.apply.is_empty());
}

// A pack stands in for an absent predecessor only if the successor's seq continues the
// parent record it holds.
#[test]
fn pack_stand_in_still_checks_seq() {
    let s1 = next(None, 1, &["a"]);
    let s2 = next(Some(&s1), 1, &["b"]);
    let p = pack(2, &[&s1, &s2]);
    let b = &s2.txs()[0];
    let bad = build(
        1,
        Some(s2.hash().to_string()),
        vec![rec("c", b.seq + 2, Some(&b.tx_hash), "")],
        vec![],
        0,
    );
    let r = resolve(&[p, bad.clone()], &Known::empty(LOCAL));
    assert_eq!(applied(&r), ids(&["a", "b"]));
    assert!(
        matches!(&r.quarantine[..], [(s, QuarantineReason::Malformed(_))] if s == &bad),
        "{r:#?}"
    );
}
