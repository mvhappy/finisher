//! Evaluation cost (plan 4C review of Tasks 15/16, B2): a frame or a short audio range costs
//! the items it meets plus one linear pass per track, not items × transitions. 20,000 items
//! with 10,000 transitions per track; the bounds are generous (debug builds, slow machines).

mod eval_common;

use std::time::{Duration, Instant};

use eval_common::*;
use fin_core::eval::{eval_audio, eval_frame};
use fin_core::{Curve, Item, Project, Track, TrackKind};

const ITEMS: i64 = 20_000;

/// `ITEMS` one-second clips back to back, with a crossfade at every other cut.
fn long_track(id: &str, kind: TrackKind) -> Track {
    let items: Vec<Item> = (0..ITEMS)
        .map(|i| clip(&format!("{id}c{i}"), secs(i), secs(1), secs(1)))
        .collect();
    let mut t = track(id, kind, items);
    t.transitions = (0..ITEMS / 2)
        .map(|k| {
            let (a, b) = (2 * k, 2 * k + 1);
            crossfade(
                &format!("{id}tr{k}"),
                &format!("{id}c{a}"),
                &format!("{id}c{b}"),
                r("1/2"),
                Curve::Linear,
            )
        })
        .collect();
    t
}

fn big() -> Project {
    project_with(vec![timeline(
        "tl1",
        vec![
            long_track("v", TrackKind::Video),
            long_track("a", TrackKind::Audio),
        ],
    )])
}

#[test]
fn frames_and_ranges_of_a_long_timeline_are_cheap() {
    let p = big();
    let started = Instant::now();
    for k in 0..200 {
        let t = (secs(k * 97 % ITEMS) + r("3/4")).unwrap();
        let plan = eval_frame(&p, "tl1", t, None).unwrap();
        assert!(!plan.layers.is_empty() && plan.layers.len() <= 2, "{t}");
        let plan = eval_audio(&p, "tl1", t, (t + secs(2)).unwrap()).unwrap();
        assert!(!plan.items.is_empty() && plan.items.len() <= 4, "{t}");
    }
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(60),
        "200 frames and ranges took {took:?}"
    );
    // The crossfade at the cut at 1 s: both items, in item order.
    let plan = eval_frame(&p, "tl1", r("1"), None).unwrap();
    let ids: Vec<&str> = plan.layers.iter().map(|l| l.item.as_str()).collect();
    assert_eq!(ids, ["vc0", "vc1"]);
}
