//! Fresh ids for recovered records (plan 4B Decision 27): each record of a group is re-run with
//! a `$placeholder` for every id its original bound, so the store's id counters hand out new
//! ones, and later records of the group that name an old id literally (their inverse-derived
//! ops included: an undo's `clip.remove`, a restore) name the new one.
//!
//! A literal id that is neither remapped nor present at the group's fork state is checked
//! against the database: when a record off the fork's lineage binds it (another server's
//! entity under the same id, Decision 33), the group is not created (`ID_COLLISION`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use fin_core::{Conflict, Project, Record, RecordKind, Tx};
use rusqlite::Connection;

use crate::StoreError;
use crate::branch::lineage::{self, LineageSeg};
use crate::db::{parse_record, to_u64};

/// The ids a group's records bound, old to new.
#[derive(Debug, Clone, Default)]
pub(crate) struct Remap {
    map: BTreeMap<String, String>,
}

/// Every entity id in `project` ([`Project::entity_ids`]).
pub(crate) fn entity_ids(project: &Project) -> BTreeSet<String> {
    project.entity_ids().into_iter().map(String::from).collect()
}

impl Remap {
    /// `rec.intent` with every id its bindings created turned back into its `$placeholder`,
    /// and every literal id in the remap rewritten. `Ok(Err(ID_COLLISION))` for a literal id
    /// neither remapped nor in `fork_ids` (the entity ids at the group's fork state) that
    /// `bound_elsewhere` finds in the database. The caller sets the tx's id, author, branch
    /// and base.
    pub(crate) fn prepare(
        &self,
        rec: &Record,
        fork_ids: &BTreeSet<String>,
        bound_elsewhere: &mut dyn FnMut(&str) -> Result<bool, StoreError>,
    ) -> Result<Result<Tx, Conflict>, StoreError> {
        let placeholder: BTreeMap<&str, &str> = rec
            .bindings
            .iter()
            .map(|(k, v)| (v.as_str(), k.as_str()))
            .collect();
        let mut tx = rec.intent.clone();
        for (i, op) in tx.ops.iter_mut().enumerate() {
            for r in op.refs_mut() {
                if r.starts_with('$') {
                    continue;
                }
                if let Some(p) = placeholder.get(r.as_str()) {
                    *r = (*p).to_string();
                } else if let Some(new) = self.map.get(r.as_str()) {
                    *r = new.clone();
                } else if !fork_ids.contains(r.as_str()) && bound_elsewhere(r)? {
                    let mut c = Conflict::new(
                        rec.tx_id.clone(),
                        i,
                        fin_core::rebase::reasons::ID_COLLISION,
                        None,
                    );
                    c.details = Some(Box::new(serde_json::json!({ "id": r.as_str() })));
                    return Ok(Err(c));
                }
            }
        }
        Ok(Ok(tx))
    }

    /// Learns the ids `rec`'s re-run bound: each of its placeholders' old id to the new one.
    pub(crate) fn learn(&mut self, rec: &Record, new_bindings: &BTreeMap<String, String>) {
        for (p, old) in &rec.bindings {
            if let Some(new) = new_bindings.get(p) {
                self.map.insert(old.clone(), new.clone());
            }
        }
    }
}

/// Whether a record off the lineage up to a root binds an id (memoised).
pub(crate) struct Bound<'a> {
    pub(crate) conn: &'a Connection,
    lineages: HashMap<String, Vec<LineageSeg>>,
    memo: HashMap<(String, u64, String), bool>,
}

impl<'a> Bound<'a> {
    pub(crate) fn new(conn: &'a Connection) -> Self {
        Bound {
            conn,
            lineages: HashMap::new(),
            memo: HashMap::new(),
        }
    }

