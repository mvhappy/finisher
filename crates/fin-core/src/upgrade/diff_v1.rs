//! The stored-diff step v1 → v2 (plan 4C Decision 5). **Frozen** once 4C merges: it works on
//! `serde_json::Value` with literal field and op names, uses no current model types, and has a
//! golden test (`tests/upgrade_golden.rs`).
//!
//! A v1 diff has no `op` on its entries. Every v1 op recorded exactly one change, and each
//! clip op (`clip.insert`, `clip.trim`, `clip.delete`, `clip.remove`, `clip.restore`) exactly one
//! shift; `asset.add` and `asset.remove` recorded none (`ops/*` at `36fe7e0`). So the step gives
//! the changes the indexes of their ops in order, and the shifts the indexes of the clip ops in
//! order. A `clip.trim` change also gains the op's `edge`, and on the start edge the content
//! offset `by = −delta` (Decision 13).

use serde_json::{Value, json};

use crate::error::CoreError;
use crate::record::invalid_format;

/// The v1 op kinds that recorded one shift.
const SHIFTING: &[&str] = &[
    "clip.insert",
    "clip.trim",
    "clip.delete",
    "clip.remove",
    "clip.restore",
];
/// The v1 op kinds that recorded no shift.
const STILL: &[&str] = &["asset.add", "asset.remove"];

/// `diff` (v1-shaped, every entry without `op`) of a transaction whose raw v1 ops are `ops`,
/// in the v2 shape. `INVALID_FORMAT` when the entries do not match the ops.
pub(super) fn run(mut diff: Value, ops: &[Value]) -> Result<Value, CoreError> {
    let bad = |why: String| invalid_format("diff", why);
    let mut shifting = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        match op.get("op").and_then(Value::as_str) {
            Some(name) if SHIFTING.contains(&name) => shifting.push(i),
            Some(name) if STILL.contains(&name) => {}
            other => return Err(bad(format!("op {i} ({other:?}) is not a v1 op"))),
        }
    }
    let changes = list(&mut diff, "changes")?;
    if changes.len() != ops.len() {
        return Err(bad(format!(
            "a v1 diff has one change per op: {} changes, {} ops",
            changes.len(),
            ops.len()
        )));
    }
    for (i, (change, op)) in changes.iter_mut().zip(ops).enumerate() {
        let change = change
            .as_object_mut()
            .ok_or_else(|| bad(format!("change {i} is not an object")))?;
        change.insert("op".into(), json!(i));
        if op["op"] == "clip.trim" {
            change.insert("edge".into(), op["edge"].clone());
            if op["edge"] == "start" {
                let delta = op["delta"]
                    .as_str()
                    .ok_or_else(|| bad(format!("op {i} has no delta")))?;
                let by = negate(delta)
                    .ok_or_else(|| bad(format!("op {i} delta {delta:?} is not a time")))?;
                change.insert("by".into(), json!(by));
            }
        }
    }
    let shifts = list(&mut diff, "shifts")?;
    if shifts.len() != shifting.len() {
        return Err(bad(format!(
            "a v1 diff has one shift per clip op: {} shifts, {} clip ops",
            shifts.len(),
            shifting.len()
        )));
    }
    for (i, (shift, op)) in shifts.iter_mut().zip(shifting).enumerate() {
        shift
            .as_object_mut()
            .ok_or_else(|| bad(format!("shift {i} is not an object")))?
            .insert("op".into(), json!(op));
    }
    Ok(diff)
}

fn list<'a>(diff: &'a mut Value, key: &str) -> Result<&'a mut Vec<Value>, CoreError> {
    diff.get_mut(key)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| invalid_format("diff", format!("{key} is missing or not a list")))
}

/// `−t`, in canonical form (`n`, or `n/d` in lowest terms with `d > 1`; a leading `-` only;
/// `0` never signed), for a time `t` written as `[+-]n[/d]` in decimal digits. Parsed
/// leniently (a `+`, leading zeros, a fraction not in lowest terms), so the output is canonical
/// whatever the input; a canonical `t` only has its sign flipped. `None` when `t` is not of
/// that form, has a zero denominator, or does not fit 64 bits.
fn negate(t: &str) -> Option<String> {
    let (negative, body) = match t.as_bytes().first()? {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };
    let (num, den) = body.split_once('/').unwrap_or((body, "1"));
    let digits = |s: &str| -> Option<u64> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    let (num, den) = (digits(num)?, digits(den)?);
    if den == 0 {
        return None;
    }
    let (mut a, mut b) = (num, den);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let (num, den) = (num / a, den / a);
    if num == 0 {
        return Some("0".into());
    }
    let sign = if negative { "" } else { "-" };
    Some(if den == 1 {
        format!("{sign}{num}")
    } else {
        format!("{sign}{num}/{den}")
    })
}
