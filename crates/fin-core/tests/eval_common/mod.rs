//! Projects built literally from model types for the eval tests: evaluation must not depend on
//! the ops that create the item kinds, effects, keyframes and transitions.
#![allow(dead_code)]

use std::collections::BTreeMap;

use fin_core::{
    Asset, AssetKind, AudioInfo, AudioParams, CaptionBlock, Curve, Effect, ImageInfo, Interp, Item,
    ItemBody, Keyframe, LottieInfo, ParamValue, Project, Rational, Resolution, SlotSpec, Timeline,
    Track, TrackKind, Transition, TransitionKind, VideoInfo, Word, new_project,
};

pub fn r(s: &str) -> Rational {
    s.parse().unwrap()
}

pub fn secs(n: i64) -> Rational {
    Rational::from_int(n)
}

pub fn prop_config(default: u32) -> proptest::test_runner::Config {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    proptest::test_runner::Config {
        cases,
        ..proptest::test_runner::Config::default()
    }
}

/// Assets: `a1` media (video + audio, 600 s), `a2` image, `a3` a template with a `title`
/// (text) and a `tint` (color) slot.
pub fn assets() -> BTreeMap<String, Asset> {
    let base = |id: &str, kind| Asset {
        id: id.into(),
        kind,
        path: format!("{id}.bin"),
        fingerprint: format!("fp-{id}"),
        bin: None,
        duration: None,
        video: None,
        audio: None,
        image: None,
        lottie: None,
        font: None,
        lut: None,
    };
    let mut media = base("a1", AssetKind::Media);
    media.duration = Some(secs(600));
    media.video = Some(VideoInfo {
        width: 1920,
        height: 1080,
        rate: secs(30),
    });
    media.audio = Some(AudioInfo {
        sample_rate: 48000,
        channels: 2,
    });
    let mut image = base("a2", AssetKind::Image);
    image.image = Some(ImageInfo {
        width: 800,
        height: 600,
    });
    let mut lottie = base("a3", AssetKind::LottieTemplate);
    lottie.duration = Some(secs(3));
    lottie.lottie = Some(LottieInfo {
        width: 1080,
        height: 1920,
        rate: secs(30),
        duration: secs(3),
        slots: vec![
            SlotSpec {
                id: "title".into(),
                kind: "text".into(),
            },
            SlotSpec {
                id: "tint".into(),
                kind: "color".into(),
            },
        ],
        fonts: Vec::new(),
    });
    [media, image, lottie]
        .into_iter()
        .map(|a| (a.id.clone(), a))
        .collect()
}

pub fn track(id: &str, kind: TrackKind, items: Vec<Item>) -> Track {
    let mut t = Track::new(id, kind, id.to_uppercase());
    t.items = items;
    t
}

pub fn timeline(id: &str, tracks: Vec<Track>) -> Timeline {
    Timeline {
        id: id.into(),
        name: id.into(),
        rate: secs(30),
        resolution: Resolution {
            width: 1080,
            height: 1920,
        },
        markers: Vec::new(),
        tracks,
    }
}

/// A 30 fps project holding `timelines` and [`assets`].
pub fn project_with(timelines: Vec<Timeline>) -> Project {
    let mut p = new_project("p1".into(), "Eval".into(), secs(30)).unwrap();
    p.timelines = timelines;
    p.assets = assets();
    p
}

pub fn clip(id: &str, start: Rational, source_in: Rational, duration: Rational) -> Item {
    Item::clip(id, "a1", start, source_in, duration)
}

pub fn with_speed(mut item: Item, s: Rational) -> Item {
    if let ItemBody::Clip { speed, .. } = &mut item.body {
        *speed = s;
    }
    item
}

/// A key with a fresh id (eval ignores ids).
pub fn kf(at: Rational, value: ParamValue, interp: Interp) -> Keyframe {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Keyframe {
        id: format!("kf{n}"),
        at,
        value,
        interp,
    }
}

pub fn num(n: f64) -> ParamValue {
    ParamValue::Number(n)
}

pub fn keyed(mut item: Item, path: &str, kfs: Vec<Keyframe>) -> Item {
    item.common.keyframes.insert(path.into(), kfs);
    item
}

pub fn effect(id: &str, kind: &str, params: &[(&str, ParamValue)], enabled: bool) -> Effect {
    Effect {
        id: id.into(),
        kind: kind.into(),
        enabled,
        params: params
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    }
}

pub fn crossfade(id: &str, a: &str, b: &str, duration: Rational, curve: Curve) -> Transition {
    Transition {
        id: id.into(),
        between: [a.into(), b.into()],
        kind: TransitionKind::Crossfade,
        duration,
        curve,
    }
}

pub fn caption(
    id: &str,
    start: Rational,
    duration: Rational,
    words: &[(&str, &str, &str)],
) -> Item {
    let words = words
        .iter()
        .map(|(text, s, e)| Word {
            text: text.to_string(),
            start: r(s),
            end: r(e),
        })
        .collect();
    let block = CaptionBlock {
        words,
        style: "preset:bold".into(),
        overrides: BTreeMap::new(),
        source: None,
    };
    Item::new(id, start, duration, ItemBody::Caption { block })
}

pub fn with_audio(
    mut item: Item,
    gain: f64,
    pan: f64,
    fade_in: Rational,
    fade_out: Rational,
) -> Item {
    item.common.audio = Some(AudioParams {
        gain,
        pan,
        fade_in,
        fade_out,
    });
    item
}
