//! A diverged tail (plan 4B Task 16), as `tests/recover.rs` sets it up: A (epoch 1) writes,
//! pauses, B takes over (epoch 2); A's late files reach the follower F before B's.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use fin_store::Store;

use super::{Env, asset, commit, held_epoch, insert};

pub fn freeze(s: &Store) {
    s.lease().expect("holding").fail_heartbeats(u32::MAX);
}

pub fn poll_until_held(e: &Env, s: &mut Store, max: u64) {
    let mut waited = 0;
    while held_epoch(s).is_none() {
        assert!(waited <= max, "not acquired: {:?}", s.lease_state());
        e.advance(2);
        waited += 2;
        e.poll(s);
    }
}

pub fn all_segments(dir: &Path) -> Vec<PathBuf> {
    let root = dir.join(".finisher").join("segments");
    let mut out = vec![];
    for d in fs::read_dir(&root).into_iter().flatten().flatten() {
        if d.path().is_dir() {
            for f in fs::read_dir(d.path()).unwrap().flatten() {
                out.push(f.path());
            }
        }
    }
    out.sort();
    out
}

pub struct Split {
    pub a: Store,
    pub b: Store,
    pub f: Store,
}

/// As `tests/cascade.rs`: A commits `t0` (an asset) and `before`, exports, pauses; B takes
/// over (epoch 2); A's `late` writes (still epoch 1, test lease) are exported and F applies
/// them before B, which does not see them, writes `won` and exports.
pub fn split(
    e: &Env,
    before: impl FnOnce(&mut Store),
    late: impl FnOnce(&mut Store),
    won: impl FnOnce(&mut Store),
) -> Split {
    let mut a = e.create("a");
    commit(&mut a, "t0", vec![asset("/m/0.mov")]);
    before(&mut a);
    e.flush(&mut a);
    let mut f = e.open("f");
    let mut b = e.open("b");
    freeze(&a);
    poll_until_held(e, &mut b, 80);
    assert_eq!(held_epoch(&b), Some(2));
    e.advance(10);
    e.poll(&mut b);
    assert_eq!(b.read_only(), None);
    a.set_test_lease(1, true);
    let had: BTreeSet<PathBuf> = all_segments(&e.dir).into_iter().collect();
    late(&mut a);
    e.flush(&mut a);
    let late_files: Vec<PathBuf> = all_segments(&e.dir)
        .into_iter()
        .filter(|p| !had.contains(p))
        .collect();
    e.poll(&mut f);
    for p in &late_files {
        fin_store::scan::hide_for_test(p);
    }
    won(&mut b);
    e.flush(&mut b);
    for p in &late_files {
        fin_store::scan::unhide_for_test(p);
    }
    Split { a, b, f }
}

pub fn winner(b: &mut Store) {
    commit(b, "u1", vec![insert(0, "a1")]);
}

/// A's unexported `tail` loses to B's `u1`: a diverged tail on A, which then holds a test
/// lease at epoch 3 to write.
pub fn diverged(e: &Env, tail: impl FnOnce(&mut Store)) -> Split {
    let mut s = split(e, |_| {}, |_| {}, winner);
    tail(&mut s.a);
    s.a.set_test_lease(1, false);
    e.poll(&mut s.a);
    s.a.set_test_lease(3, true);
    s
}
