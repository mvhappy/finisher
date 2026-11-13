//! Scoped reads of stored state (review batch L): `fin track ls`, `fin asset ls`, `fin bin ls`
//! and `fin effect ls`. Each lists one kind of entity with its stable id, stored values and
//! relationships, a page at a time: `{<key>, total, offset, next}`, where `next` is the exact
//! command for the following page (`null` on the last). `fin status` and `fin view` stay
//! summaries and point here for the rest.

use clap::Args;
use fin_core::{CoreError, Project, Timeline, TrackKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Globals, target_branch};
use crate::backend::Backend;
use crate::output::usage;
use crate::{addr, follow};

/// Rows a page holds without `--limit`.
pub const DEFAULT_LIMIT: usize = 50;
/// The largest `--limit`.
pub const MAX_LIMIT: usize = 500;

/// Which page: `--offset <n>` rows skipped, at most `--limit <n>` (default 50, at most 500).
#[derive(Debug, Clone, Default, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Paging {
    /// Rows to skip (default 0).
    #[arg(long, value_name = "n")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// Rows to list (default 50, at most 500).
    #[arg(long, value_name = "n")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// List tracks: id, name, kind, timeline, index, items, end and flags.
#[derive(Debug, Clone, Default, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrackLsArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub page: Paging,
}

/// List assets: id, kind, path, bin and duration.
#[derive(Debug, Clone, Default, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssetLsArgs {
    /// Only the assets filed in this bin (not its sub-bins).
    #[arg(long, value_name = "bN")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub page: Paging,
}

/// List bins as a tree: id, name, parent, path and how many bins and assets each holds.
#[derive(Debug, Clone, Default, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BinLsArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub page: Paging,
}

/// List stored effects, disabled ones included: item, id, kind, enabled, index, params and
/// animated params.
#[derive(Debug, Clone, Default, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectLsArgs {
    /// Only this item's effects (default: every item's).
    #[arg(value_name = "i")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub page: Paging,
}

/// Lists a summary (`fin status`) shows at most: the 20 of every other capped list.
pub const SUMMARY_CAP: usize = 20;

/// Cuts `doc[key]` to [`SUMMARY_CAP`] entries; when it was longer, `<key>Total` counts them all
/// and, when a command lists the rest, `<key>Next` names it exactly ([`follow::cmd`]).
pub fn cut(doc: &mut Value, key: &str, next: Option<&str>) {
    if let Some(list) = doc.get_mut(key).and_then(Value::as_array_mut)
        && list.len() > SUMMARY_CAP
    {
        let total = list.len();
        list.truncate(SUMMARY_CAP);
        doc[format!("{key}Total")] = json!(total);
        if let Some(rest) = next {
            doc[format!("{key}Next")] = json!(follow::cmd(rest));
        }
    }
}

/// A read named where a write was expected (a `batch` line, the edit path).
pub fn not_a_write(name: &str) -> CoreError {
    usage(
        format!("{name} is not a write"),
        json!({ "command": name }),
        format!("run fin {} on its own", name.replace('.', " ")),
    )
}

/// One page of `rows` under `key`, with `total`, `offset` and the exact `next` command (`cmd`
/// with `--offset`/`--limit` appended).
fn page(key: &str, rows: Vec<Value>, p: &Paging, cmd: &str) -> Result<Value, CoreError> {
    let limit = p.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(usage(
            format!("--limit {limit} is out of range"),
            json!({ "field": "limit", "min": 1, "max": MAX_LIMIT }),
            format!("pass --limit from 1 to {MAX_LIMIT}"),
        ));
    }
    let offset = p.offset.unwrap_or(0);
    let total = rows.len();
    let end = offset.saturating_add(limit).min(total);
    let shown: Vec<Value> = rows.into_iter().skip(offset).take(limit).collect();
    let next = (end < total).then(|| {
        let mut c = format!("{cmd} --offset {end}");
        if p.limit.is_some() {
            c.push_str(&format!(" --limit {limit}"));
        }
        follow::cmd(&c)
    });
    Ok(json!({ key: shown, "total": total, "offset": offset, "next": next }))
}

fn snapshot(backend: &dyn Backend, g: &Globals) -> Result<(u64, Project), CoreError> {
    let snap = backend.snapshot(&target_branch(backend, g)?)?;
    Ok((snap.rev, snap.project))
}

/// `with_rev`: the page plus the snapshot's `rev`.
fn with_rev(mut doc: Value, rev: u64) -> Value {
    doc["rev"] = json!(rev);
    doc
}

/// The global `--timeline`'s timelines (all of them without it).
fn timelines<'p>(p: &'p Project, g: &Globals) -> Result<Vec<&'p Timeline>, CoreError> {
    Ok(match g.timeline.as_deref() {
        Some(id) => vec![addr::timeline(p, Some(id))?],
        None => p.timelines.iter().collect(),
    })
}

