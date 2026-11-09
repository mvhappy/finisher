//! An in-memory log of branches for the history planners ([`LogView`]): each branch keeps its
//! own records and the state after each of them, and forks from its parent at a seq.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use fin_core::history::{
    Lineage, LogRec, LogView, MergePlan, Planned, StackRec, Stacks, derive_stacks,
    plan_cherry_pick, plan_merge, plan_redo, plan_revert, plan_undo, plan_undo_to,
};
use fin_core::layout::TextMeasurer;
use fin_core::{
    Applied, ApplyEnv, ApplyMode, BoundIds, CoreError, Diff, Link, MemIds, Op, Project, Record,
    SCHEMA_VERSION, Tx, apply_tx_env, apply_tx_mode, rebase_tx_env, resolve_ops,
};

struct Br {
    parent: Option<String>,
    fork: u64,
    recs: Vec<LogRec>,
    /// `states[i]`: the state at seq `fork + i` (`states[0]`: at the fork).
    states: Vec<Project>,
}

pub struct MemLog {
    branches: BTreeMap<String, Br>,
    /// One id counter for the whole log, as the store's.
    pub ids: MemIds,
    /// What edits and the planners measure captions with (plan 4C Task 20); none by default.
    pub measurer: Option<Arc<dyn TextMeasurer>>,
}

impl MemLog {
    /// `main` at seq 0 with `genesis`.
    pub fn new(genesis: Project) -> MemLog {
        let ids = MemIds::after(&genesis);
        let main = Br {
            parent: None,
            fork: 0,
            recs: vec![],
            states: vec![genesis],
        };
        MemLog {
            branches: BTreeMap::from([("main".to_string(), main)]),
            ids,
            measurer: None,
        }
    }

    /// Branch `id` forked from `parent` at `at` (no `branch` record: the planners never
    /// read one).
    pub fn fork(&mut self, id: &str, parent: &str, at: u64) {
        let state = self.state_at(parent, at).unwrap();
        self.branches.insert(
            id.to_string(),
            Br {
                parent: Some(parent.to_string()),
                fork: at,
                recs: vec![],
                states: vec![state],
            },
        );
    }

    fn br(&self, b: &str) -> Result<&Br, CoreError> {
        self.branches
            .get(b)
            .ok_or_else(|| CoreError::new("BRANCH_NOT_FOUND", format!("no branch {b}")))
    }

    pub fn head_seq(&self, b: &str) -> u64 {
        let br = &self.branches[b];
        br.fork + br.recs.len() as u64
    }

    pub fn state(&self, b: &str) -> Project {
        self.branches[b].states.last().unwrap().clone()
    }

    /// An edit at the head of `b`, applied as a client tx.
    pub fn commit(&mut self, b: &str, tx_id: &str, author: &str, ops: Vec<Op>) -> LogRec {
        self.try_commit(b, tx_id, author, ops)
            .unwrap_or_else(|e| panic!("{tx_id}: {e:?}"))
    }

    /// [`MemLog::commit`], or the apply's error.
    pub fn try_commit(
        &mut self,
        b: &str,
        tx_id: &str,
        author: &str,
        ops: Vec<Op>,
    ) -> Result<LogRec, CoreError> {
        let tx = self.tx(b, tx_id, author, self.head_seq(b), ops);
        let env = ApplyEnv::measuring(self.measurer.as_deref());
        let applied = apply_tx_env(&self.state(b), &tx, &mut self.ids, ApplyMode::Client, env)?;
        Ok(self.push(b, Link::Edit, tx, applied))
    }

    /// An edit written at `base` on `b`, rebased over the branch's records since (stale).
    pub fn commit_at(
        &mut self,
        b: &str,
        tx_id: &str,
        author: &str,
        base: u64,
        ops: Vec<Op>,
    ) -> Result<LogRec, CoreError> {
        let tx = self.tx(b, tx_id, author, base, ops);
        let since = self
            .records(b, base, self.head_seq(b))?
            .iter()
            .map(LogRec::committed)
            .collect::<Result<Vec<_>, _>>()?;
        let at_base = self.state_at(b, base)?;
        let env = ApplyEnv::measuring(self.measurer.as_deref());
        let r = rebase_tx_env(&at_base, &self.state(b), &tx, &since, &mut self.ids, env)?;
        Ok(self.push(b, Link::Edit, r.tx, r.applied))
    }

