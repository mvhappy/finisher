//! Replay property (spec §4.6 prop 5; plan 4B Task 20): a generated log with branches, edits
//! by several authors, undos, redos, merges, cherry-picks and a checkpoint, written through the
//! store. Replaying each branch from its first snapshot (`apply_committed`) gives its live
//! head, and a fresh server-data that imports the folder has the same heads, branch index,
//! `merged` states and undo stacks, and verifies.

mod integration_support;

use fin_core::{
    CommittedTx, CoreError, Edge, Op, Project, Rational, SCHEMA_VERSION, Tx, apply_committed,
};
use fin_store::{
    BranchCreate, CheckpointReq, CherryPickReq, HistoryReq, MAIN, MergeReq, Store, StoreError,
};
use integration_support as is;
use proptest::prelude::*;
use rusqlite::{Connection, params};

const AUTHORS: [&str; 3] = ["alice", "bob", "carol"];

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

fn secs(n: i64) -> Rational {
    Rational::from_int(n)
}

#[derive(Debug, Clone)]
enum Act {
    /// On branch `b` (an index into the branches made so far): an insert before clip `k` (or
    /// at the end), a delete or an end trim of clip `k`.
    Edit {
        b: usize,
        what: u8,
        k: usize,
        n: i64,
    },
    /// A branch forked from branch `from` at its head.
    Branch {
        from: usize,
    },
    Undo {
        b: usize,
    },
    Redo {
        b: usize,
    },
    /// Branch `from` merged into its parent.
    Merge {
        from: usize,
        skip: bool,
    },
    /// The `k`th record of branch `from` cherry-picked into its parent.
    Pick {
        from: usize,
        k: usize,
    },
    Checkpoint {
        b: usize,
    },
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        6 => (0usize..4, 0u8..3, 0usize..6, -3i64..4)
            .prop_map(|(b, what, k, n)| Act::Edit { b, what, k, n }),
        2 => (0usize..4).prop_map(|from| Act::Branch { from }),
        3 => (0usize..4).prop_map(|b| Act::Undo { b }),
        2 => (0usize..4).prop_map(|b| Act::Redo { b }),
        3 => (1usize..4, any::<bool>()).prop_map(|(from, skip)| Act::Merge { from, skip }),
        2 => (1usize..4, 0usize..8).prop_map(|(from, k)| Act::Pick { from, k }),
        1 => (0usize..4).prop_map(|b| Act::Checkpoint { b }),
    ]
}

/// Refusals the script may meet (and goes on): nothing to undo or redo, a conflict, a pick of
/// something already there, a trim past its limits, a merge with nothing in common.
const EXPECTED: [&str; 6] = [
    "NOTHING_TO_UNDO",
    "NOTHING_TO_REDO",
    "CONFLICT",
    "INVALID_RANGE",
    "INVALID_OP",
    "NAME_TAKEN",
];

fn allowed<T>(r: Result<T, StoreError>) -> Result<(), TestCaseError> {
    if let Err(e) = r {
        let e = CoreError::from(e);
        prop_assert!(EXPECTED.contains(&e.code.as_str()), "{e:?}");
    }
    Ok(())
}

struct Script {
    s: Store,
    branches: Vec<String>,
    n: usize,
}

impl Script {
    fn id(&mut self) -> String {
        self.n += 1;
        format!("t{}", self.n)
    }

