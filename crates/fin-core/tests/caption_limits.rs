//! The words a caption item may hold (review finding #14): at most
//! [`MAX_CAPTION_WORDS`] (5,000) on every edit that makes a caption's words: `item.insert` of
//! a caption body (`fin caption import` and SubRip import expand into those), `caption.setText`
//! (refused before the text is aligned), the internal `caption.setWords` and the result of
//! `caption.merge`. More is `CAPTION_TOO_LONG {item, words, max, opIndex}`. A policy check:
//! replay does not re-judge committed history.

mod common;

use std::collections::BTreeMap;

use common::*;
use fin_core::captions::{MAX_CAPTION_WORDS, parse_srt};
use fin_core::compose::caption_import;
use fin_core::{
    ApplyMode, CaptionBlock, CaptionMerge, CaptionSetText, CaptionSetWords, CaptionStyle,
    CoreError, InsertMode, ItemBody, ItemInsert, MemIds, Op, Project, Rational, Track, TrackKind,
    Word, apply_tx_mode,
};
use serde_json::json;

/// `n` words `{prefix}{i}`, a millisecond each from 0.
fn words(n: usize, prefix: &str) -> Vec<Word> {
    (0..n)
        .map(|i| Word {
            text: format!("{prefix}{i}"),
            start: Rational::new(i as i64, 1000).unwrap(),
            end: Rational::new(i as i64 + 1, 1000).unwrap(),
        })
        .collect()
}

fn caption(id: &str, at: i64, words: Vec<Word>) -> Op {
    ItemInsert {
        id: id.into(),
        track: "t3".into(),
        at: secs(at),
        duration: secs(10),
        mode: InsertMode::Overwrite,
        body: ItemBody::Caption {
            block: CaptionBlock {
                words,
                style: "preset:bold".into(),
                overrides: BTreeMap::new(),
                source: None,
            },
        },
    }
    .into()
}

/// A project with caption track `t3`.
fn with_caption_track() -> Project {
    let mut p = project();
    p.timelines[0]
        .tracks
        .push(Track::new("t3", TrackKind::Caption, "C1"));
    p
}

/// `with_caption_track` plus one caption per entry of `counts`, 10 s each from 0, in order.
fn captioned(counts: &[usize]) -> (Project, Vec<String>) {
    let p = with_caption_track();
    let ops = counts
        .iter()
        .enumerate()
        .map(|(k, n)| caption(&format!("$c{k}"), 10 * k as i64, words(*n, "w")))
        .collect();
    let p = apply(&p, ops).project;
    let ids = p.timelines[0].tracks[2]
        .items
        .iter()
        .map(|i| i.id.clone())
        .collect();
    (p, ids)
}

fn mode(p: &Project, ops: Vec<Op>, mode: ApplyMode) -> Result<fin_core::Applied, CoreError> {
    apply_tx_mode(p, &tx(ops), &mut MemIds::after(p), mode)
}

fn assert_too_long(e: &CoreError, item: Option<&str>, words: usize, op_index: usize) {
    assert_eq!(e.code, "CAPTION_TOO_LONG", "{e:?}");
    assert_eq!(e.details["words"], json!(words), "{e:?}");
    assert_eq!(e.details["max"], json!(MAX_CAPTION_WORDS), "{e:?}");
    assert_eq!(e.details["opIndex"], json!(op_index), "{e:?}");
    if let Some(item) = item {
        assert_eq!(e.details["item"], json!(item), "{e:?}");
    }
    assert!(e.fix.is_some(), "{e:?}");
}

#[test]
fn the_limit_is_5000() {
    assert_eq!(MAX_CAPTION_WORDS, 5_000);
}

#[test]
fn insert_takes_5000_words_and_refuses_5001() {
    let p = with_caption_track();
    let ok = apply(&p, vec![caption("$a", 0, words(5_000, "w"))]).project;
    assert_eq!(ok.timelines[0].tracks[2].items.len(), 1);
    let e = try_apply(
        &p,
        vec![
            caption("$a", 0, words(1, "w")),
            caption("$b", 20, words(5_001, "w")),
        ],
    )
    .unwrap_err();
    assert_too_long(&e, Some("$b"), 5_001, 1);
    // Replay does not re-judge history committed under other rules.
    mode(
        &p,
        vec![caption("cap9", 0, words(5_001, "w"))],
        ApplyMode::Replay,
    )
    .unwrap();
}

