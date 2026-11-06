//! Fresh ids for recovered records (plan 4B Decision 27): each record of a group is re-run with
//! a `$placeholder` for every id its original bound, so the store's id counters hand out new
//! ones, and later records of the group that name an old id literally (their inverse-derived
//! ops included: an undo's `clip.remove`, a restore) name the new one.
//!
//! A literal id that is neither remapped nor present at the group's fork state is checked
//! against the database: when a record off the fork's lineage binds it (another server's
//! entity under the same id, Decision 33), the group is not created (`ID_COLLISION`).

use std::collections::{BTreeMap, BTreeSet};

use fin_core::{Conflict, Project, Record, Tx};

use crate::StoreError;

/// The ids a group's records bound, old to new.
#[derive(Debug, Clone, Default)]
pub(crate) struct Remap {
    map: BTreeMap<String, String>,
}

/// Every entity id in `project`: the timeline, its tracks and clips, the assets.
pub(crate) fn entity_ids(project: &Project) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    out.insert(project.timeline.id.clone());
    for t in &project.timeline.tracks {
        out.insert(t.id.clone());
        out.extend(t.items.iter().map(|c| c.id.clone()));
    }
    out.extend(project.assets.keys().cloned());
    out
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
        Op::ClipTrim {
            clip: clip.into(),
            edge: Edge::End,
            delta: Rational::from_int(-1),
        }
    }

    #[test]
    fn bound_ids_become_placeholders_and_later_literals_follow_the_remap() {
        let mut m = Remap::default();
        let first = record(
            vec![Op::ClipRemove {
                id: "c10".into(),
                ripple: true,
            }],
            &[("$c", "c10")],
        );
        let mut never = |_: &str| -> Result<bool, StoreError> { panic!("not asked") };
        let tx = m
            .prepare(&first, &BTreeSet::new(), &mut never)
            .unwrap()
            .unwrap();
        assert_eq!(
            tx.ops,
            [Op::ClipRemove {
                id: "$c".into(),
                ripple: true
            }]
        );
        m.learn(&first, &BTreeMap::from([("$c".into(), "c42".into())]));
        let later = record(vec![trim("c10"), trim("c3")], &[]);
        let fork = BTreeSet::from(["c3".to_string()]);
        let tx = m.prepare(&later, &fork, &mut never).unwrap().unwrap();
        assert_eq!(tx.ops, [trim("c42"), trim("c3")]);
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
