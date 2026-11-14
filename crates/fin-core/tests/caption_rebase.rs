//! Caption edits meeting each other at a stale base (the review of plan 4C Tasks 18–19): the
//! repros E1–E7 as regression tests, and two properties — a stale edit that merges equals
//! the edit authored fresh at the head, both orders converge, and an undo's word times map
//! through a start trim.

mod common;

use std::collections::BTreeMap;

use common::stale::{conflict_reason, rebase_over};
use common::*;
use fin_core::rebase::Rebased;
use fin_core::{
    CaptionBlock, CaptionMerge, CaptionSetStyle, CaptionSetText, CaptionSetWords, CaptionSource,
    CaptionSplit, CoreError, Edge, InsertMode, ItemBody, ItemInsert, ItemTrim, MemIds, Op, Project,
    Rational, Track, TrackKind, TrimMode, Word, apply_internal,
};
use proptest::prelude::*;

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

/// `cap1` [0, 4) "one two three four" (a word a second) and `cap2` [5, 7) "five six" on `t3`.
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

fn set_style(item: &str, style: &str) -> Op {
    CaptionSetStyle {
        item: item.into(),
        style: style.into(),
        overrides: None,
    }
    .into()
}

fn trim_mode(item: &str, edge: Edge, delta: &str, mode: TrimMode) -> Op {
    ItemTrim {
        item: item.into(),
        edge,
        delta: r(delta),
        mode,
        alone: false,
    }
    .into()
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

/// The caption track as (start, duration, text, word times, style, source), ids dropped.
type Row = (
    Rational,
    Rational,
    String,
    Vec<(Rational, Rational)>,
    String,
    Option<CaptionSource>,
);

fn rows(p: &Project) -> Vec<Row> {
    p.timelines[0].tracks[2]
        .items
        .iter()
        .map(|it| match &it.body {
            ItemBody::Caption { block } => (
                it.start,
                it.duration,
                block
                    .words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
                block.words.iter().map(|w| (w.start, w.end)).collect(),
                block.style.clone(),
                block.source.clone(),
            ),
            _ => unreachable!(),
        })
        .collect()
}

fn ordered(words: &[Word]) -> bool {
    words
        .windows(2)
        .all(|p| p[0].start <= p[0].end && p[0].end <= p[1].start)
}

#[test]
fn e1_stale_set_text_over_split_conflicts() {
    let p = captioned();
    let e = rebase_over(
        &p,
        vec![split("cap1", 2)],
        vec![set_text("cap1", "one two three four!")],
    )
    .unwrap_err();
    assert_eq!(conflict_reason(&e), "SAME_PARAM");
}

#[test]
fn e2_stale_set_text_over_merge_conflicts() {
    let p = captioned();
    let e = rebase_over(
        &p,
        vec![merge("cap1", "cap2")],
        vec![set_text("cap1", "One two three four")],
    )
    .unwrap_err();
    assert_eq!(conflict_reason(&e), "SAME_PARAM");
}

#[test]
fn e3_undo_of_set_text_over_split_conflicts() {
    let p = captioned();
    let a = apply(&p, vec![set_text("cap1", "uno dos tres cuatro")]);
    let e = rebase_over(&a.project, vec![split("cap1", 2)], a.inverse.clone()).unwrap_err();
    assert_eq!(conflict_reason(&e), "SAME_PARAM");
}

#[test]
fn e4_merge_after_end_trim_keeps_words_in_order() {
    let p = captioned();
    let a = apply(
        &p,
        vec![
            trim_mode("cap1", Edge::End, "-2", TrimMode::Overwrite),
            caption("$c", 2, 1, &[("x", "0", "1")]),
        ],
    );
    // cap1 is [0, 2) but keeps "three" and "four" past its end (Decision 13).
    assert_eq!(block(&a.project, "cap1").words.len(), 4);
    let id = a.project.timelines[0].tracks[2].items[1].id.clone();
    let m = apply(&a.project, vec![merge("cap1", &id)]);
    let words = block(&m.project, "cap1").words.clone();
    assert_eq!(
        words,
        [w("one", "0", "1"), w("two", "1", "2"), w("x", "2", "3")]
    );
    // The dropped words come back with an undo.
    assert_eq!(undo(&m), a.project);
    // The merged words are a valid word list, and the caption splits.
    let again: Op = CaptionSetWords {
        item: "cap1".into(),
        words,
        source: None,
    }
    .into();
    apply_internal(&m.project, &tx(vec![again]), &mut MemIds::after(&m.project)).unwrap();
    apply(&m.project, vec![split("cap1", 2)]);
}

#[test]
fn e4b_merge_cuts_words_across_the_edges() {
    let p = captioned();
    // cap1 ends at 3/2, inside "two"; cap2 starts at 11/2, inside "five".
    let a = apply(
        &p,
        vec![
            trim_mode("cap1", Edge::End, "-5/2", TrimMode::Overwrite),
            trim_mode("cap2", Edge::Start, "1/2", TrimMode::Overwrite),
        ],
    );
    let m = apply(&a.project, vec![merge("cap1", "cap2")]);
    let words = &block(&m.project, "cap1").words;
    assert_eq!(
        *words,
        [
            w("one", "0", "1"),
            w("two", "1", "3/2"),
            w("five", "11/2", "6"),
            w("six", "6", "7"),
        ]
    );
    assert!(ordered(words));
    assert_eq!(undo(&m), a.project);
}

#[test]
fn e6_set_style_and_split_conflict_both_ways() {
    let p = captioned();
    let e = rebase_over(
        &p,
        vec![split("cap1", 2)],
        vec![set_style("cap1", "preset:clean")],
    )
    .unwrap_err();
    assert_eq!(conflict_reason(&e), "SAME_PARAM");
    let e = rebase_over(
        &p,
        vec![set_style("cap1", "preset:clean")],
        vec![split("cap1", 2)],
    )
    .unwrap_err();
    assert_eq!(conflict_reason(&e), "SAME_PARAM");
}

#[test]
fn e7_repeated_set_text_stays_on_the_grid() {
    let mut p = captioned();
    let mut text = String::from("one two three four");
    for i in 0..200 {
        text = format!("{text} {}", "z".repeat(3 + (i % 7)));
        p = apply(&p, vec![set_text("cap1", &text)]).project;
        let words = &block(&p, "cap1").words;
        assert!(ordered(words), "edit {i}: {words:?}");
        assert!(
            words
                .iter()
                .all(|w| 1000 % w.start.den() == 0 && 1000 % w.end.den() == 0),
            "edit {i}: {words:?}"
        );
    }
    let words = &block(&p, "cap1").words;
    assert_eq!(words.len(), 204);
    assert_eq!(words.last().unwrap().end, secs(4));
}

#[test]
fn word_range_follows_split_merge_and_set_text() {
    let mut p = captioned();
    for (id, range) in [("cap1", [10, 14]), ("cap2", [14, 16])] {
        let ItemBody::Caption { block } = &mut p.timelines[0].tracks[2]
            .items
            .iter_mut()
            .find(|i| i.id == id)
            .unwrap()
            .body
        else {
            unreachable!()
        };
        block.source = Some(CaptionSource {
            artifact: "tr1".into(),
            word_range: range,
        });
    }
    let range = |p: &Project, id: &str| block(p, id).source.as_ref().unwrap().word_range;
    let s = apply(&p, vec![split("cap1", 1)]);
    assert_eq!(
        (range(&s.project, "cap1"), range(&s.project, "cap3")),
        ([10, 11], [11, 14])
    );
    assert_eq!(undo(&s), p);
    let m = apply(&p, vec![merge("cap1", "cap2")]);
    assert_eq!(range(&m.project, "cap1"), [10, 16]);
    assert_eq!(undo(&m), p);
    // Edited text keeps the transcript span; a later split shares it out by words.
    let t = apply(&p, vec![set_text("cap1", "a b")]).project;
    assert_eq!(range(&t, "cap1"), [10, 14]);
    let s = apply(&t, vec![split("cap1", 1)]).project;
    assert_eq!((range(&s, "cap1"), range(&s, "cap3")), ([10, 12], [12, 14]));
}

// ---- Differential: stale (rebased) vs fresh, and convergence of both orders.

fn cap(c: u8) -> &'static str {
    ["cap1", "cap2"][c as usize]
}

