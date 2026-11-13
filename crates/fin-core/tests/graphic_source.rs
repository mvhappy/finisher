//! Review #54: a graphic's template time is content-anchored. Splitting a graphic, or moving
//! its start edge (any trim mode, a slide), keeps the retained content at the template time it
//! had: `sourceIn` (spec §5.13) moves with the edge exactly as a nested item's does. Item
//! keyframes were already content-anchored and stay in phase with the template.

mod common;

use common::*;
use fin_core::eval::{FramePlan, eval_frame};
use fin_core::{
    AssetAdd, AssetKind, Edge, InsertMode, Interp, ItemBody, ItemInsert, ItemSlide, ItemSlip,
    ItemSplit, ItemTrim, KeyframeSet, LottieInfo, Op, ParamValue, Project, Rational, TrimMode,
};

fn template() -> Op {
    let mut a = AssetAdd::media("$lot", "/m/brand.json", secs(10), None, None, "fp");
    a.kind = AssetKind::LottieTemplate;
    a.duration = Some(secs(4));
    a.lottie = Some(LottieInfo {
        width: 1080,
        height: 1920,
        rate: secs(30),
        duration: secs(4),
        slots: Vec::new(),
        fonts: Vec::new(),
    });
    a.into()
}

/// Asset `a1` (A/V, 100 s) and a template; graphic `g` on `t1` at `at` for 4 s, with opacity
/// keys 0.5 at 0 and 1 at 4 (content-anchored, the control for the template time).
fn graphic_at(at: i64) -> (Project, String) {
    let a = apply(&with_clips(&[]), vec![template()]);
    let (p, asset) = (a.project, a.bindings["$lot"].clone());
    let ops: Vec<Op> = vec![
        ItemInsert {
            id: "$g".into(),
            track: "t1".into(),
            at: secs(at),
            duration: secs(4),
            mode: InsertMode::Ripple,
            body: ItemBody::Graphic {
                asset,
                slots: Default::default(),
                source_in: Rational::zero(),
            },
        }
        .into(),
        KeyframeSet::timed(
            "$g",
            "opacity",
            secs(0),
            ParamValue::Number(0.5),
            Interp::Linear,
        )
        .into(),
        KeyframeSet::timed(
            "$g",
            "opacity",
            secs(4),
            ParamValue::Number(1.0),
            Interp::Linear,
        )
        .into(),
    ];
    let a = apply(&p, ops);
    let g = a.bindings["$g"].clone();
    (a.project, g)
}

fn frame(p: &Project, t: Rational) -> FramePlan {
    eval_frame(p, &p.timelines[0].id, t, None).unwrap()
}

/// The graphic layer at `t`: (item, template time, layer source time, opacity).
fn graphic(p: &Project, t: Rational) -> (String, Rational, Rational, f64) {
    let f = frame(p, t);
    let l = f
        .layers
        .iter()
        .find(|l| l.graphic.is_some())
        .unwrap_or_else(|| panic!("no graphic at {t}: {f:?}"));
    let g = l.graphic.as_ref().unwrap();
    let src = l.source.as_ref().unwrap();
    assert_eq!(
        src.time, g.time,
        "the layer's source time is the template time"
    );
    (l.item.clone(), g.time, src.time, l.opacity)
}

fn time_and_opacity(p: &Project, t: Rational) -> (Rational, f64) {
    let (_, time, _, opacity) = graphic(p, t);
    (time, opacity)
}

fn split(item: &str, offset: Rational) -> Op {
    ItemSplit {
        item: item.into(),
        offset,
        id: "$r".into(),
    }
    .into()
}

fn trim_mode(item: &str, edge: Edge, delta: Rational, mode: TrimMode) -> Op {
    ItemTrim {
        item: item.into(),
        edge,
        delta,
        mode,
        alone: false,
    }
    .into()
}

#[test]
fn control_template_time_is_the_local_time() {
    let (p, _) = graphic_at(0);
    assert_eq!(time_and_opacity(&p, secs(3)), (secs(3), 0.875));
    assert_eq!(time_and_opacity(&p, secs(1)).0, secs(1));
}

#[test]
fn split_keeps_template_time_on_both_halves() {
    let (p, g) = graphic_at(0);
    let a = apply(&p, vec![split(&g, secs(2))]);
    let right = a.bindings["$r"].clone();
    let (item, time, _, opacity) = graphic(&a.project, secs(3));
    assert_eq!(item, right);
    assert_eq!((time, opacity), (secs(3), 0.875), "the retained content");
    assert_eq!(time_and_opacity(&a.project, secs(1)), (secs(1), 0.625));
    assert_eq!(undo(&a), p, "apply then undo is the original project");
}

