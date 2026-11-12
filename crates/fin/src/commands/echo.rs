//! A write's `resolved` echo when its answer is a commit this attempt did not plan exactly
//! (review #48): a keyed retry answered from the log (its plan applied the command a second
//! time, to the state the commit made), a lost answer found by `tx.get`, or a commit the server
//! rebased over concurrent ones. Such a plan's values are speculative, so every value that
//! depends on state comes from the commit instead: from its ops (positions, deltas, modes),
//! its diff (what a delete took) or the state right after it (an edge, a source in), which is
//! known only when the branch's head is that commit. A value the commit cannot tell is `null`,
//! never the speculative one.

use fin_core::{ChangeKind, CommittedTx, Edge, EntityKind, ItemBody, Op, Project};
use serde_json::{Value, json};

use super::item::edge_at;
use crate::addr;

/// Whether the planned echo is the commit's: the commit applied exactly the ops this attempt
/// sent, straight onto the rev its plan read.
pub(super) fn planned_is_committed(sent: &[Op], base_rev: u64, committed: &CommittedTx) -> bool {
    committed.rev == base_rev + 1 && committed.tx.ops == sent
}

/// Replaces the state-dependent values of `resolved` (one step's object, or a batch's
/// `[{line, cmd, resolved}]`; `cmds` names each step) with the commit's. `after`: the state
/// right after the commit, when known.
pub(super) fn from_commit(
    resolved: &mut Value,
    cmds: &[String],
    committed: &CommittedTx,
    after: Option<&Project>,
) {
    let mut used = vec![false; committed.tx.ops.len()];
    let ctx = Ctx { committed, after };
    match resolved {
        Value::Array(lines) => {
            for line in lines.iter_mut() {
                let Some(n) = line.get("line").and_then(Value::as_u64) else {
                    continue;
                };
                let cmd = line.get("cmd").and_then(Value::as_str).map(str::to_string);
                if let (Some(r), Some(cmd)) = (line.get_mut("resolved"), cmd) {
                    ctx.step(r, &cmd, &format!("$clip{n}"), &mut used);
                }
            }
        }
        single => {
            if let Some(cmd) = cmds.first() {
                ctx.step(single, cmd, "$clip", &mut used);
            }
        }
    }
}

/// Each `clip.insert` echo gets the clip its commit placed, found among the committed ops by
/// its placeholder (`$clip`, or `$clipN` on batch line N), not by where a plan put it: the
/// commit may hold pins, splits or audio ops a plan does not, or lack some it has.
fn patch_insert(r: &mut Value, ops: &[Op], placeholder: &str) {
    let clip = ops.iter().find_map(|op| match op {
        Op::ItemInsert(i) => match &i.body {
            ItemBody::Clip { source_in, .. } if i.id == placeholder => {
                Some((i.at, i.duration, *source_in))
            }
            _ => None,
        },
        _ => None,
    });
    let (Some(map), Some((at, duration, source_in))) = (r.as_object_mut(), clip) else {
        return;
    };
    map.insert("at".to_string(), json!(at));
    map.insert("in".to_string(), json!(source_in));
    map.insert("duration".to_string(), json!(duration));
    map.insert(
        "out".to_string(),
        source_in
            .checked_add(duration)
            .map_or(Value::Null, |out| json!(out)),
    );
}

/// The op a step's echo describes, by command; and the echo key naming its item.
fn op_of(cmd: &str) -> Option<(&'static str, &'static str)> {
    Some(match cmd {
        "item.trim" => ("item.trim", "item"),
        "clip.trim" => ("item.trim", "clip"),
        "item.slip" => ("item.slip", "item"),
        "item.slide" => ("item.slide", "item"),
        "item.move" => ("item.move", "item"),
        "item.delete" => ("item.delete", "item"),
        "clip.delete" => ("item.delete", "clip"),
        _ => return None,
    })
}

/// Echo keys an op states itself (absolute or as sent): copied from the committed op.
const FROM_OP: [&str; 5] = ["at", "track", "delta", "edge", "mode"];

struct Ctx<'a> {
    committed: &'a CommittedTx,
    after: Option<&'a Project>,
}

impl Ctx<'_> {
    fn step(&self, r: &mut Value, cmd: &str, placeholder: &str, used: &mut [bool]) {
        let ops = &self.committed.tx.ops;
        if cmd == "clip.insert" {
            patch_insert(r, ops, placeholder);
            return;
        }
        let Some((name, key)) = op_of(cmd) else {
            return;
        };
        let Some(map) = r.as_object_mut() else {
            return;
        };
        let target = map.get(key).cloned().unwrap_or(Value::Null);
        let found = ops.iter().enumerate().find_map(|(i, op)| {
            let v = serde_json::to_value(op).ok()?;
            (!used[i] && op.name() == name && v.get("item") == Some(&target)).then_some((i, v))
        });
        if let Some((i, _)) = found {
            used[i] = true;
        }
        if let Some((_, op)) = &found {
            for k in FROM_OP {
                if let (Some(slot), Some(v)) = (map.get_mut(k), op.get(k)) {
                    *slot = v.clone();
                }
            }
        }
        let id = target
            .as_str()
            .map(|t| self.committed.bindings.get(t).map_or(t, String::as_str));
        let index = found.as_ref().map(|(i, _)| *i);
        // The state right after the commit is this step's only when its op is the last.
        let after = self
            .after
            .filter(|_| index.is_some_and(|i| i + 1 == ops.len()));
        let edge = found
            .as_ref()
            .and_then(|(_, op)| serde_json::from_value::<Edge>(op.get("edge")?.clone()).ok());
        let mut set = |k: &str, v: Option<Value>| {
            if let Some(slot) = map.get_mut(k) {
                *slot = v.unwrap_or(Value::Null);
            }
        };
        match (name, id) {
            ("item.delete", Some(id)) => set("items", index.map(|i| self.removed(i, id))),
            (_, Some(id)) => {
                let edge_v = |e: Edge| after.and_then(|p| edge_at(p, id, e).ok());
                set("edgeAt", edge.and_then(edge_v));
                set("start", edge_v(Edge::Start));
                set("sourceIn", after.and_then(|p| source_in(p, id)));
                set("items", after.and_then(|p| members(p, id)));
            }
            _ => {
                for k in ["edgeAt", "start", "sourceIn", "items"] {
                    set(k, None);
                }
            }
        }
    }

    /// The items op `i` of the commit removed (`item` first): a delete's link group.
    fn removed(&self, i: usize, item: &str) -> Value {
        let mut items = vec![item.to_string()];
        items.extend(
            self.committed
                .diff
                .changes
                .iter()
                .filter(|c| {
                    c.op as usize == i
                        && c.kind == EntityKind::Item
                        && c.change == ChangeKind::Removed
                        && c.id != item
                })
                .map(|c| c.id.clone()),
        );
        json!(items)
    }
}

fn source_in(project: &Project, item: &str) -> Option<Value> {
    let (_, _, it) = addr::item(project, item).ok()?;
    match &it.body {
        ItemBody::Clip { source_in, .. } | ItemBody::Nested { source_in, .. } => {
            Some(json!(source_in))
        }
        _ => None,
    }
}

fn members(project: &Project, item: &str) -> Option<Value> {
    let (_, _, it) = addr::item(project, item).ok()?;
    Some(json!(addr::link_members(project, it)))
}
