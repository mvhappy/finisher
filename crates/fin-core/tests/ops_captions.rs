//! Caption item edits (plan 4C Task 19): `caption.setText`, internal `caption.setWords`,
//! `caption.split`, `caption.merge`, `caption.setStyle`; and which captions a transaction
//! asks the end-of-transaction overflow check to measure (`captions::to_check`; Task 20 wires
//! the measurer).

mod common;

use std::collections::BTreeMap;

use common::ids_json::ids_cover_every_id_in_json;
use common::stale::{conflict_reason, rebase_over};
use common::*;
use fin_core::captions::{LayoutInputs, to_check};
use fin_core::rebase::{Touch, footprint, split_diff};
use fin_core::{
    ApplyMode, CaptionBlock, CaptionMerge, CaptionSetStyle, CaptionSetText, CaptionSetWords,
    CaptionSplit, CoreError, DeleteMode, Edge, EntityKind, InsertMode, ItemBody, ItemInsert,
    ItemRestore, ItemTrim, MemIds, Op, ParamValue, Project, StyleAdd, StyleSet, Track, TrackKind,
    TrimMode, Word, apply_tx_mode,
};

fn w(text: &str, start: &str, end: &str) -> Word {
    Word {
        text: text.into(),
        start: r(start),
        end: r(end),
    }
}

fn caption(id: &str, at: i64, duration: i64, words: &[(&str, &str, &str)]) -> Op {
    ItemInsert {
        id: id.into(),
        track: "t3".into(),
        at: secs(at),
        duration: secs(duration),
        mode: InsertMode::Overwrite,
        body: ItemBody::Caption {
            block: CaptionBlock {
                words: words.iter().map(|(t, s, e)| w(t, s, e)).collect(),
                style: "preset:bold".into(),
                overrides: BTreeMap::new(),
                source: None,
            },
        },
    }
    .into()
}

/// Caption track `t3` with `cap1` [0, 4) "one two three four" (a word a second) and `cap2`
/// [5, 7) "five six".
fn captioned() -> Project {
    let mut p = project();
    p.timelines[0]
        .tracks
        .push(Track::new("t3", TrackKind::Caption, "C1"));
    apply(
        &p,
        vec![
            caption(
                "$a",
                0,
                4,
                &[
                    ("one", "0", "1"),
                    ("two", "1", "2"),
                    ("three", "2", "3"),
                    ("four", "3", "4"),
                ],
            ),
            caption("$b", 5, 2, &[("five", "0", "1"), ("six", "1", "2")]),
        ],
    )
    .project
}

fn block<'a>(p: &'a Project, id: &str) -> &'a CaptionBlock {
    let it = p.timelines[0].tracks[2]
        .items
        .iter()
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no {id}"));
    match &it.body {
        ItemBody::Caption { block } => block,
        _ => panic!("{id} is not a caption"),
    }
}

fn set_text(item: &str, text: &str) -> Op {
    CaptionSetText {
        item: item.into(),
        text: text.into(),
    }
    .into()
}

fn split(item: &str, word: u32) -> Op {
    CaptionSplit {
        item: item.into(),
        word,
        id: "$cap".into(),
    }
    .into()
}

fn merge(item: &str, with: &str) -> Op {
    CaptionMerge {
        item: item.into(),
        with: with.into(),
    }
    .into()
}

fn set_style(item: &str, style: &str, overrides: Option<&[(&str, ParamValue)]>) -> Op {
    CaptionSetStyle {
        item: item.into(),
        style: style.into(),
        overrides: overrides.map(|o| {
            o.iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect()
        }),
    }
    .into()
}

fn code<T: std::fmt::Debug>(r: Result<T, CoreError>) -> String {
    r.unwrap_err().code
}

fn replay(p: &Project, ops: Vec<Op>) -> Result<fin_core::Applied, CoreError> {
    apply_tx_mode(p, &tx(ops), &mut MemIds::after(p), ApplyMode::Replay)
}

fn fp(op: &Op, a: &fin_core::Applied) -> fin_core::rebase::Footprint {
    footprint(
        op,
        &split_diff(std::slice::from_ref(op), &a.diff).unwrap()[0],
    )
}

