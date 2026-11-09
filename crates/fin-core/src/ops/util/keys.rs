//! Stable keyframe ids (sub-project 4 spec §5.12): the `"<path>#<key>"` parameter a key is
//! judged by, `KEYFRAME_EXISTS`, and the canonical form of a time-form `keyframe.set` or
//! `keyframe.remove` ([`key_form`]).
//!
//! A client may name a key by its time (`at` without `key`). Where a transaction is first
//! applied (any mode but replay), the applier ([`crate::apply::Applier`]) replaces each such
//! op by its key form
//! before it runs: the key at exactly `at` (named by the placeholder that bound it, when an
//! earlier op of the transaction made it), or, for a set with no key there, an insert under
//! the generated placeholder `$kf<opIndex>`. The committed intent holds what ran, so every
//! stored op names its key and a rebase carries the key, not its time. Replay refuses the time
//! form ([`time_form`]).

use serde_json::json;

use crate::error::CoreError;
use crate::model::Project;
use crate::ops::util::item_index;
use crate::ops::util::paths::resolve_path;
use crate::ops::{Bindings, KeyframeRemove, KeyframeSet, Op};
use crate::time::Rational;

/// The parameter one key is recorded and judged under: `<path>#<key>`.
pub(crate) fn key_param(path: &str, key: &str) -> String {
    format!("{path}#{key}")
}

/// The entity a key of `path` on `item` is a parameter of: the item, or `<item>/<fx>` for an
/// effect path.
pub(crate) fn key_entity(item: &str, path: &str) -> String {
    crate::ops::param_set::entity(item, path).1
}

/// `KEYFRAME_EXISTS`: key `existing` of `path` already holds `at`, where `key` would go.
pub(crate) fn keyframe_exists(
    item: &str,
    path: &str,
    key: &str,
    at: Rational,
    existing: &str,
) -> CoreError {
    CoreError::new(
        "KEYFRAME_EXISTS",
        format!("key {existing} of {path} on item {item} is already at {at}"),
    )
    .with_details(json!({ "item": item, "path": path, "key": key, "at": at, "existing": existing }))
    .with_fix(format!("edit key {existing} instead, or use another time"))
}

/// `INVALID_OP` for a keyframe op that does not name its key as stored ops must (a time form
/// in replay), or that names both or neither of `key` and `at`.
pub(crate) fn time_form(op: &Op) -> CoreError {
    CoreError::new(
        "INVALID_OP",
        format!(
            "{} names its key by id (key) in history; clients name it by key or by time (at)",
            op.name()
        ),
    )
    .with_details(json!({ "op": op.name() }))
    .with_fix("give key (an id such as kf3) or, for a client edit, at")
}

/// Whether `op` is a client time-form keyframe op (no `key`).
pub(crate) fn is_time_form(op: &Op) -> bool {
    match op {
        Op::KeyframeSet(k) => k.key.is_none(),
        Op::KeyframeRemove(k) => k.key.is_none(),
        _ => false,
    }
}

/// The key form of the time-form op `op` (op `index` of its transaction) on `project`, as the
/// earlier ops left it with `bindings`. Errors as the op would: `NOT_FOUND` for its item or,
/// for a removal, the key; `INVALID_OP` for a removal that names both or neither of `key` and
/// `at`, or a set with neither.
pub(crate) fn key_form(
    project: &Project,
    bindings: &Bindings,
    op: &Op,
    index: usize,
) -> Result<Op, CoreError> {
    let (item, path, at) = match op {
        Op::KeyframeSet(k) => (&k.item, &k.path, k.at),
        Op::KeyframeRemove(k) => (&k.item, &k.path, k.at),
        _ => return Ok(op.clone()),
    };
    let Some(at) = at else {
        return Err(time_form(op));
    };
    let id = bindings.resolve(item)?;
    let (ti, ci) = item_index(project, &id)?;
    let real = resolve_path(bindings, path)?;
    let found = ti.get(project).items[ci]
        .common
        .keyframes
        .get(&real)
        .and_then(|l| l.iter().find(|x| x.at == at))
        .map(|x| {
            // A key an earlier op of this transaction made is named by its placeholder.
            bindings
                .map
                .iter()
                .find(|(_, v)| **v == x.id)
                .map_or_else(|| x.id.clone(), |(p, _)| p.clone())
        });
    Ok(match op {
        Op::KeyframeSet(k) => {
            let key = match found {
                Some(key) => key,
                None => fresh(bindings, index)?,
            };
            KeyframeSet {
                key: Some(key),
                ..(**k).clone()
            }
            .into()
        }
        Op::KeyframeRemove(k) => {
            let key = found.ok_or_else(|| {
                crate::ops::util::not_found("keyframe", &format!("{id} {real}@{at}"))
            })?;
            KeyframeRemove {
                key: Some(key),
                at: None,
                ..k.clone()
            }
            .into()
        }
        _ => op.clone(),
    })
}

/// Our own ops' diffs (`own`, one per op) with the key ids our transaction minted given back
/// as the placeholders that minted them (`rebase::footprint::unbind_own`): an insert's
/// `"<path>#kf9"` (bound where the tx was first applied) becomes `"<path>#$kf0"`, in every op's
/// changes, so a footprint never claims a key another record minted under that id.
pub(crate) fn unbind_keys(ops: &[Op], own: &mut [crate::tx::Diff]) {
    let mut bound: Vec<(String, String)> = Vec::new();
    for (op, d) in ops.iter().zip(own.iter()) {
        let (Op::KeyframeSet(_), Some(p)) = (op, op.allocates()) else {
            continue;
        };
        if bound.iter().any(|(_, q)| q == p) {
            continue;
        }
        let minted = d
            .changes
            .iter()
            .filter_map(|c| c.path.as_deref()?.rsplit_once('#'))
            .find(|(_, k)| !k.starts_with('$'));
        if let Some((_, k)) = minted {
            bound.push((k.to_string(), p.to_string()));
        }
    }
    if bound.is_empty() {
        return;
    }
    for d in own {
        for c in &mut d.changes {
            let Some((path, k)) = c.path.as_deref().and_then(|p| p.rsplit_once('#')) else {
                continue;
            };
            if let Some((_, p)) = bound.iter().find(|(id, _)| id == k) {
                c.path = Some(key_param(path, p));
            }
        }
    }
}

/// The placeholder `$kf<index>` for a new key, unless an earlier op bound it already
/// (`DUPLICATE_PLACEHOLDER`: it would name that op's entity instead).
fn fresh(bindings: &Bindings, index: usize) -> Result<String, CoreError> {
    let name = format!("$kf{index}");
    if bindings.map.contains_key(&name) {
        return Err(CoreError::new(
            "DUPLICATE_PLACEHOLDER",
            format!("placeholder {name} is bound twice: a new keyframe of op {index} takes it"),
        )
        .with_details(json!({ "placeholder": name }))
        .with_fix(format!("name your own placeholders other than {name}")));
    }
    Ok(name)
}
