//! Fix texts in `fin` terms for the edit refusals whose core fix names ops (`item.split`,
//! `item.unlink`, `compose::overwrite`): what to run next, with the ids from the error's
//! details (plan 4C Task 23). Other errors keep the core's fix.

use fin_core::CoreError;
use serde_json::Value;

fn s<'v>(d: &'v Value, key: &str) -> &'v str {
    d.get(key).and_then(Value::as_str).unwrap_or("<id>")
}

pub fn for_cli(e: CoreError) -> CoreError {
    let d = &e.details;
    let fix = match e.code.as_str() {
        "OVERWRITE_SPLITS_ITEM" => format!(
            "split item {} first (fin item split {} --at <time>), then repeat; fin clip insert, \
             image insert and graphic insert split for you",
            s(d, "item"),
            s(d, "item")
        ),
        "NOT_ADJACENT" if d.get("between").is_some() => {
            "a transition needs two touching items: --between <out> <in>, where <in> starts at \
             <out>'s end (fin view shows the cuts)"
                .to_string()
        }
        "NOT_ADJACENT" if d.get("neighbour").is_some() => format!(
            "item {} and {} do not touch: slide an item only against touching neighbours, or \
             move it (fin item move)",
            s(d, "item"),
            s(d, "neighbour")
        ),
        "NOT_ADJACENT" => format!(
            "no item touches that edge of {}: trim with --mode ripple or --mode overwrite, or \
             roll an edge at a cut",
            s(d, "item")
        ),
        "LINK_LEGS_OVERLAP" => {
            let items = d.get("items").map(Value::to_string).unwrap_or_default();
            format!(
                "linked items {items} would land on one track ({}): move to another --track, or \
                 fin item unlink {} and move them one by one",
                s(d, "track"),
                d.pointer("/items/0")
                    .and_then(Value::as_str)
                    .unwrap_or("<item>")
            )
        }
        "ALREADY_LINKED" => format!(
            "item {} is in link group {}: fin item unlink {} first",
            s(d, "item"),
            s(d, "link"),
            s(d, "link")
        ),
        "TRANSITION_NO_HANDLES" => format!(
            "item {} has too little source past the cut: use a shorter --dur, or trim the items \
             (fin item trim) to leave source there",
            s(d, "item")
        ),
        "TRANSITION_EXISTS" => format!(
            "fin transition set updates transition {}; fin transition remove {} takes it off",
            s(d, "transition"),
            s(d, "transition")
        ),
        "MOVE_INTO_ITSELF" => format!(
            "move item {} to a time outside it, or with --mode overwrite",
            s(d, "item")
        ),
        "SLIDE_BLOCKED" => format!(
            "slide item {} by less, or toward a touching neighbour (fin view shows the gaps)",
            s(d, "item")
        ),
        "TRACK_LOCKED" => format!(
            "unlock it first: fin track set {} lock false",
            s(d, "track")
        ),
        "TRACK_NOT_EMPTY" => format!(
            "fin track remove {} --with-items also removes its items, or delete them first",
            s(d, "track")
        ),
        "BIN_NOT_EMPTY" => format!(
            "empty bin {} first: fin asset move <a> (to the top level) and fin bin move <b>",
            s(d, "bin")
        ),
        _ => return e,
    };
    e.with_fix(fix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edit_refusals_get_fin_fixes_with_their_ids() {
        let cases = [
            (
                "OVERWRITE_SPLITS_ITEM",
                json!({ "item": "c3" }),
                "fin item split c3",
            ),
            (
                "NOT_ADJACENT",
                json!({ "track": "t1", "between": ["c1", "c3"] }),
                "--between",
            ),
            ("NOT_ADJACENT", json!({ "item": "c1" }), "--mode ripple"),
            (
                "LINK_LEGS_OVERLAP",
                json!({ "items": ["c1", "c2"], "track": "t2" }),
                "fin item unlink c1",
            ),
            (
                "ALREADY_LINKED",
                json!({ "item": "c1", "link": "lk1" }),
                "fin item unlink lk1",
            ),
            (
                "TRANSITION_NO_HANDLES",
                json!({ "item": "c1" }),
                "shorter --dur",
            ),
            (
                "TRANSITION_EXISTS",
                json!({ "transition": "tr1" }),
                "fin transition remove tr1",
            ),
            (
                "TRACK_LOCKED",
                json!({ "track": "t1" }),
                "fin track set t1 lock false",
            ),
            (
                "NOT_ADJACENT",
                json!({ "item": "c1", "neighbour": "c2" }),
                "fin item move",
            ),
            (
                "SLIDE_BLOCKED",
                json!({ "item": "c1" }),
                "slide item c1 by less",
            ),
            ("TRACK_NOT_EMPTY", json!({ "track": "t1" }), "--with-items"),
            ("BIN_NOT_EMPTY", json!({ "bin": "b1" }), "fin bin move"),
        ];
        for (code, details, want) in cases {
            let e = CoreError::new(code, "x")
                .with_details(details)
                .with_fix("core fix");
            let fix = for_cli(e).fix.unwrap();
            assert!(fix.contains(want), "{code}: {fix}");
        }
        let other = CoreError::new("CLIP_OVERLAP", "x").with_fix("core fix");
        assert_eq!(for_cli(other).fix.as_deref(), Some("core fix"));
    }
}