#[test]
fn set_text_golden() {
    let p = captioned();
    let op = set_text("cap1", "One two 3 four");
    let a = golden_apply(&p, vec![op.clone()]);
    assert_eq!(
        block(&a.project, "cap1").words,
        [
            w("One", "0", "1"),
            w("two", "1", "2"),
            w("3", "2", "3"),
            w("four", "3", "4")
        ]
    );
    let c = &a.diff.changes[0];
    assert_eq!(
        (c.kind, c.id.as_str(), c.path.as_deref(), c.track.as_deref()),
        (EntityKind::Item, "cap1", Some("caption.words"), Some("t3"))
    );
    assert_eq!(
        a.inverse,
        vec![
            CaptionSetWords {
                item: "cap1".into(),
                words: block(&p, "cap1").words.clone(),
                source: None,
            }
            .into()
        ]
    );
    assert_eq!(
        fp(&op, &a).touches,
        [("cap1".to_string(), Touch::Param("caption.words".into()))]
    );
    ids_cover_every_id_in_json(&op);
    ids_cover_every_id_in_json(&a.inverse[0]);
    // No words, a control character, or an item that is not a caption.
    assert_eq!(
        code(try_apply(&p, vec![set_text("cap1", " \n ")])),
        "INVALID_OP"
    );
    assert_eq!(
        code(try_apply(&p, vec![set_text("cap1", "a\u{1}b")])),
        "INVALID_OP"
    );
    let q = with_clips(&[3]);
    assert_eq!(code(try_apply(&q, vec![set_text("c1", "x")])), "INVALID_OP");
    assert_eq!(
        code(try_apply(&p, vec![set_text("cap9", "x")])),
        "NOT_FOUND"
    );
}

#[test]
fn set_words_is_internal_and_checked() {
    let p = captioned();
    let words = vec![w("a", "0", "1"), w("b", "1", "3")];
    let op: Op = CaptionSetWords {
        item: "cap1".into(),
        words: words.clone(),
        source: None,
    }
    .into();
    assert_eq!(code(try_apply(&p, vec![op.clone()])), "INVALID_OP");
    let a = fin_core::apply_internal(&p, &tx(vec![op]), &mut MemIds::after(&p)).unwrap();
    assert_eq!(block(&a.project, "cap1").words, words);
    // Ordered, not overlapping, at least one (structural, replay included).
    for bad in [
        vec![],
        vec![w("a", "1", "0")],
        vec![w("a", "0", "2"), w("b", "1", "3")],
    ] {
        let op: Op = CaptionSetWords {
            item: "cap1".into(),
            words: bad,
            source: None,
        }
        .into();
        assert_eq!(code(replay(&p, vec![op])), "INVALID_OP");
    }
    // Out-of-range times are kept (an undo restores what a start trim moved out, Decision 13).
    let op: Op = CaptionSetWords {
        item: "cap1".into(),
        words: vec![w("a", "-1", "1")],
        source: None,
    }
    .into();
    assert!(fin_core::apply_internal(&p, &tx(vec![op]), &mut MemIds::after(&p)).is_ok());
}

#[test]
fn split_golden() {
    let p = captioned();
    let op = split("cap1", 2);
    let a = golden_apply(&p, vec![op.clone()]);
    assert_eq!(a.bindings["$cap"], "cap3");
    let items: Vec<(String, Rational, Rational)> = a.project.timelines[0].tracks[2]
        .items
        .iter()
        .map(|i| (i.id.clone(), i.start, i.duration))
        .collect();
    assert_eq!(
        items,
        [
            ("cap1".into(), secs(0), secs(2)),
            ("cap3".into(), secs(2), secs(2)),
            ("cap2".into(), secs(5), secs(2)),
        ]
    );
    assert_eq!(
        block(&a.project, "cap1").words,
        [w("one", "0", "1"), w("two", "1", "2")]
    );
    assert_eq!(
        block(&a.project, "cap3").words,
        [w("three", "0", "1"), w("four", "1", "2")]
    );
    let f = fp(&op, &a);
    assert!(f.touches.contains(&("cap1".to_string(), Touch::Exists)));
    assert!(
        f.touches
            .contains(&("cap1".to_string(), Touch::Edge(Edge::End)))
    );
    assert!(f.creates.contains(&"$cap".to_string()));
    ids_cover_every_id_in_json(&op);
    for inv in &a.inverse {
        ids_cover_every_id_in_json(inv);
    }
    // The cut is at a word strictly inside: not the first, not past the last.
    assert_eq!(code(try_apply(&p, vec![split("cap1", 0)])), "INVALID_RANGE");
    assert_eq!(code(try_apply(&p, vec![split("cap1", 4)])), "INVALID_RANGE");
    assert_eq!(
        code(try_apply(&with_clips(&[3]), vec![split("c1", 1)])),
        "INVALID_OP"
    );
}