#[test]
fn set_text_takes_5000_words_and_refuses_5001() {
    let (p, ids) = captioned(&[3]);
    let text = |n: usize| {
        (0..n)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let set = |n: usize| -> Op {
        CaptionSetText {
            item: ids[0].clone(),
            text: text(n),
        }
        .into()
    };
    let ok = apply(&p, vec![set(5_000)]).project;
    let it = &ok.timelines[0].tracks[2].items[0];
    let ItemBody::Caption { block } = &it.body else {
        panic!("not a caption")
    };
    assert_eq!(block.words.len(), 5_000);
    let e = try_apply(&p, vec![set(5_001)]).unwrap_err();
    assert_too_long(&e, Some(&ids[0]), 5_001, 0);
}

#[test]
fn set_words_takes_5000_words_and_refuses_5001() {
    let (p, ids) = captioned(&[3]);
    let set = |n: usize| -> Op {
        CaptionSetWords {
            item: ids[0].clone(),
            words: words(n, "v"),
            source: None,
        }
        .into()
    };
    mode(&p, vec![set(5_000)], ApplyMode::Internal).unwrap();
    let e = mode(&p, vec![set(5_001)], ApplyMode::Internal).unwrap_err();
    assert_too_long(&e, Some(&ids[0]), 5_001, 0);
    mode(&p, vec![set(5_001)], ApplyMode::Replay).unwrap();
}

#[test]
fn merge_refuses_a_result_over_5000_words() {
    let merge = |ids: &[String]| -> Op {
        CaptionMerge {
            item: ids[0].clone(),
            with: ids[1].clone(),
        }
        .into()
    };
    let (p, ids) = captioned(&[2_500, 2_500]);
    let merged = apply(&p, vec![merge(&ids)]).project;
    assert_eq!(merged.timelines[0].tracks[2].items.len(), 1);
    let (p, ids) = captioned(&[2_500, 2_501]);
    let e = try_apply(&p, vec![merge(&ids)]).unwrap_err();
    assert_too_long(&e, Some(&ids[0]), 5_001, 0);
}

/// One SubRip cue of `n` one-letter words over 100 s.
fn srt(n: usize) -> String {
    format!(
        "1\n00:00:00,000 --> 00:01:40,000\n{}\n",
        vec!["a"; n].join(" ")
    )
}

#[test]
fn srt_import_holds_each_caption_item_to_the_limit() {
    let frame = r("1/30");
    let mut p = with_caption_track();
    // The style's rule bounds a block (at most 10 lines of 200 characters), so a long cue
    // becomes several items, each within the limit.
    let parsed = parse_srt(&srt(6_000), frame).unwrap();
    let composed = caption_import(&p, "t3", secs(0), &parsed.words, "preset:bold").unwrap();
    let imported = apply(&p, composed.ops).project;
    let items = &imported.timelines[0].tracks[2].items;
    assert!(items.len() > 1);
    // A block over the limit (a style stored past the policy ranges) is refused per item.
    p.styles.insert(
        "st1".into(),
        CaptionStyle {
            id: "st1".into(),
            name: "huge".into(),
            max_chars: 1_000_000,
            max_lines: 10,
            gap: Rational::zero(),
            max_duration: None,
            ..CaptionStyle::default()
        },
    );
    for (n, ok) in [(5_000, true), (5_001, false)] {
        let parsed = parse_srt(&srt(n), frame).unwrap();
        let composed = caption_import(&p, "t3", secs(0), &parsed.words, "st1").unwrap();
        assert_eq!(composed.ops.len(), 1);
        let got = try_apply(&p, composed.ops);
        if ok {
            got.unwrap();
        } else {
            assert_too_long(&got.unwrap_err(), Some("$cap1"), n, 0);
        }
    }
}