    /// A `txs` record that binds `id` and is not on `root`'s lineage at or below its seq: not
    /// counting a `merge` or `cherryPick` copy whose origin binds the same id (copies keep
    /// their source's ids, Decision 18).
    pub(crate) fn elsewhere(&mut self, root: &(String, u64), id: &str) -> Result<bool, StoreError> {
        let key = (root.0.clone(), root.1, id.to_string());
        if let Some(b) = self.memo.get(&key) {
            return Ok(*b);
        }
        if !self.lineages.contains_key(&root.0) {
            let segs = lineage::lineage(self.conn, &root.0)?;
            self.lineages.insert(root.0.clone(), segs);
        }
        let segs = &self.lineages[&root.0];
        let on_lineage = |branch: &str, seq: u64| {
            seq <= root.1
                && segs.iter().any(|s| {
                    s.branch == branch && seq > s.after && s.through.is_none_or(|t| seq <= t)
                })
        };
        let pattern = format!("\"{id}\"").into_bytes();
        let mut stmt = self.conn.prepare_cached(
            "SELECT branch, seq, record_bytes FROM txs WHERE instr(record_bytes, ?1) > 0",
        )?;
        let rows: Vec<(String, i64, Vec<u8>)> = stmt
            .query_map([pattern], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        let mut found = false;
        for (branch, seq, bytes) in rows {
            let seq = to_u64(seq)?;
            if on_lineage(&branch, seq) {
                continue;
            }
            let rec = parse_record(Some(seq), &bytes)?;
            if !rec.bindings.values().any(|v| v == id) {
                continue;
            }
            if matches!(rec.kind, RecordKind::Merge | RecordKind::CherryPick)
                && let Some(origin) = &rec.origin
                && origin_binds(self.conn, origin, id)?
            {
                continue;
            }
            found = true;
            break;
        }
        self.memo.insert(key, found);
        Ok(found)
    }
}

fn origin_binds(conn: &Connection, origin: &str, id: &str) -> Result<bool, StoreError> {
    Ok(crate::db::find_tx(conn, origin)?.is_some_and(|c| c.bindings.values().any(|v| v == id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fin_core::{Edge, Op, Rational, SCHEMA_VERSION};

    fn record(ops: Vec<Op>, bindings: &[(&str, &str)]) -> Record {
        let intent = Tx {
            tx_id: "x".into(),
            author: "a".into(),
            branch: "main".into(),
            base_rev: 1,
            schema_version: SCHEMA_VERSION,
            ops,
            args_hash: None,
        };
        Record::new_edit(
            "x".into(),
            2,
            SCHEMA_VERSION,
            None,
            "a".into(),
            0,
            intent,
            vec![],
            vec![],
            bindings
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    fn trim(clip: &str) -> Op {
        fin_core::ItemTrim {
            item: clip.into(),
            edge: Edge::End,
            delta: Rational::from_int(-1),
            mode: fin_core::TrimMode::Ripple,
            alone: false,
        }
        .into()
    }

    fn remove(id: &str) -> Op {
        fin_core::ItemRemove {
            id: id.into(),
            mode: fin_core::DeleteMode::Ripple,
        }
        .into()
    }

    #[test]
    fn bound_ids_become_placeholders_and_later_literals_follow_the_remap() {
        let mut m = Remap::default();
        let first = record(vec![remove("c10")], &[("$c", "c10")]);
        let mut never = |_: &str| -> Result<bool, StoreError> { panic!("not asked") };
        let tx = m
            .prepare(&first, &BTreeSet::new(), &mut never)
            .unwrap()
            .unwrap();
        assert_eq!(tx.ops, [remove("$c")]);
        m.learn(&first, &BTreeMap::from([("$c".into(), "c42".into())]));
        let later = record(vec![trim("c10"), trim("c3")], &[]);
        let fork = BTreeSet::from(["c3".to_string()]);
        let tx = m.prepare(&later, &fork, &mut never).unwrap().unwrap();
        assert_eq!(tx.ops, [trim("c42"), trim("c3")]);
    }

    /// B6 (plan 4C Decision 24): the ids nested in a restored item are remapped and checked
    /// like any other: its own id follows the remap, its effect and link ids are judged.
    #[test]
    fn recovery_remaps_nested_ids() {
        let r = Rational::from_int;
        let mut m = Remap::default();
        m.learn(
            &record(vec![], &[("$c", "c10")]),
            &BTreeMap::from([("$c".into(), "c42".into())]),
        );
        let mut item = fin_core::Item::clip("c10", "a1", r(0), r(0), r(1));
        item.common.effects.push(fin_core::Effect {
            id: "fx5".into(),
            kind: "blur".into(),
            enabled: true,
            params: Default::default(),
        });
        item.common.link = Some("lk2".into());
        let restore: Op = fin_core::ItemRestore {
            item,
            track: "t1".into(),
            mode: fin_core::DeleteMode::Lift,
        }
        .into();
        let mut asked = vec![];
        let mut elsewhere = |id: &str| -> Result<bool, StoreError> {
            asked.push(id.to_string());
            Ok(id == "lk2")
        };
        let fork = BTreeSet::from(["a1".to_string(), "t1".to_string()]);
        let rec = record(vec![restore.clone()], &[]);
        let c = m.prepare(&rec, &fork, &mut elsewhere).unwrap().unwrap_err();
        assert_eq!(c.reason, "ID_COLLISION");
        assert_eq!(asked, ["fx5", "lk2"]);
        let fork = BTreeSet::from(["a1".into(), "t1".into(), "fx5".into(), "lk2".into()]);
        let tx = m.prepare(&rec, &fork, &mut never_asked).unwrap().unwrap();
        let Op::ItemRestore(got) = &tx.ops[0] else {
            panic!("{:?}", tx.ops)
        };
        assert_eq!(got.item.ids(), ["c42", "a1", "fx5", "lk2"]);
    }

    fn never_asked(_: &str) -> Result<bool, StoreError> {
        panic!("not asked")
    }

    #[test]
    fn literal_bound_elsewhere_is_an_id_collision() {
        let m = Remap::default();
        let rec = record(vec![trim("c3"), trim("c7")], &[]);
        let mut asked = vec![];
        let mut elsewhere = |id: &str| -> Result<bool, StoreError> {
            asked.push(id.to_string());
            Ok(id == "c7")
        };
        let c = m
            .prepare(&rec, &BTreeSet::new(), &mut elsewhere)
            .unwrap()
            .unwrap_err();
        assert_eq!((c.reason.as_str(), c.op_index), ("ID_COLLISION", 1));
        assert_eq!(asked, ["c3", "c7"]);
    }
}