#[test]
fn caption_merge_inverse_exact() {
    let p = captioned();
    let op = merge("cap1", "cap2");
    let a = golden_apply(&p, vec![op.clone()]);
    let track = &a.project.timelines[0].tracks[2];
    assert_eq!(track.items.len(), 1);
    assert_eq!(
        (track.items[0].start, track.items[0].duration),
        (secs(0), secs(7))
    );
    assert_eq!(
        block(&a.project, "cap1").words[4..],
        [w("five", "5", "6"), w("six", "6", "7")]
    );
    // The inverse: setWords, a strict trim, a lift restore.
    assert!(matches!(&a.inverse[0], Op::CaptionSetWords(s) if s.item == "cap1"));
    assert!(matches!(
        &a.inverse[1],
        Op::ItemTrim(ItemTrim {
            mode: TrimMode::Strict,
            edge: Edge::End,
            ..
        })
    ));
    assert!(
        matches!(&a.inverse[2], Op::ItemRestore(r) if matches!(**r, ItemRestore { mode: DeleteMode::Lift, .. }))
    );
    let f = fp(&op, &a);
    assert!(f.removes.contains(&"cap2".to_string()));
    assert!(
        f.touches
            .contains(&("cap1".to_string(), Touch::Edge(Edge::End)))
    );
    ids_cover_every_id_in_json(&op);
    // `with` must be the next caption on the same track.
    assert_eq!(
        code(try_apply(&p, vec![merge("cap2", "cap1")])),
        "NOT_ADJACENT"
    );
    assert_eq!(
        code(try_apply(&p, vec![merge("cap1", "cap1")])),
        "NOT_ADJACENT"
    );
    let q = apply(&p, vec![caption("$c", 10, 1, &[("x", "0", "1")])]).project;
    assert_eq!(
        code(try_apply(&q, vec![merge("cap1", "cap3")])),
        "NOT_ADJACENT"
    );
    assert!(try_apply(&q, vec![merge("cap2", "cap3")]).is_ok());
}

#[test]
fn set_style_golden() {
    let p = captioned();
    let p = apply(
        &p,
        vec![
            StyleAdd {
                id: "$st".into(),
                name: "Mine".into(),
                from: None,
                set: BTreeMap::new(),
            }
            .into(),
        ],
    )
    .project;
    let op = set_style(
        "cap1",
        "st1",
        Some(&[("text.size", ParamValue::Number(90.0))]),
    );
    let a = golden_apply(&p, vec![op.clone()]);
    let b = block(&a.project, "cap1");
    assert_eq!(b.style, "st1");
    assert_eq!(b.overrides["text.size"], ParamValue::Number(90.0));
    assert_eq!(a.inverse, vec![set_style("cap1", "preset:bold", Some(&[]))]);
    let c = &a.diff.changes[0];
    assert_eq!(
        (c.id.as_str(), c.path.as_deref()),
        ("cap1", Some("caption.style"))
    );
    let f = fp(&op, &a);
    assert_eq!(
        f.touches,
        [("cap1".to_string(), Touch::Param("caption.style".into()))]
    );
    assert!(f.uses.contains(&"st1".to_string()));
    ids_cover_every_id_in_json(&op);
    // No overrides given: they stay.
    let q = a.project;
    let a = golden_apply(&q, vec![set_style("cap1", "preset:clean", None)]);
    assert_eq!(block(&a.project, "cap1").overrides.len(), 1);
    // The style must exist (structural); overrides are style values.
    assert_eq!(
        code(replay(&p, vec![set_style("cap1", "st9", None)])),
        "NOT_FOUND"
    );
    assert_eq!(
        code(try_apply(
            &p,
            vec![set_style(
                "cap1",
                "st1",
                Some(&[("opacity", ParamValue::Number(1.0))])
            )]
        )),
        "PARAM_UNKNOWN"
    );
    assert_eq!(
        code(try_apply(
            &p,
            vec![set_style(
                "cap1",
                "st1",
                Some(&[("text.size", ParamValue::Number(-4.0))])
            )]
        )),
        "INVALID_RANGE"
    );
}

