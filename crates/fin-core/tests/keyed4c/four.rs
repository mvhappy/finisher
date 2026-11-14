//! The 4C additions to the keyed model (plan 4C Task 26): the base with captions, effects and
//! pins, the intent strategies of `props_rebase_4c`, committed slips, and the refusal rule.

use std::collections::BTreeMap;

use super::{Intent, base, frames, intent};
use crate::common::apply;
use crate::common::gen4c::{self, INTER, caption, inter, sha};
use fin_core::rebase::reasons;
use fin_core::{
    CommittedTx, CoreError, EffectAdd, Op, ParamValue, Project, Rational, StyleAdd, Track,
    TrackKind,
};
use proptest::prelude::*;

/// [`base`] with a caption track `t5` holding `caps` captions (style `st1` on the second),
/// a blur on `c0_0` (`fx`) and `Inter` pinned (`pinned`).
pub fn base_4c(lens: &[Vec<i64>], linked: bool, trs: bool, x: (usize, bool, bool)) -> Project {
    let (caps, fx, pinned) = x;
    let mut p = base(lens, linked, trs);
    p.timelines[0]
        .tracks
        .push(Track::new("t5", TrackKind::Caption, "C1"));
    let mut ops: Vec<Op> = vec![
        StyleAdd {
            id: "$st".into(),
            name: "Big".into(),
            from: None,
            set: BTreeMap::from([("text.size".into(), ParamValue::Number(80.0))]),
        }
        .into(),
    ];
    let texts: [&[&str]; 3] = [
        &["one", "two"],
        &["three", "four", "five"],
        &["six", "seven"],
    ];
    for (i, words) in texts.iter().take(caps).enumerate() {
        let style = if i == 1 { "$st" } else { "preset:bold" };
        // Back to back (a merge needs touching captions).
        let at = frames([0, 30, 75][i]);
        ops.push(caption(&format!("$cap{i}"), "t5", at, words, style));
    }
    let first = p.timelines[0].tracks[0].items.first().map(|c| c.id.clone());
    if fx && let Some(c) = &first {
        ops.push(
            EffectAdd {
                item: c.clone(),
                id: "$fx".into(),
                kind: "blur".into(),
                at: None,
                params: BTreeMap::new(),
            }
            .into(),
        );
        ops.push(gen4c::key_op(c, "effects.$fx.radius", frames(6), 4.0));
        ops.push(
            EffectAdd {
                item: c.clone(),
                id: "$fx2".into(),
                kind: "colorAdjust".into(),
                at: None,
                params: BTreeMap::new(),
            }
            .into(),
        );
    }
    if pinned {
        ops.push(gen4c::pin(INTER, inter(&sha('a')), None));
    }
    apply(&p, ops).project
}

/// [`intent`] and the other 4C edits: trims in every mode (start trims included), split,
/// slip, opacity, keyframe set/remove, effect add/move, caption split/merge and font pins.
pub fn intent_4c() -> impl Strategy<Value = Intent> {
    let t = 0usize..4;
    let c = 0usize..6;
    prop_oneof![
        6 => intent(),
        2 => (t.clone(), c.clone(), any::<bool>(), -8i64..8, 0u8..3)
            .prop_filter("moves", |x| x.3 != 0)
            .prop_map(|(track, clip, start, frames, mode)| Intent::Trim { track, clip, start, frames, mode }),
        1 => (t.clone(), c.clone(), 1i64..8)
            .prop_map(|(track, clip, eighths)| Intent::Split { track, clip, eighths }),
        1 => (t.clone(), c.clone(), -20i64..20)
            .prop_filter("moves", |x| x.2 != 0)
            .prop_map(|(track, clip, frames)| Intent::Slip { track, clip, frames }),
        1 => (t.clone(), c.clone(), 0i64..10)
            .prop_map(|(track, clip, v)| Intent::Opacity { track, clip, v }),
        2 => (t.clone(), c.clone(), any::<bool>(), 0i64..30, 0i64..10)
            .prop_map(|(track, clip, fx, at, v)| Intent::KeySet { track, clip, fx, at, v }),
        1 => (t.clone(), c.clone(), 0usize..3)
            .prop_map(|(track, clip, j)| Intent::KeyRemove { track, clip, j }),
        1 => (t.clone(), c.clone(), any::<bool>(), prop::option::of(0u32..2))
            .prop_map(|(track, clip, blur, at)| Intent::EffectAdd { track, clip, blur, at }),
        1 => (t.clone(), c.clone(), 0u32..2)
            .prop_map(|(track, clip, to)| Intent::EffectMove { track, clip, to }),
        1 => (0usize..3, 1u32..3).prop_map(|(k, word)| Intent::CapSplit { k, word }),
        1 => (0usize..3).prop_map(|k| Intent::CapMerge { k }),
        1 => (0u8..3, prop::option::of(0u32..2))
            .prop_map(|(which, fallback)| Intent::Pin { which, fallback }),
    ]
}

