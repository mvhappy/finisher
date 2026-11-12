//! A write's `resolved` echo when its answer is a commit this attempt did not plan exactly
//! (review #48): a keyed retry answered from the log (its plan applied the command a second
//! time, to the state the commit made), a lost answer found by `tx.get`, or a commit the server
//! rebased over concurrent ones. Such a plan's values are speculative, so every value that
//! depends on state comes from the commit instead: from its ops (positions, deltas, modes,
//! splits, new tracks), its diff (what a delete took) or the state right after it (an edge, a
//! source in), which is known only when the head of the commit's branch is that commit. A
//! value the commit cannot tell is `null`, never the speculative one; so is every value of a
//! command this module has no rule for, unless it is the input as given.

use fin_core::layout::style_key;
use fin_core::{ChangeKind, CommittedTx, Edge, EntityKind, ItemBody, Op, Project};
use serde_json::{Map, Value, json};

use super::builder::suffixed;
use super::item::edge_at;
use crate::addr;

/// Whether the planned echo is the commit's: the commit applied exactly the ops this attempt
/// sent (`sent`, in the canonical form the server stores, `fin_core::canonicalize`), straight
/// onto the rev its plan read. The caller checks that the commit is on the plan's branch.
pub(super) fn planned_is_committed(sent: &[Op], base_rev: u64, committed: &CommittedTx) -> bool {
    committed.rev == base_rev + 1 && committed.tx.ops == sent
}

/// Commands whose echo is the input as given, or what the unchanged project says of it (a
/// rate, a probe, an asset): the plan's echo holds, even planned a second time.
const INPUT_ONLY: &[&str] = &[
    "asset.add",
    "asset.move",
    "asset.remove",
    "bin.add",
    "bin.move",
    "bin.remove",
    "effect.add",
    "effect.move",
    "effect.remove",
    "marker.add",
    "marker.set",
    "marker.remove",
    "param.set",
    "timeline.add",
    "timeline.set",
    "timeline.remove",
    "track.move",
    "track.set",
    "caption.split",
    "caption.style",
    "style.remove",
    "item.link",
    "transition.remove",
    "fonts.pin",
    "fonts.unpin",
];

/// Replaces the state-dependent values of `resolved` (one step's object, or a batch's
/// `[{line, cmd, resolved}]`; `inputs` holds each step's `{cmd, args}`) with the commit's.
/// `after`: the state right after the commit, when known.
pub(super) fn from_commit(
    resolved: &mut Value,
    inputs: &[Value],
    committed: &CommittedTx,
    after: Option<&Project>,
) {
    let mut ctx = Ctx {
        committed,
        ops: committed
            .tx
            .ops
            .iter()
            .map(|op| serde_json::to_value(op).unwrap_or(Value::Null))
            .collect(),
        used: vec![false; committed.tx.ops.len()],
        after,
        single: !resolved.is_array(),
    };
    match resolved {
        Value::Array(lines) => {
            for (line, input) in lines.iter_mut().zip(inputs) {
                let n = line.get("line").and_then(Value::as_u64).unwrap_or(0);
                if let Some(r) = line.get_mut("resolved") {
                    ctx.step(r, input, &n.to_string());
                }
            }
        }
        single => {
            if let Some(input) = inputs.first() {
                ctx.step(single, input, "");
            }
        }
    }
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
const FROM_OP: [&str; 7] = ["at", "track", "delta", "edge", "mode", "value", "interp"];

/// Whether `id` is a split placeholder of the step with batch `suffix` (`$split`, `$split2`;
/// on line 3 `$split3`, `$split2_3`).
fn split_of(id: &str, suffix: &str) -> bool {
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    let Some(rest) = id.strip_prefix("$split") else {
        return false;
    };
    if suffix.is_empty() {
        return digits(rest);
    }
    rest == suffix
        || rest
            .strip_suffix(&format!("_{suffix}"))
            .is_some_and(|d| !d.is_empty() && digits(d))
}

/// Sets `key` of `map` when the echo has it (`None` is `null`).
fn set(map: &mut Map<String, Value>, key: &str, v: Option<Value>) {
    if let Some(slot) = map.get_mut(key) {
        *slot = v.unwrap_or(Value::Null);
    }
}

/// Sets `key` to `v`, or removes it (as the plan's `merge_echo` leaves out empty values).
fn put(map: &mut Map<String, Value>, key: &str, v: Option<Value>) {
    match v {
        Some(v) => map.insert(key.to_string(), v),
        None => map.remove(key),
    };
}

struct Ctx<'a> {
    committed: &'a CommittedTx,
    /// The committed ops as JSON.
    ops: Vec<Value>,
    used: Vec<bool>,
    after: Option<&'a Project>,
    /// One command, not a batch: the state after the commit is the state after its step.
    single: bool,
}