#[test]
fn set_text_vs_set_text_conflicts_same_param() {
    let p = captioned();
    let e = rebase_over(
        &p,
        vec![set_text("cap1", "a b")],
        vec![set_text("cap1", "c d")],
    )
    .unwrap_err();
    assert_eq!(conflict_reason(&e), "SAME_PARAM");
    // Text and style are different params: they merge.
    let ok = rebase_over(
        &p,
        vec![set_text("cap1", "a b")],
        vec![set_style("cap1", "preset:clean", None)],
    );
    assert!(ok.is_ok(), "{ok:?}");
}

/// Reverses the plan's `split_after_set_text_reruns` (review decision B1): a split records
/// the item's `caption.words`, so a stale split over a `caption.setText` (whose word index
/// would land on other words) is `SAME_PARAM`, whatever the index.
#[test]
fn split_after_set_text_conflicts() {
    let p = captioned();
    for k in [1, 3] {
        let e = rebase_over(
            &p,
            vec![set_text("cap1", "only two")],
            vec![split("cap1", k)],
        )
        .unwrap_err();
        assert_eq!(conflict_reason(&e), "SAME_PARAM", "word {k}");
    }
}

#[test]
fn caption_split_vs_start_trim() {
    let p = captioned();
    let trim: Op = ItemTrim {
        item: "cap1".into(),
        edge: Edge::Start,
        delta: r("1/2"),
        mode: TrimMode::Overwrite,
        alone: false,
    }
    .into();
    // The word index is unaffected; the times are theirs (rebased by −1/2).
    let rb = rebase_over(&p, vec![trim], vec![split("cap1", 2)]).unwrap();
    let track = &rb.applied.project.timelines[0].tracks[2];
    let starts: Vec<(String, Rational)> = track
        .items
        .iter()
        .map(|i| (i.id.clone(), i.start))
        .collect();
    assert_eq!(
        starts,
        [
            ("cap1".into(), r("1/2")),
            ("cap3".into(), secs(2)),
            ("cap2".into(), secs(5))
        ]
    );
    assert_eq!(
        block(&rb.applied.project, "cap1").words[0],
        w("one", "-1/2", "1/2")
    );
    assert_eq!(
        block(&rb.applied.project, "cap3").words[0],
        w("three", "0", "1")
    );
}

/// The captions an overflow check of `a` (applied to `p`) measures.
fn checks(p: &Project, a: &fin_core::Applied) -> Vec<String> {
    to_check(
        &LayoutInputs::of(p),
        &a.project,
        &a.diff,
        &Default::default(),
    )
}

#[test]
fn overflow_is_checked_for_what_the_tx_changed() {
    let p = captioned();
    let p = apply(
        &p,
        vec![
            StyleAdd {
                id: "$st".into(),
                name: "Mine".into(),
                from: None,
                set: BTreeMap::new(),
            }
            .into(),
            set_style("cap2", "$st", None),
        ],
    )
    .project;
    // A caption edit: that caption.
    let a = apply(&p, vec![set_text("cap1", "x y")]);
    assert_eq!(checks(&p, &a), ["cap1"]);
    // A style edit: its users.
    let op: Op = StyleSet {
        style: "st1".into(),
        path: "text.size".into(),
        value: Some(ParamValue::Number(100.0)),
    }
    .into();
    let a = apply(&p, vec![op]);
    assert_eq!(checks(&p, &a), ["cap2"]);
    // A resolution change: every caption of that timeline, once.
    let op: Op = fin_core::TimelineSet {
        timeline: "tl1".into(),
        path: "resolution".into(),
        value: Some(ParamValue::Vec2([1920.0, 1080.0])),
    }
    .into();
    let a = apply(&p, vec![op, set_text("cap1", "z")]);
    assert_eq!(checks(&p, &a), ["cap1", "cap2"]);
    // A split: both parts; a merge: the merged one; a clip edit: nothing.
    let a = apply(&p, vec![split("cap1", 2)]);
    assert_eq!(checks(&p, &a), ["cap1", "cap3"]);
    let a = apply(&p, vec![merge("cap1", "cap2")]);
    assert_eq!(checks(&p, &a), ["cap1"]);
    let q = with_clips(&[2, 2]);
    let a = apply(&q, vec![trim("c2", Edge::End, secs(-1))]);
    assert!(checks(&q, &a).is_empty());
}