/// `fin track ls`: the tracks of `--timeline` (default every timeline), in compositing order.
pub fn tracks(backend: &dyn Backend, g: &Globals, a: &TrackLsArgs) -> Result<Value, CoreError> {
    let (rev, p) = snapshot(backend, g)?;
    let mut rows = Vec::new();
    for tl in timelines(&p, g)? {
        for (index, t) in tl.tracks.iter().enumerate() {
            let kind = match t.kind {
                TrackKind::Video => "video",
                TrackKind::Audio => "audio",
                TrackKind::Caption => "caption",
            };
            rows.push(json!({
                "id": t.id, "name": t.name, "kind": kind, "timeline": tl.id, "index": index,
                "items": t.items.len(), "end": addr::track_end(&p, &t.id)?,
                "mute": t.mute, "solo": t.solo, "lock": t.lock, "gain": t.gain,
            }));
        }
    }
    let cmd = match &g.timeline {
        Some(tl) => format!("track ls --timeline {tl}"),
        None => "track ls".to_string(),
    };
    Ok(with_rev(page("tracks", rows, &a.page, &cmd)?, rev))
}

/// `fin asset ls`: assets by id, with the path, bin and length each holds.
pub fn assets(backend: &dyn Backend, g: &Globals, a: &AssetLsArgs) -> Result<Value, CoreError> {
    let (rev, p) = snapshot(backend, g)?;
    if let Some(b) = &a.bin
        && !p.bins.contains_key(b)
    {
        let all: Vec<String> = p.bins.keys().cloned().collect();
        return Err(addr::not_found("bin", b, all)
            .with_fix(format!("`{}` lists the bins", follow::cmd("bin ls"))));
    }
    let rows: Vec<Value> = p
        .assets
        .values()
        .filter(|x| a.bin.is_none() || x.bin == a.bin)
        .map(|x| {
            let kind = serde_json::to_value(x.kind).unwrap_or(Value::Null);
            json!({ "id": x.id, "kind": kind, "path": x.path, "bin": x.bin,
                    "duration": x.duration })
        })
        .collect();
    let cmd = match &a.bin {
        Some(b) => format!("asset ls --bin {b}"),
        None => "asset ls".to_string(),
    };
    Ok(with_rev(page("assets", rows, &a.page, &cmd)?, rev))
}

/// The ids of `p`'s bins in tree order: each top-level bin (by id), then its children.
fn tree_order(p: &Project) -> Vec<(String, usize)> {
    fn walk(p: &Project, parent: Option<&str>, depth: usize, out: &mut Vec<(String, usize)>) {
        for b in p.bins.values().filter(|b| b.parent.as_deref() == parent) {
            out.push((b.id.clone(), depth));
            walk(p, Some(&b.id), depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(p, None, 0, &mut out);
    out
}

/// `fin bin ls`: every bin in tree order with its `path` of names (`Originals/Stills`).
pub fn bins(backend: &dyn Backend, g: &Globals, a: &BinLsArgs) -> Result<Value, CoreError> {
    let (rev, p) = snapshot(backend, g)?;
    let path = |id: &str| {
        let mut names = Vec::new();
        let mut at = p.bins.get(id);
        while let Some(b) = at {
            names.push(b.name.clone());
            at = b.parent.as_deref().and_then(|x| p.bins.get(x));
        }
        names.reverse();
        names.join("/")
    };
    let rows: Vec<Value> = tree_order(&p)
        .into_iter()
        .map(|(id, depth)| {
            let b = &p.bins[&id];
            let bins = p.bins.values().filter(|c| c.parent.as_deref() == Some(&id));
            let assets = p.assets.values().filter(|x| x.bin.as_deref() == Some(&id));
            json!({ "id": id, "name": b.name, "parent": b.parent, "path": path(&id),
                    "depth": depth, "bins": bins.count(), "assets": assets.count() })
        })
        .collect();
    let top = p.assets.values().filter(|x| x.bin.is_none()).count();
    let mut doc = page("bins", rows, &a.page, "bin ls")?;
    doc["topLevelAssets"] = json!(top);
    Ok(with_rev(doc, rev))
}

/// `fin effect ls [<i>]`: stored effects of `--timeline`'s items (default every timeline's), in
/// timeline, track and item order, then list order.
pub fn effects(backend: &dyn Backend, g: &Globals, a: &EffectLsArgs) -> Result<Value, CoreError> {
    let (rev, p) = snapshot(backend, g)?;
    let only = a.item.as_deref().map(|i| addr::item(&p, i)).transpose()?;
    let mut rows = Vec::new();
    for tl in timelines(&p, g)? {
        for t in &tl.tracks {
            for it in &t.items {
                if only.is_some_and(|(_, _, o)| o.id != it.id) {
                    continue;
                }
                for (index, e) in it.common.effects.iter().enumerate() {
                    let prefix = format!("effects.{}.", e.id);
                    let animated: Vec<&str> = it
                        .common
                        .keyframes
                        .keys()
                        .filter_map(|k| k.strip_prefix(&prefix))
                        .collect();
                    rows.push(json!({
                        "item": it.id, "effect": e.id, "kind": e.kind, "enabled": e.enabled,
                        "index": index, "params": e.params, "animated": animated,
                    }));
                }
            }
        }
    }
    let mut cmd = match &a.item {
        Some(i) => format!("effect ls {}", follow::quote(i)),
        None => "effect ls".to_string(),
    };
    if let Some(tl) = &g.timeline {
        cmd.push_str(&format!(" --timeline {}", follow::quote(tl)));
    }
    Ok(with_rev(page("effects", rows, &a.page, &cmd)?, rev))
}
