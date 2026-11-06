//! One quarantined row re-run on its recovered branch ([`super::run`]): what stands for it,
//! planned or written. Each returns how many records it puts on the group's branch and
//! whether anything stands for the row, or the conflict that stops the group; a row it
//! cannot stand for is left in quarantine with a note (`notes`), and the group goes on.

use std::collections::BTreeSet;

use fin_core::{ApplyMode, Conflict, Link, Project, Record, RecordKind, apply_tx_mode};

use super::groups::{Fork, resolve_fork};
use super::ids::{Preset, Root, root};
use super::remap::Remap;
use super::run::{NO_COMMON_HISTORY, Runner};
use crate::branch::create::{At, BranchCreate, metadata_draft, plan_create_with};
use crate::branch::index;
use crate::db::Counters;
use crate::write::Draft;
use crate::{MAIN, StoreError};

/// What one row gave: records on the group's branch, and whether anything stands for it.
pub(super) type Done = Result<(u64, bool), Conflict>;

/// A note: `reason` for row `tx_id`, with `fix`.
fn note(tx_id: &str, reason: &str, fix: String) -> Conflict {
    let mut c = Conflict::new(tx_id, 0, reason, None);
    c.fix = fix;
    c
}

impl Runner<'_, '_, '_> {
    /// Row `i` (`rec`) of group `g`; `branch`: the group's recovered branch (written runs);
    /// `at`: where the group's rows stand now.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn row(
        &mut self,
        g: usize,
        i: usize,
        rec: &Record,
        fork_ids: &BTreeSet<String>,
        state: &mut Project,
        remap: &mut Remap,
        branch: Option<&str>,
        at: (String, u64),
        root_at: &(String, u64),
    ) -> Result<Done, StoreError> {
        match rec.kind {
            k if k.has_ops() => {
                let r = self.content(g, i, rec, fork_ids, state, remap, branch, root_at)?;
                Ok(r.map(|()| (1, true)))
            }
            RecordKind::Checkpoint => {
                if let Some(b) = branch {
                    let link = Link::Checkpoint {
                        name: rec.name.clone().unwrap_or_default(),
                        origin_hash: Some(self.forest.rows[i].hash.clone()),
                    };
                    self.no_op_record(b, link)?;
                }
                Ok(Ok((1, true)))
            }
            RecordKind::Branch => self.branch_row(g, i, rec, &at),
            RecordKind::BranchSet => self.branch_set(g, i, rec),
            _ => Ok(Ok((0, false))),
        }
    }

    /// Appends a record without ops to `branch`, standing for a row.
    fn no_op_record(&mut self, branch: &str, link: Link) -> Result<(), StoreError> {
        let tx_id = self.next_id()?;
        let author = self.author;
        let ctx = self.ctx.as_deref_mut().expect("writing");
        let draft = metadata_draft(ctx, branch, &tx_id, author, link)?;
        self.committed.push(ctx.append(draft)?);
        Ok(())
    }

    /// A record with ops: re-run under its fresh (or, from a live source, kept) ids and
    /// appended as a `cherryPick` of its origin root ([`super::ids`]).
    #[allow(clippy::too_many_arguments)]
    fn content(
        &mut self,
        g: usize,
        i: usize,
        rec: &Record,
        fork_ids: &BTreeSet<String>,
        state: &mut Project,
        remap: &mut Remap,
        branch: Option<&str>,
        root_at: &(String, u64),
    ) -> Result<Result<(), Conflict>, StoreError> {
        let hash = self.forest.rows[i].hash.clone();
        let root = root(self.conn, self.forest, rec)?;
        if let Root::Unknown { tx_id } = &root
            && !self.writes()
        {
            self.notes[g].push(note(
                &rec.tx_id,
                "SOURCE_NOT_FOUND",
                format!(
                    "the record {tx_id} it copies is not here: its copy gets new ids, so merging that record again later may bring it twice"
                ),
            ));
        }
        let bound = &mut self.bound;
        let mut tx = match remap.prepare(rec, fork_ids, &mut |id| bound.elsewhere(root_at, id))? {
            Ok(tx) => tx,
            Err(c) => return Ok(Err(c)),
        };
        tx.author = self.author.to_string();
        tx.args_hash = None;
        if let Some(b) = branch {
            tx.tx_id = self.next_id()?;
            tx.branch = b.to_string();
            tx.base_rev = self.ctx.as_deref_mut().expect("writing").head(b)?.rev;
        }
        let queue = self.root_ids.preset(self.conn, &root, rec, &tx)?;
        let mut alloc = Preset {
            queue,
            ids: &mut self.ids,
        };
        let applied = match apply_tx_mode(state, &tx, &mut alloc, ApplyMode::Internal) {
            Ok(a) => a,
            Err(e) => {
                let op = e.details.get("opIndex").and_then(serde_json::Value::as_u64);
                let op = usize::try_from(op.unwrap_or(0)).unwrap_or(0);
                return Ok(Err(Conflict::from_rerun(&e, &rec.tx_id, op, None)));
            }
        };
        remap.learn(rec, &applied.bindings);
        self.root_ids.learn(&root, rec, &applied.bindings);
        *state = applied.project.clone();
        if let Some(b) = branch {
            let from = match &root {
                Root::Live { branch, .. } | Root::Aside { branch, .. } => branch.clone(),
                Root::Unknown { .. } => rec.from.clone().unwrap_or(rec.intent.branch.clone()),
            };
            let ctx = self.ctx.as_deref_mut().expect("writing");
            let c = ctx.append(Draft {
                branch: b.to_string(),
                link: Link::CherryPick {
                    origin: root.tx_id().to_string(),
                    from,
                    origin_hash: Some(hash),
                },
                intent: tx,
                inverse: applied.inverse,
                bindings: applied.bindings,
                diff: applied.diff,
                project: applied.project,
            })?;
            self.committed.push(c);
        }
        Ok(Ok(()))
    }

    /// A `branch` row: nothing when a group of its branch's records stands for it, else the
    /// (empty) branch made again where it forked: on the recovered branch at the copy of its
    /// fork record, or where that record is in the log.
    fn branch_row(
        &mut self,
        g: usize,
        i: usize,
        rec: &Record,
        at: &(String, u64),
    ) -> Result<Done, StoreError> {
        if self.groups.iter().any(|gr| gr.made_by == Some(i)) {
            return Ok(Ok((0, false)));
        }
        let fork = match &rec.fork_hash {
            None => Some((MAIN.to_string(), 0)),
            Some(h) => match self.pos_of.get(h) {
                Some(p) => Some(p.clone()),
                None => match resolve_fork(self.conn, Some(h))? {
                    Fork::At { branch, seq, .. } => Some((branch, seq)),
                    Fork::NoCommonHistory => None,
                },
            },
        };
        let Some((from, seq)) = fork else {
            self.notes[g].push(note(
                &rec.tx_id,
                NO_COMMON_HISTORY,
                "the record the branch forked at is not here: import the folder that holds it (fin project recover --import <dir>), or dismiss the group".into(),
            ));
            return Ok(Ok((0, false)));
        };
        if !from.starts_with('#') && index::get(self.conn, &from)?.is_some_and(|b| b.archived) {
            self.notes[g].push(note(
                &rec.tx_id,
                "BRANCH_ARCHIVED",
                format!(
                    "fin branch archive {from} --restore, then fin project recover --create all"
                ),
            ));
            return Ok(Ok((0, false)));
        }
        let on_branch = u64::from(from == at.0);
        if !self.writes() {
            return Ok(Ok((on_branch, true)));
        }
        let ix = index::load(self.conn)?;
        let base = rec.name.clone().unwrap_or_else(|| "recovered".into());
        let mut name = base.clone();
        let mut k = 2;
        while ix.holder(&name).is_some() {
            name = format!("{base}-{k}");
            k += 1;
        }
        let tx_id = self.next_id()?;
        let author = self.author.to_string();
        let ctx = self.ctx.as_deref_mut().expect("writing");
        self.ids.save(ctx.txn)?;
        let made = plan_create_with(
            ctx,
            &BranchCreate {
                tx_id,
                author,
                name: Some(name),
                from,
                at: Some(At::Seq(seq)),
                dry_run: ctx.dry_run,
                args_hash: None,
            },
            Some(self.forest.rows[i].hash.clone()),
        )?;
        self.ids = Counters::load(ctx.txn)?;
        self.committed.push(made.record);
        self.created.push((g, made.branch.id));
        Ok(Ok((on_branch, true)))
    }

    /// A `branchSet` row: nothing when the branch it changes is set aside with it (its
    /// `branch` row brings it back); for a live branch the same change again, unless it no
    /// longer applies (left with a note).
    fn branch_set(&mut self, g: usize, i: usize, rec: &Record) -> Result<Done, StoreError> {
        let target = rec.branch_id.clone().unwrap_or_default();
        if !self.forest.makers(&target).is_empty() {
            return Ok(Ok((0, false)));
        }
        let ix = index::load(self.conn)?;
        let Some(info) = ix.get(&target) else {
            self.notes[g].push(note(
                &rec.tx_id,
                "BRANCH_NOT_FOUND",
                format!("branch {target} it changes is gone; dismiss the group (fin project recover --dismiss <group>)"),
            ));
            return Ok(Ok((0, false)));
        };
        let Some(parent) = info.parent.clone() else {
            self.notes[g].push(note(
                &rec.tx_id,
                "INVALID_OP",
                "main is never renamed or archived; dismiss the group (fin project recover --dismiss <group>)".into(),
            ));
            return Ok(Ok((0, false)));
        };
        let archived = rec.archived.unwrap_or(info.archived);
        let name = rec.name.clone().unwrap_or_else(|| info.name.clone());
        if !archived && let Some(holder) = ix.holder(&name).filter(|h| h.id != info.id) {
            self.notes[g].push(note(
                &rec.tx_id,
                "NAME_TAKEN",
                format!(
                    "branch {} holds the name {name:?}: rename or archive it (fin branch rename {} <new>), then fin project recover --create all",
                    ix.display(holder),
                    holder.id
                ),
            ));
            return Ok(Ok((0, false)));
        }
        if self.writes() {
            let link = Link::BranchSet {
                branch_id: target,
                name: rec.name.clone(),
                archived: rec.archived,
                origin_hash: Some(self.forest.rows[i].hash.clone()),
            };
            self.no_op_record(&parent, link)?;
        }
        Ok(Ok((0, true)))
    }
}