use fin_core::Rational;

#[test]
fn set_text_never_hides_typed_words() {
    use fin_core::captions::shown_words;
    use fin_core::eval::eval_frame;
    let p = captioned();
    // Trim cap1 to [0, 2): "three" and "four" stay stored but are hidden.
    let p = apply(&p, vec![trim("cap1", Edge::End, secs(-2))]).project;
    assert_eq!(block(&p, "cap1").words.len(), 4);
    let shown: Vec<_> = shown_words(&block(&p, "cap1").words, secs(2))
        .into_iter()
        .map(|w| w.text)
        .collect();
    assert_eq!(shown, ["one", "two"]);
    let plan = eval_frame(&p, "tl1", r("1/2"), None).unwrap();
    let c = plan.layers.iter().find_map(|l| l.caption.clone()).unwrap();
    assert_eq!(c.words.len(), 2);
    // Typed words timed past the end (a hidden word matched, or text after the last shown
    // word) are refused, not hidden.
    for text in ["one two three", "one two more"] {
        let e = try_apply(&p, vec![set_text("cap1", text)]).unwrap_err();
        assert_eq!(e.code, "INVALID_RANGE", "{text}: {e:?}");
        assert_eq!(
            (&e.details["item"], &e.details["word"]),
            (&"cap1".into(), &2.into())
        );
        assert!(e.fix.is_some());
    }
    // Text within the shown words is fine; the hidden ones go, and the undo brings them back.
    let a = apply(&p, vec![set_text("cap1", "one 2")]);
    let texts: Vec<_> = block(&a.project, "cap1")
        .words
        .iter()
        .map(|w| w.text.clone())
        .collect();
    assert_eq!(texts, ["one", "2"]);
    assert_eq!(undo(&a), p);
    // A zero-length word at the very end is shown.
    let z = Word {
        text: "z".into(),
        start: secs(2),
        end: secs(2),
    };
    assert!(fin_core::captions::is_shown(&z, secs(2)));
    assert!(!fin_core::captions::is_shown(&w("z", "2", "3"), secs(2)));
}

#[test]
fn merge_drops_words_hidden_by_trims_on_both_sides() {
    let p = captioned();
    // cap2 [5, 7) trimmed to [5, 6): "six" hidden; cap1 trimmed to [0, 3): "four" hidden.
    let p = apply(
        &p,
        vec![
            trim("cap2", Edge::End, secs(-1)),
            trim("cap1", Edge::End, secs(-1)),
        ],
    )
    .project;
    let m = apply(&p, vec![merge("cap1", "cap2")]);
    let texts: Vec<_> = block(&m.project, "cap1")
        .words
        .iter()
        .map(|w| w.text.as_str())
        .collect();
    // "four" (past cap1's end, at the seam) goes; "six" (past cap2's end) stays, hidden.
    assert_eq!(texts, ["one", "two", "three", "five", "six"]);
    let merged = &m.project.timelines[0].tracks[2].items[0];
    let shown: Vec<_> =
        fin_core::captions::shown_words(&block(&m.project, "cap1").words, merged.duration)
            .into_iter()
            .map(|w| w.text)
            .collect();
    assert_eq!(shown, ["one", "two", "three", "five"]);
    assert_eq!(undo(&m), p);
}