    fn author(&self) -> &'static str {
        AUTHORS[self.n % AUTHORS.len()]
    }

    fn branch(&self, b: usize) -> String {
        self.branches[b % self.branches.len()].clone()
    }

    fn parent(&self, b: &str) -> Option<String> {
        self.s
            .branches(true)
            .unwrap()
            .into_iter()
            .find(|v| v.id == b)
            .and_then(|v| v.parent)
    }

    fn history(&mut self, b: &str) -> HistoryReq {
        let tx_id = self.id();
        HistoryReq {
            tx_id,
            author: self.author().into(),
            branch: b.into(),
            dry_run: false,
            args_hash: None,
            to: None,
        }
    }

    fn edit(&mut self, b: &str, what: u8, k: usize, n: i64) -> Result<(), TestCaseError> {
        let items = self.s.snapshot(b).unwrap().1.timeline.tracks[0]
            .items
            .clone();
        let clip = items.get(k % items.len().max(1));
        let op = match (what, clip) {
            (0, _) => Op::ClipInsert {
                id: "$c".into(),
                track: "t1".into(),
                at: clip.map_or_else(
                    || {
                        items
                            .last()
                            .map_or(secs(0), |c| (c.start + c.duration).unwrap())
                    },
                    |c| c.start,
                ),
                asset: "a1".into(),
                source_in: secs(0),
                duration: secs(n.abs() + 1),
            },
            (1, Some(c)) => Op::ClipDelete { clip: c.id.clone() },
            (_, Some(c)) if n != 0 => Op::ClipTrim {
                clip: c.id.clone(),
                edge: Edge::End,
                delta: secs(n),
            },
            _ => return Ok(()),
        };
        let tx_id = self.id();
        let r = self.s.commit(Tx {
            tx_id,
            author: self.author().into(),
            branch: b.into(),
            base_rev: self.s.head(b),
            schema_version: SCHEMA_VERSION,
            args_hash: None,
            ops: vec![op],
        });
        allowed(r)
    }

    fn step(&mut self, act: &Act) -> Result<(), TestCaseError> {
        match *act {
            Act::Edit { b, what, k, n } => {
                let b = self.branch(b);
                self.edit(&b, what, k, n)
            }
            Act::Branch { from } => {
                let from = self.branch(from);
                let tx_id = self.id();
                let made = self.s.create_branch(BranchCreate {
                    tx_id,
                    author: self.author().into(),
                    name: None,
                    from,
                    at: None,
                    dry_run: false,
                    args_hash: None,
                });
                self.branches.push(made.unwrap().branch.id);
                Ok(())
            }
            Act::Undo { b } => {
                let r = self.history(&self.branch(b));
                allowed(self.s.undo(r))
            }
            Act::Redo { b } => {
                let r = self.history(&self.branch(b));
                allowed(self.s.redo(r))
            }
            Act::Merge { from, skip } => {
                let from = self.branch(from);
                let Some(into) = self.parent(&from) else {
                    return Ok(());
                };
                let tx_id = self.id();
                let r = self.s.merge(MergeReq {
                    tx_id,
                    author: self.author().into(),
                    from,
                    into,
                    skip_conflicting: skip,
                    dry_run: false,
                    args_hash: None,
                });
                allowed(r)
            }
            Act::Pick { from, k } => {
                let from = self.branch(from);
                let Some(into) = self.parent(&from) else {
                    return Ok(());
                };
                let fork = self
                    .s
                    .branches(true)
                    .unwrap()
                    .into_iter()
                    .find(|v| v.id == from)
                    .unwrap()
                    .fork_seq;
                let own: Vec<CommittedTx> = self.s.ops_since(&from, fork).unwrap();
                let Some(origin) = own.get(k % own.len().max(1)) else {
                    return Ok(());
                };
                let tx_id = self.id();
                let r = self.s.cherry_pick(CherryPickReq {
                    tx_id,
                    author: self.author().into(),
                    origin: origin.tx.tx_id.clone(),
                    into,
                    dry_run: false,
                    args_hash: None,
                });
                allowed(r)
            }
            Act::Checkpoint { b } => {
                let branch = self.branch(b);
                let tx_id = self.id();
                let r = self.s.checkpoint(CheckpointReq {
                    tx_id,
                    author: self.author().into(),
                    branch,
                    name: "k1".into(),
                    dry_run: false,
                    args_hash: None,
                });
                allowed(r)
            }
        }
    }
}

/// Every branch with its index row and its head (rev and project).
fn heads(s: &Store) -> Vec<(fin_store::BranchView, u64, Project)> {
    s.branches(true)
        .unwrap()
        .into_iter()
        .map(|b| {
            let (rev, p) = s.snapshot(&b.id).unwrap();
            (b, rev, p)
        })
        .collect()
}

/// Each branch replayed from its first snapshot equals its head.
fn assert_replays(s: &Store) -> Result<(), TestCaseError> {
    let conn = Connection::open(s.db_path()).unwrap();
    for b in s.branches(true).unwrap() {
        let (seq, json): (i64, String) = conn
            .query_row(
                "SELECT seq, json FROM snapshots WHERE branch = ?1 ORDER BY seq LIMIT 1",
                params![b.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let mut p: Project = serde_json::from_str(&json).unwrap();
        for c in s.ops_since(&b.id, u64::try_from(seq).unwrap()).unwrap() {
            p = apply_committed(&p, &c.tx, &c.bindings)
                .unwrap_or_else(|e| panic!("{} {}: {e:?}", b.id, c.tx.tx_id))
                .project;
        }
        prop_assert_eq!(&p, &s.snapshot(&b.id).unwrap().1, "replay of {}", b.id);
    }
    Ok(())
}

proptest! {
    #![proptest_config(config(16))]
    #[test]
    fn replay_and_export_import_roundtrip(
        mut acts in prop::collection::vec(act(), 4..24),
        at in any::<prop::sample::Index>(),
        cp in 0usize..4,
    ) {
        // Every log has a branch, a checkpoint somewhere and a merge of the branch at the end.
        acts.insert(0, Act::Branch { from: 0 });
        acts.insert(1 + at.index(acts.len()), Act::Checkpoint { b: cp });
        acts.push(Act::Merge { from: 1, skip: true });
        let e = is::env();
        let mut s = e.create("a");
        is::commit(&mut s, "s1", vec![is::asset("/media/a.mov")]);
        let clips = (0..3)
            .map(|i| Op::ClipInsert {
                id: format!("$c{i}"),
                track: "t1".into(),
                at: secs(10 * i),
                asset: "a1".into(),
                source_in: secs(0),
                duration: secs(10),
            })
            .collect();
        is::commit(&mut s, "s2", clips);
        let mut run = Script { s, branches: vec![MAIN.to_string()], n: 0 };
        for act in &acts {
            run.step(act)?;
        }
        let mut s = run.s;
        assert_replays(&s)?;
        let v = s.verify();
        prop_assert!(v.ok, "{v:?}");

        e.flush(&mut s);
        let mut f = e.open("f");
        e.poll(&mut f);
        prop_assert_eq!(heads(&f), heads(&s));
        assert_replays(&f)?;
        for b in s.branches(true).unwrap() {
            for a in AUTHORS {
                prop_assert_eq!(
                    f.undo_stacks(&b.id, a).unwrap(),
                    s.undo_stacks(&b.id, a).unwrap(),
                    "stacks of {} on {}", a, b.id
                );
            }
        }
        let v = f.verify();
        prop_assert!(v.ok, "{v:?}");
    }
}