    /// A record of kind `link` at the head of `b` with `ops` applied as an internal tx (a
    /// merge or cherry-pick copy, a checkpoint with no ops).
    pub fn append_link(
        &mut self,
        b: &str,
        tx_id: &str,
        author: &str,
        link: Link,
        ops: Vec<Op>,
    ) -> LogRec {
        let tx = self.tx(b, tx_id, author, self.head_seq(b), ops);
        let applied = if tx.ops.is_empty() {
            Applied {
                project: self.state(b),
                diff: Diff::default(),
                inverse: vec![],
                bindings: BTreeMap::new(),
            }
        } else {
            apply_tx_mode(&self.state(b), &tx, &mut self.ids, ApplyMode::Internal)
                .unwrap_or_else(|e| panic!("{tx_id}: {e:?}"))
        };
        self.push(b, link, tx, applied)
    }

    /// A planned record appended at the head of `b`.
    pub fn append(&mut self, b: &str, p: Planned) -> LogRec {
        assert_eq!(p.intent.base_rev, self.head_seq(b), "planned at the head");
        self.push(b, p.link, p.intent, p.applied)
    }

    pub fn undo(&mut self, b: &str, author: &str, tx_id: &str) -> Result<LogRec, CoreError> {
        let mut ids = self.ids.clone();
        let p = plan_undo(self, b, author, tx_id, &mut ids)?;
        self.ids = ids;
        Ok(self.append(b, p))
    }

    pub fn redo(&mut self, b: &str, author: &str, tx_id: &str) -> Result<LogRec, CoreError> {
        let mut ids = self.ids.clone();
        let p = plan_redo(self, b, author, tx_id, &mut ids)?;
        self.ids = ids;
        Ok(self.append(b, p))
    }

    /// A `checkpoint` record named `name` at the head of `b`; returns its seq.
    pub fn checkpoint(&mut self, b: &str, tx_id: &str, author: &str, name: &str) -> u64 {
        let link = Link::Checkpoint {
            name: name.into(),
            recovers: None,
        };
        self.append_link(b, tx_id, author, link, vec![]).seq
    }

    /// `undo --to` the checkpoint at `checkpoint_seq` on `b`, appended.
    pub fn undo_to(
        &mut self,
        b: &str,
        author: &str,
        checkpoint_seq: u64,
        tx_id: &str,
    ) -> Result<LogRec, CoreError> {
        let mut ids = self.ids.clone();
        let p = plan_undo_to(self, b, author, checkpoint_seq, tx_id, &mut ids)?;
        self.ids = ids;
        Ok(self.append(b, p))
    }

    /// Merges `from` into `into` and appends the planned copies.
    pub fn merge(
        &mut self,
        from: &str,
        into: &str,
        author: &str,
        merge_id: &str,
        skip_conflicting: bool,
    ) -> Result<MergePlan, CoreError> {
        let plan = plan_merge(self, from, into, author, merge_id, skip_conflicting)?;
        for p in plan.planned.clone() {
            self.append(into, p);
        }
        Ok(plan)
    }

    /// Cherry-picks `origin` into `into` and appends the copy (`None`: nothing to do).
    pub fn pick(
        &mut self,
        origin: &str,
        into: &str,
        author: &str,
        tx_id: &str,
    ) -> Result<Option<LogRec>, CoreError> {
        let p = plan_cherry_pick(self, origin, into, author, tx_id)?.planned();
        Ok(p.map(|p| self.append(into, p)))
    }

    /// Reverts merge group `merge_id` on `b` and appends the record (`None`: nothing to do).
    pub fn revert(
        &mut self,
        merge_id: &str,
        b: &str,
        author: &str,
        tx_id: &str,
    ) -> Result<Option<LogRec>, CoreError> {
        let mut ids = self.ids.clone();
        let p = plan_revert(self, merge_id, b, author, tx_id, &mut ids)?.planned();
        self.ids = ids;
        Ok(p.map(|p| self.append(b, p)))
    }

    /// A copy of `origin` at the head of `b` with kind `link`: its intent re-run as a client
    /// tx at the head, binding the origin's ids (a cherry-pick without a rebase).
    pub fn copy(
        &mut self,
        b: &str,
        tx_id: &str,
        author: &str,
        link: Link,
        origin: &LogRec,
    ) -> LogRec {
        let tx = self.tx(
            b,
            tx_id,
            author,
            self.head_seq(b),
            origin.record.intent.ops.clone(),
        );
        let mut ids = BoundIds::new(&tx.ops, &origin.record.bindings).unwrap();
        let applied = apply_tx_mode(&self.state(b), &tx, &mut ids, ApplyMode::Client)
            .unwrap_or_else(|e| panic!("{tx_id}: {e:?}"));
        self.push(b, link, tx, applied)
    }