impl<'a> Ctx<'a> {
    /// The state right after op `i`, when known: after the commit, when `i` is its step's
    /// last op (one command, or the batch's last op).
    fn after(&self, i: Option<usize>) -> Option<&'a Project> {
        self.after
            .filter(|_| self.single || i.is_some_and(|i| i + 1 == self.ops.len()))
    }

    /// The id a reference names: a placeholder through the commit's bindings.
    fn id<'s>(&'s self, reference: &'s str) -> &'s str {
        self.committed
            .bindings
            .get(reference)
            .map_or(reference, String::as_str)
    }

    /// The first unused committed op named `name` whose fields match `fields`.
    fn find(&mut self, name: &str, fields: &[(&str, &Value)]) -> Option<usize> {
        let ops = &self.committed.tx.ops;
        let i = (0..ops.len()).find(|&i| {
            !self.used[i]
                && ops[i].name() == name
                && fields.iter().all(|(k, v)| self.ops[i].get(*k) == Some(*v))
        })?;
        self.used[i] = true;
        Some(i)
    }

    fn step(&mut self, r: &mut Value, input: &Value, suffix: &str) {
        let cmd = input["cmd"].as_str().unwrap_or_default();
        let args = &input["args"];
        if cmd == "item.split" && r.is_null() {
            // A retry cannot plan the split again (the item now ends at the cut).
            *r = json!({ "item": args["item"], "at": null });
        }
        let Some(map) = r.as_object_mut() else {
            return;
        };
        match cmd {
            "clip.insert" => {
                let list = args["noAudio"] != true;
                self.insert(map, "$clip", suffix, list);
            }
            "image.insert" => self.insert(map, "$image", suffix, false),
            "graphic.insert" => self.insert(map, "$graphic", suffix, false),
            "caption.import" => {
                let at = (self.committed.tx.ops.iter())
                    .filter_map(|op| match op {
                        Op::ItemInsert(i) if matches!(i.body, ItemBody::Caption { .. }) => {
                            Some(i.at)
                        }
                        _ => None,
                    })
                    .min();
                set(map, "at", at.map(|a| json!(a)));
            }
            "item.split" => self.split(map, args),
            "track.add" => self.track_add(map, suffix),
            "track.remove" => {
                let i = self.find("track.remove", &[]);
                set(map, "items", i.map(|i| json!(self.removed(i).len())));
            }
            "caption.set-text" | "caption.merge" => {
                let item = args["item"].as_str().unwrap_or_default();
                let op = if cmd == "caption.merge" {
                    cmd
                } else {
                    "caption.setText"
                };
                let i = self.find(op, &[("item", &args["item"])]);
                let words = self.after(i).and_then(|p| {
                    let (_, _, it) = addr::item(p, self.id(item)).ok()?;
                    match &it.body {
                        ItemBody::Caption { block } => Some(json!(block.words.len())),
                        _ => None,
                    }
                });
                set(map, "words", words);
            }
            "transition.set" => self.transitions(map),
            "style.add" | "style.set" => {
                let style = map.get("style").and_then(Value::as_str).unwrap_or_default();
                let font = self.after(None).and_then(|p| {
                    fin_core::captions::style_of(p, self.id(style)).map(|s| json!(style_key(s)))
                });
                set(map, "font", font);
            }
            "keyframe.set" | "keyframe.remove" => self.keyframe(map, cmd, args),
            "fonts.repin" => set(map, "from", None),
            _ if op_of(cmd).is_some() => self.item_op(map, cmd),
            _ if INPUT_ONLY.contains(&cmd) => {}
            // No rule: only what the input says survives.
            _ => {
                for (k, v) in map.iter_mut() {
                    if args.get(k) != Some(v) {
                        *v = Value::Null;
                    }
                }
            }
        }
    }

    /// An insert (`$clip`, `$image`, `$graphic` with the step's suffix): where the commit
    /// placed it, the splits it made first (`splits` on a clip's linked-audio path, else
    /// `split`), and a clip's audio leg and link.
    fn insert(&mut self, map: &mut Map<String, Value>, base: &str, suffix: &str, list: bool) {
        let id = json!(suffixed(base, suffix));
        let found = self.find("item.insert", &[("id", &id)]);
        let placed = found.and_then(|i| match &self.committed.tx.ops[i] {
            Op::ItemInsert(i) => Some(i),
            _ => None,
        });
        set(map, "at", placed.map(|i| json!(i.at)));
        set(map, "duration", placed.map(|i| json!(i.duration)));
        let source = placed.and_then(|i| match i.body {
            ItemBody::Clip { source_in, .. } => Some((source_in, i.duration)),
            _ => None,
        });
        set(map, "in", source.map(|(s, _)| json!(s)));
        let out = source.and_then(|(s, d)| s.checked_add(d).ok());
        set(map, "out", out.map(|o| json!(o)));
        let splits = self.splits(suffix);
        if list {
            map.remove("split");
            put(map, "splits", (!splits.is_empty()).then(|| json!(splits)));
        } else {
            map.remove("splits");
            put(map, "split", splits.into_iter().next());
        }
        if base == "$clip" {
            let audio = json!(suffixed("$audio", suffix));
            let leg = self.find("item.insert", &[("id", &audio)]);
            put(
                map,
                "audio",
                leg.map(|i| json!({ "track": self.ops[i]["track"], "id": audio })),
            );
            let link = json!(suffixed("$link", suffix));
            let linked = self.find("item.link", &[("id", &link)]);
            put(map, "link", linked.map(|_| link));
        }
    }

    /// The step's `item.split` ops as the plan echoes them: `{item, offset, id}`.
    fn splits(&mut self, suffix: &str) -> Vec<Value> {
        let mut out = Vec::new();
        for (i, op) in self.committed.tx.ops.iter().enumerate() {
            if let Op::ItemSplit(s) = op
                && !self.used[i]
                && split_of(&s.id, suffix)
            {
                self.used[i] = true;
                out.push(json!({ "item": s.item, "offset": s.offset, "id": s.id }));
            }
        }
        out
    }

    /// `item split`: the cuts and the link of the right parts from the ops; the time from the
    /// item's start after the commit (the left part keeps it) and the first cut's offset.
    fn split(&mut self, map: &mut Map<String, Value>, args: &Value) {
        let splits = self.splits("");
        let item = args["item"].as_str().unwrap_or_default();
        let at = self.after(None).and_then(|p| {
            let (_, _, it) = addr::item(p, self.id(item)).ok()?;
            let offset: fin_core::Rational =
                serde_json::from_value(splits.first()?["offset"].clone()).ok()?;
            it.start.checked_add(offset).ok()
        });
        map.insert("at".into(), at.map_or(Value::Null, |a| json!(a)));
        let link = self.find("item.link", &[("id", &json!("$link"))]);
        put(map, "splits", Some(json!(splits)));
        put(map, "link", link.map(|_| json!("$link")));
    }

    /// `track add`: the track the commit added, read after it; else what its op says.
    fn track_add(&mut self, map: &mut Map<String, Value>, suffix: &str) {
        let id = suffixed("$track", suffix);
        let Some(i) = self.find("track.add", &[("id", &json!(id))]) else {
            set(map, "index", None);
            set(map, "name", None);
            return;
        };
        let Op::TrackAdd(op) = &self.committed.tx.ops[i] else {
            return;
        };
        let found = self.after(Some(i)).and_then(|p| {
            let (tl, t) = addr::track(p, self.id(&id)).ok()?;
            let index = tl.tracks.iter().position(|x| x.id == t.id)?;
            Some((json!(index), json!(t.name)))
        });
        let (index, name) = found.unwrap_or((json!(op.index), json!(op.name)));
        set(map, "index", Some(index));
        set(map, "name", Some(name));
    }

    /// `transition set`: each transition's id from its committed op (a literal id is one the
    /// commit updated; a placeholder, one it added).
    fn transitions(&mut self, map: &mut Map<String, Value>) {
        let ids: Vec<String> = (self.committed.tx.ops.iter())
            .filter_map(|op| match op {
                Op::TransitionSet(t) => Some(t.id.clone()),
                _ => None,
            })
            .collect();
        let count = map
            .get("transitions")
            .and_then(Value::as_array)
            .map(Vec::len);
        if count.is_some_and(|n| n != ids.len()) {
            map.insert("transitions".into(), Value::Null);
        }
        let Some(list) = map.get_mut("transitions").and_then(Value::as_array_mut) else {
            return;
        };
        for (t, id) in list.iter_mut().zip(ids) {
            let Some(t) = t.as_object_mut() else { continue };
            put(t, "updated", (!id.starts_with('$')).then_some(json!(true)));
            t.insert("id".into(), json!(id));
        }
    }

    /// A keyframe write: value and interpolation from its op; its time from the op, else the
    /// input (`--at`), else unknown. The key is `keyframe::echo_key`'s.
    fn keyframe(&mut self, map: &mut Map<String, Value>, cmd: &str, args: &Value) {
        let (item, path) = (args["item"].clone(), args["path"].clone());
        let i = self.find(cmd, &[("item", &item), ("path", &path)]);
        let op = i.map(|i| self.ops[i].clone()).unwrap_or(Value::Null);
        for k in ["value", "interp"] {
            if let (Some(slot), Some(v)) = (map.get_mut(k), op.get(k)) {
                *slot = v.clone();
            }
        }
        let at = op.get("at").filter(|a| !a.is_null()).cloned();
        if at.is_some() || args.get("at").is_none() {
            set(map, "at", at);
        }
    }

    fn item_op(&mut self, map: &mut Map<String, Value>, cmd: &str) {
        let Some((name, key)) = op_of(cmd) else {
            return;
        };
        let target = map.get(key).cloned().unwrap_or(Value::Null);
        let index = self.find(name, &[("item", &target)]);
        if let Some(i) = index {
            for k in FROM_OP {
                if let (Some(slot), Some(v)) = (map.get_mut(k), self.ops[i].get(k)) {
                    *slot = v.clone();
                }
            }
        }
        let Some(item) = target.as_str() else {
            for k in ["edgeAt", "start", "sourceIn", "items"] {
                set(map, k, None);
            }
            return;
        };
        let id = self.id(item).to_string();
        if name == "item.delete" {
            let items = index.map(|i| {
                let mut all = vec![item.to_string()];
                all.extend(self.removed(i).into_iter().filter(|x| x != item));
                json!(all)
            });
            set(map, "items", items);
            return;
        }
        let after = self.after(index);
        let edge =
            index.and_then(|i| serde_json::from_value::<Edge>(self.ops[i]["edge"].clone()).ok());
        let edge_v = |e: Edge| after.and_then(|p| edge_at(p, &id, e).ok());
        set(map, "edgeAt", edge.and_then(edge_v));
        set(map, "start", edge_v(Edge::Start));
        set(map, "sourceIn", after.and_then(|p| source_in(p, &id)));
        set(map, "items", after.and_then(|p| members(p, &id)));
    }

    /// The items op `i` of the commit removed.
    fn removed(&self, i: usize) -> Vec<String> {
        (self.committed.diff.changes.iter())
            .filter(|c| {
                c.op as usize == i && c.kind == EntityKind::Item && c.change == ChangeKind::Removed
            })
            .map(|c| c.id.clone())
            .collect()
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