#[test]
fn split_of_a_shifted_graphic_adds_the_offset() {
    let (p, g) = graphic_at(0);
    let a = apply(&p, vec![split(&g, secs(1))]);
    let right = a.bindings["$r"].clone();
    let b = apply(&a.project, vec![split(&right, secs(1))]);
    for t in 0..4 {
        assert_eq!(time_and_opacity(&b.project, secs(t)).0, secs(t), "at {t}");
    }
    assert_eq!(undo(&b), a.project);
}

#[test]
fn start_trims_keep_template_time_in_every_mode() {
    for mode in [TrimMode::Overwrite, TrimMode::Ripple] {
        let (p, g) = graphic_at(0);
        let a = apply(&p, vec![trim_mode(&g, Edge::Start, secs(2), mode)]);
        // Overwrite keeps the content where it was; ripple pulls it back by 2.
        let at = if mode == TrimMode::Ripple { 1 } else { 3 };
        assert_eq!(
            time_and_opacity(&a.project, secs(at)),
            (secs(3), 0.875),
            "{mode:?}"
        );
        assert_eq!(undo(&a), p, "{mode:?}: apply then undo");
    }
}

#[test]
fn start_extension_reaches_before_the_template_and_clamps() {
    let (p, g) = graphic_at(2);
    let a = apply(
        &p,
        vec![trim_mode(&g, Edge::Start, secs(-1), TrimMode::Overwrite)],
    );
    // The content stays where it was; before the template's start the plan holds time 0.
    assert_eq!(time_and_opacity(&a.project, secs(3)).0, secs(1));
    assert_eq!(time_and_opacity(&a.project, secs(1)).0, Rational::zero());
    assert_eq!(undo(&a), p);
}

#[test]
fn roll_moves_the_later_graphic_source() {
    let (p, g) = graphic_at(0);
    let a = apply(
        &p,
        vec![
            split(&g, secs(2)),
            trim_mode("$r", Edge::Start, secs(1), TrimMode::Roll),
        ],
    );
    // The left part now runs to 3 and the right from 3: both show the original template time.
    for t in 0..4 {
        assert_eq!(time_and_opacity(&a.project, secs(t)).0, secs(t), "at {t}");
    }
    assert_eq!(undo(&a), p);
}

/// A slide moves the next part's start edge under its content (and extends the previous end).
#[test]
fn slide_keeps_the_neighbours_template_time() {
    let (p, g) = graphic_at(0);
    let a = apply(&p, vec![split(&g, secs(1))]);
    let mid = a.bindings["$r"].clone();
    let b = apply(&a.project, vec![split(&mid, secs(2))]);
    let last = b.bindings["$r"].clone();
    let c = apply(
        &b.project,
        vec![
            ItemSlide {
                item: mid.clone(),
                delta: r("1/2"),
                sides: None,
            }
            .into(),
        ],
    );
    let (item, time, _, _) = graphic(&c.project, r("15/4"));
    assert_eq!((item, time), (last, r("15/4")));
    assert_eq!(time_and_opacity(&c.project, r("5/4")).0, r("5/4"));
    // The slid part carries its own content: 1 → 3 now shows at 3/2 → 7/2.
    assert_eq!(time_and_opacity(&c.project, r("3/2")).0, secs(1));
    assert_eq!(undo(&c), b.project);
}

#[test]
fn slip_moves_a_graphic_template_under_a_fixed_window() {
    let (p, g) = graphic_at(0);
    let a = apply(
        &p,
        vec![
            ItemSlip {
                item: g.clone(),
                delta: secs(1),
            }
            .into(),
        ],
    );
    assert_eq!(time_and_opacity(&a.project, secs(0)).0, secs(1));
    // A slip moves no content offset: the keys stay put.
    assert_eq!(time_and_opacity(&a.project, secs(3)), (secs(4), 0.875));
    assert_eq!(undo(&a), p);
}

/// Control: a clip's split keeps its source mapping, as it always did.
#[test]
fn control_clip_split_keeps_source_mapping() {
    let p = with_clips(&[4]);
    let before = frame(&p, secs(3)).layers[0].source.clone().unwrap();
    let a = apply(&p, vec![split("c1", secs(2))]);
    let after = frame(&a.project, secs(3)).layers[0].source.clone().unwrap();
    assert_eq!(before, after);
    assert_eq!(undo(&a), p);
}