/// A `caption.setText` or `caption.setStyle` of either caption.
fn text_or_style_edits() -> impl Strategy<Value = Op> {
    prop_oneof![
        (
            0u8..2,
            prop::sample::select(vec![
                "a b c d",
                "one TWO three",
                "one two three four five",
                "x",
                "two four"
            ])
        )
            .prop_map(|(c, t)| set_text(cap(c), t)),
        (
            0u8..2,
            prop::sample::select(vec!["preset:clean", "preset:punch"])
        )
            .prop_map(|(c, s)| set_style(cap(c), s)),
    ]
}

fn edits() -> impl Strategy<Value = Op> {
    prop_oneof![
        text_or_style_edits(),
        (0u8..2, 1u32..4).prop_map(|(c, k)| split(cap(c), k)),
        Just(merge("cap1", "cap2")),
        (0u8..2, prop::sample::select(vec!["1/2", "-1/2", "1"])).prop_map(|(c, d)| trim_mode(
            cap(c),
            Edge::Start,
            d,
            TrimMode::Overwrite
        )),
    ]
}

fn ok(r: Result<Rebased, CoreError>) -> Option<Project> {
    r.ok().map(|r| r.applied.project)
}

proptest! {
    #![proptest_config(prop_config(256))]

    /// Two concurrent caption edits: when the stale one merges it equals the same edit
    /// authored at the head (but for a trim, whose delta is a time), and when both orders
    /// merge they converge.
    #[test]
    fn caption_edits_stale_vs_fresh(a in edits(), b in edits()) {
        let p = captioned();
        if try_apply(&p, vec![a.clone()]).is_err() || try_apply(&p, vec![b.clone()]).is_err() {
            return Ok(());
        }
        let ab = ok(rebase_over(&p, vec![a.clone()], vec![b.clone()]));
        let ba = ok(rebase_over(&p, vec![b.clone()], vec![a.clone()]));
        if let Some(ab) = &ab && !matches!(b, Op::ItemTrim(_)) {
            let head = apply(&p, vec![a.clone()]).project;
            let fresh = try_apply(&head, vec![b.clone()]).map(|x| x.project);
            prop_assert!(fresh.is_ok(), "stale merged, fresh refused: {:?} then {:?}", a, b);
            prop_assert_eq!(rows(ab), rows(&fresh.unwrap()), "stale != fresh: {:?} then {:?}", a, b);
        }
        if let (Some(ab), Some(ba)) = (&ab, &ba) {
            prop_assert_eq!(rows(ab), rows(ba), "diverged: {:?} || {:?}", a, b);
        }
    }

    /// The inverse of a caption edit (a stale `caption.setWords`) rebased over a start trim
    /// equals the trim of the edit's base: the undo's word times map through the trim
    /// (Decisions 13, 16).
    #[test]
    fn set_words_inverse_maps_through_start_trim(
        e in text_or_style_edits(),
        d in prop::sample::select(vec!["1/2", "-1/2"]),
    ) {
        let p = captioned();
        let Ok(a) = try_apply(&p, vec![e.clone()]) else { return Ok(()) };
        let item = match &e {
            Op::CaptionSetText(s) => s.item.clone(),
            Op::CaptionSetStyle(s) => s.item.clone(),
            _ => unreachable!(),
        };
        let tr = trim_mode(&item, Edge::Start, d, TrimMode::Overwrite);
        if try_apply(&a.project, vec![tr.clone()]).is_err() || try_apply(&p, vec![tr.clone()]).is_err() {
            return Ok(());
        }
        let Some(rb) = ok(rebase_over(&a.project, vec![tr.clone()], a.inverse.clone())) else {
            return Ok(());
        };
        prop_assert_eq!(rows(&rb), rows(&apply(&p, vec![tr]).project));
    }
}