/// Committed edits that move the start edge of the first clips of `t1` (B3): start trims in
/// every mode, slides and slips.
pub fn intent_edges() -> impl Strategy<Value = Intent> {
    let c = 0usize..2;
    prop_oneof![
        4 => (c.clone(), -8i64..8, 0u8..3)
            .prop_filter("moves", |x| x.1 != 0)
            .prop_map(|(clip, frames, mode)| Intent::Trim { track: 0, clip, start: true, frames, mode }),
        1 => (c.clone(), -6i64..6)
            .prop_filter("moves", |x| x.1 != 0)
            .prop_map(|(clip, frames)| Intent::Slide { track: 0, clip, frames }),
        1 => (c, -20i64..20)
            .prop_filter("moves", |x| x.1 != 0)
            .prop_map(|(clip, frames)| Intent::Slip { track: 0, clip, frames }),
    ]
}

/// Our edits at content offsets of the first clips of `t1`: keyframes and splits.
pub fn intent_offsets() -> impl Strategy<Value = Intent> {
    let c = 0usize..2;
    prop_oneof![
        3 => (c.clone(), any::<bool>(), 0i64..30, 0i64..10)
            .prop_map(|(clip, fx, at, v)| Intent::KeySet { track: 0, clip, fx, at, v }),
        1 => (c.clone(), 0usize..3).prop_map(|(clip, j)| Intent::KeyRemove { track: 0, clip, j }),
        2 => (c, 1i64..8).prop_map(|(clip, eighths)| Intent::Split { track: 0, clip, eighths }),
    ]
}

/// The committed slips per item over `since` (undone ones included, as their inverse slips).
pub fn slips(since: &[CommittedTx]) -> BTreeMap<String, Rational> {
    let mut out: BTreeMap<String, Rational> = BTreeMap::new();
    for c in since {
        for op in &c.tx.ops {
            if let Op::ItemSlip(s) = op {
                let e = out.entry(s.item.clone()).or_default();
                *e = e.checked_add(s.delta).unwrap();
            }
        }
    }
    out
}

/// Whether a stale refusal is one the spec allows where the fresh edit applies (plan 4C Task
/// 26, prop 3): every conflict is `SAME_EDGE`, `SAME_PARAM`, `REMOVED` or `FONT_PINNED`, or a
/// re-run's policy code the head explains: blamed on a committed record, or naming in its
/// details an entity a committed record changed (spec §4.3 blames only when one step did).
/// Since positions map through their anchors (2026-11-09) no unblamed policy refusal is allowed.
pub fn refusal_allowed(e: &CoreError, since: &[CommittedTx]) -> Result<(), String> {
    if e.code != "CONFLICT" {
        return Err(format!("not a conflict: {e:?}"));
    }
    let all: Vec<fin_core::Conflict> =
        serde_json::from_value(e.details["conflicts"].clone()).map_err(|x| x.to_string())?;
    let changed: std::collections::BTreeSet<&str> = since
        .iter()
        .flat_map(|c| &c.diff.changes)
        .map(|ch| ch.id.as_str())
        .collect();
    fn names(v: &serde_json::Value, changed: &std::collections::BTreeSet<&str>) -> bool {
        match v {
            serde_json::Value::String(s) => changed.contains(s.as_str()),
            serde_json::Value::Array(xs) => xs.iter().any(|x| names(x, changed)),
            serde_json::Value::Object(m) => m.values().any(|x| names(x, changed)),
            _ => false,
        }
    }
    for c in &all {
        let named = [
            reasons::SAME_EDGE,
            reasons::SAME_PARAM,
            reasons::REMOVED,
            "FONT_PINNED",
        ]
        .contains(&c.reason.as_str());
        let code = c
            .reason
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b == b'_');
        let policy = code
            && (c
                .against_tx
                .as_ref()
                .is_some_and(|t| since.iter().any(|s| s.tx.tx_id == *t))
                || c.details.as_ref().is_some_and(|d| names(d, &changed)));
        if !named && !policy {
            return Err(format!("{c:?}"));
        }
    }
    Ok(())
}
