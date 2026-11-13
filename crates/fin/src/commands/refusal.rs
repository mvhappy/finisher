//! A refused merge's conflict list, a page at a time (review batch L): `CONFLICT`'s
//! `details.conflicts` lists every conflicting candidate, which for a long branch dominates the
//! answer. `fin` keeps [`CONFLICT_CAP`] of them (from `--conflicts-from`, default the first),
//! adds `conflictsTotal` and `conflictsFrom`, and, while more follow, `conflictsNext`: the exact
//! dry-run `fin merge` that shows the next page. The top-level `details` (the first conflict's)
//! and the fix are kept.

use fin_core::CoreError;
use serde_json::json;

/// Conflicts one refusal lists: the 20 of every other capped list (`undo`, `merge`).
pub const CONFLICT_CAP: usize = 20;

/// The merge a refusal came from, to write its next page's command.
pub struct MergeCall<'a> {
    pub from: &'a str,
    pub into: &'a str,
    pub include: Option<&'a [String]>,
    pub up_to: Option<u64>,
    /// The first conflict to list (`--conflicts-from`).
    pub conflicts_from: usize,
}

/// `e` with its `details.conflicts` cut to a page (any other error unchanged).
pub fn page(mut e: CoreError, call: &MergeCall<'_>) -> CoreError {
    let Some(list) = e
        .details
        .get_mut("conflicts")
        .and_then(|c| c.as_array_mut())
    else {
        return e;
    };
    let total = list.len();
    let from = call.conflicts_from.min(total);
    if from == 0 && total <= CONFLICT_CAP {
        return e;
    }
    let end = (from + CONFLICT_CAP).min(total);
    let shown: Vec<_> = list.drain(from..end).collect();
    *list = shown;
    e.details["conflictsTotal"] = json!(total);
    e.details["conflictsFrom"] = json!(from);
    if end < total {
        e.details["conflictsNext"] = json!(next(call, end));
    }
    e
}

fn next(call: &MergeCall<'_>, from: usize) -> String {
    let q = crate::follow::quote;
    let mut cmd = format!("merge {} --into {} --dry-run", q(call.from), q(call.into));
    if let Some(include) = call.include {
        cmd.push_str(&format!(" --include {}", include.join(",")));
    }
    if let Some(n) = call.up_to {
        cmd.push_str(&format!(" --up-to {n}"));
    }
    cmd.push_str(&format!(" --conflicts-from {from}"));
    crate::follow::cmd(&cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn refusal(n: usize) -> CoreError {
        let all: Vec<Value> = (0..n).map(|i| json!({ "txId": format!("t{i}") })).collect();
        CoreError::new("CONFLICT", "x").with_details(json!({ "conflicts": all }))
    }

    fn call(from: usize) -> MergeCall<'static> {
        MergeCall {
            from: "work",
            into: "main",
            include: None,
            up_to: Some(7),
            conflicts_from: from,
        }
    }

    #[test]
    fn short_lists_are_untouched() {
        let e = page(refusal(20), &call(0));
        assert_eq!(e.details["conflicts"].as_array().unwrap().len(), 20);
        assert!(e.details.get("conflictsTotal").is_none());
    }

    #[test]
    fn long_lists_are_paged_with_the_next_command() {
        let e = page(refusal(45), &call(0));
        let shown = e.details["conflicts"].as_array().unwrap();
        assert_eq!(shown.len(), 20);
        assert_eq!(shown[0]["txId"], "t0");
        assert_eq!(e.details["conflictsTotal"], 45);
        assert_eq!(
            e.details["conflictsNext"],
            "fin merge work --into main --dry-run --up-to 7 --conflicts-from 20"
        );
        let last = page(refusal(45), &call(40));
        assert_eq!(last.details["conflicts"].as_array().unwrap().len(), 5);
        assert_eq!(last.details["conflicts"][0]["txId"], "t40");
        assert_eq!(last.details["conflictsFrom"], 40);
        assert!(last.details.get("conflictsNext").is_none());
        let past = page(refusal(45), &call(99));
        assert_eq!(past.details["conflicts"], json!([]));
        assert_eq!(past.details["conflictsFrom"], 45);
    }
}