    pub fn stacks(&self, b: &str, author: &str) -> Stacks {
        let br = &self.branches[b];
        let own: Vec<StackRec> = br.recs.iter().map(StackRec::of).collect();
        derive_stacks(&own, author, &self.merged_away(b).unwrap())
    }

    fn tx(&self, b: &str, tx_id: &str, author: &str, base: u64, ops: Vec<Op>) -> Tx {
        Tx {
            tx_id: tx_id.into(),
            author: author.into(),
            branch: b.into(),
            base_rev: base,
            schema_version: SCHEMA_VERSION,
            ops,
            args_hash: None,
            via: None,
        }
    }

    fn push(&mut self, b: &str, link: Link, intent: Tx, applied: Applied) -> LogRec {
        let seq = self.head_seq(b) + 1;
        let resolved = resolve_ops(&intent.ops, &applied.bindings).unwrap();
        let mut record = Record::new_edit(
            intent.tx_id.clone(),
            seq,
            SCHEMA_VERSION,
            None,
            intent.author.clone(),
            0,
            Tx {
                base_rev: seq - 1,
                ..intent
            },
            resolved,
            applied.inverse,
            applied.bindings,
        );
        record.set_link(link);
        record.link().expect("a well-formed record");
        let rec = LogRec {
            branch: b.into(),
            seq,
            record,
            diff: applied.diff,
        };
        let br = self.branches.get_mut(b).unwrap();
        br.recs.push(rec.clone());
        br.states.push(applied.project);
        rec
    }
}

impl LogView for MemLog {
    fn measurer(&self) -> Option<&dyn TextMeasurer> {
        self.measurer.as_deref()
    }

    fn lineage(&self, branch: &str) -> Result<Lineage, CoreError> {
        let mut segs = vec![];
        let (mut cur, mut through) = (branch.to_string(), None::<u64>);
        loop {
            let br = self.br(&cur)?;
            segs.push((cur.clone(), br.fork, through));
            let Some(p) = &br.parent else {
                return Ok(Lineage { segs });
            };
            through = Some(through.map_or(br.fork, |t| t.min(br.fork)));
            cur = p.clone();
        }
    }

    fn records(&self, branch: &str, after: u64, through: u64) -> Result<Vec<LogRec>, CoreError> {
        Ok(self
            .br(branch)?
            .recs
            .iter()
            .filter(|r| r.seq > after && r.seq <= through)
            .cloned()
            .collect())
    }

    fn head(&self, branch: &str) -> Result<(u64, Project), CoreError> {
        self.br(branch)?;
        Ok((self.head_seq(branch), self.state(branch)))
    }

    fn state_at(&self, branch: &str, seq: u64) -> Result<Project, CoreError> {
        let br = self.br(branch)?;
        if seq >= br.fork {
            let i = usize::try_from(seq - br.fork).unwrap();
            return br
                .states
                .get(i)
                .cloned()
                .ok_or_else(|| CoreError::new("DB_ERROR", format!("{branch} has no seq {seq}")));
        }
        match &br.parent {
            Some(p) => self.state_at(p, seq),
            None => Err(CoreError::new("DB_ERROR", "below the root")),
        }
    }

    fn find(&self, tx_id: &str) -> Result<Option<LogRec>, CoreError> {
        Ok(self
            .branches
            .values()
            .flat_map(|b| &b.recs)
            .find(|r| r.record.tx_id == tx_id)
            .cloned())
    }

    fn merge_group(&self, merge_id: &str) -> Result<Vec<LogRec>, CoreError> {
        Ok(self
            .branches
            .values()
            .flat_map(|b| &b.recs)
            .filter(|r| r.record.merge_id.as_deref() == Some(merge_id))
            .cloned()
            .collect())
    }

    fn is_descendant(&self, a: &str, of: &str) -> Result<bool, CoreError> {
        let mut cur = self.br(a)?.parent.clone();
        while let Some(p) = cur {
            if p == of {
                return Ok(true);
            }
            cur = self.br(&p)?.parent.clone();
        }
        Ok(false)
    }

    fn merged_away(&self, branch: &str) -> Result<HashSet<String>, CoreError> {
        let own: HashSet<&str> = self
            .br(branch)?
            .recs
            .iter()
            .map(|r| r.record.tx_id.as_str())
            .collect();
        let mut out = HashSet::new();
        for (id, br) in &self.branches {
            if id == branch || self.is_descendant(id, branch)? {
                continue;
            }
            for r in &br.recs {
                if let Ok(Link::Merge { from, origin, .. }) = r.record.link()
                    && from == branch
                    && own.contains(origin.as_str())
                {
                    out.insert(origin);
                }
            }
        }
        Ok(out)
    }
}
