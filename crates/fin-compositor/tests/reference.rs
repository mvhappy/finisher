//! The CPU reference compositor against its corpus (spec §13.2, T0): every case's output
//! BLAKE3 equals `golden/t0.json` on every OS and on wasm32 (`wasm32-wasip1`); frame ids read
//! from the output equal `frame_at`'s choice; layers it cannot draw are reported.
//!
//! `FIN_BLESS_T0=1` rewrites the goldens (native only); `FIN_REFERENCE_PNG=<dir>` writes each
//! output as a PNG for inspection.

use std::collections::{BTreeMap, HashMap};

use fin_compositor::{
    LayerIssue, SourceFrame, SourceProvider, base_box, layer_matrix, reference, transform_point,
};
use fin_core::Rational;
use fin_core::eval::{FramePlan, Layer};
use fin_media::testing::{Rect, Yuv420, pattern, read_id_rgba};
use fin_media::{
    ColourTags, FrameEntry, PixelFormat, RENDER_VERSION, Rotation, SampleTable, Size, display_size,
    frame_at,
};
use serde::Deserialize;
use serde_json::json;

const CORPUS: [&str; 4] = [
    include_str!("corpus/basic.json"),
    include_str!("corpus/blend.json"),
    include_str!("corpus/transitions.json"),
    include_str!("corpus/geometry.json"),
];
const GOLDEN: &str = include_str!("golden/t0.json");

#[derive(Debug, Deserialize)]
struct PatternSpec {
    width: u32,
    height: u32,
    seed: u64,
}

#[derive(Debug, Deserialize)]
struct SourceSpec {
    pattern: PatternSpec,
    rate: Rational,
    rotation: Rotation,
    colour: ColourTags,
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    out: [u32; 2],
    plan: FramePlan,
    sources: BTreeMap<String, SourceSpec>,
    #[serde(default)]
    ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Golden {
    render_version: u32,
    hashes: BTreeMap<String, String>,
}

/// A CFR table of 10,000 frames at `rate`.
fn table(rate: Rational) -> SampleTable {
    let (ts, d) = (rate.num() as u32, rate.den() as u32);
    let frames = (0..10_000u32)
        .map(|i| FrameEntry {
            pts: i as i64 * d as i64,
            duration: d,
            keyframe: true,
            decode_index: i,
            offset: 0,
            size: 1,
        })
        .collect();
    SampleTable::new(ts, frames).unwrap()
}

/// Pattern pictures chosen by `frame_at` at each layer's source time.
struct Patterns<'a> {
    sources: &'a BTreeMap<String, SourceSpec>,
    cache: HashMap<(String, u32), Yuv420>,
}

impl Patterns<'_> {
    fn index(&self, l: &Layer) -> Option<u32> {
        let s = l.source.as_ref()?;
        let spec = self.sources.get(&s.asset)?;
        Some(frame_at(&table(spec.rate), s.time).index as u32)
    }
}

impl SourceProvider for Patterns<'_> {
    fn frame(&mut self, l: &Layer) -> Result<SourceFrame<'_>, LayerIssue> {
        let s = l.source.as_ref().expect("clip layers have a source");
        let Some(spec) = self.sources.get(&s.asset) else {
            return Err(LayerIssue {
                code: "ORIGINALS_MISSING".into(),
                details: json!({ "asset": s.asset }),
            });
        };
        let index = self.index(l).expect("source present");
        let (w, h, seed) = (spec.pattern.width, spec.pattern.height, spec.pattern.seed);
        let p = self
            .cache
            .entry((s.asset.clone(), index))
            .or_insert_with(|| pattern(index, w, h, seed));
        Ok(SourceFrame {
            format: PixelFormat::I420,
            size: Size::new(w, h),
            planes: [&p.y, &p.u, &p.v],
            strides: [w as usize, w.div_ceil(2) as usize, w.div_ceil(2) as usize],
            colour: spec.colour.resolve_defaults(h),
            rotation: spec.rotation,
            pixel_aspect: Rational::from_int(1),
        })
    }
}

fn cases() -> Vec<Case> {
    CORPUS
        .iter()
        .flat_map(|c| serde_json::from_str::<Vec<Case>>(c).unwrap())
        .collect()
}

/// Where layer `l` (unrotated) lands in the output: its base box under its transform.
fn landed(c: &Case, l: &Layer, spec: &SourceSpec) -> Rect {
    let res = c.plan.resolution;
    let display = display_size(
        Size::new(spec.pattern.width, spec.pattern.height),
        spec.rotation,
        Rational::from_int(1),
    );
    let m = layer_matrix(&l.transform, base_box(display, res), res, Rotation::R0);
    let (a, b) = (
        transform_point(&m, [0.0, 0.0]),
        transform_point(&m, [1.0, 1.0]),
    );
    let (sx, sy) = (
        c.out[0] as f64 / res.width as f64,
        c.out[1] as f64 / res.height as f64,
    );
    Rect {
        x: a[0] * sx,
        y: a[1] * sy,
        width: (b[0] - a[0]) * sx,
        height: (b[1] - a[1]) * sy,
    }
}

fn write_png(dir: &str, name: &str, size: Size, rgba: &[u8]) {
    let f = std::fs::File::create(format!("{dir}/{name}.png")).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), size.width, size.height);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}

#[test]
fn corpus_matches_the_t0_goldens_and_frame_ids_read_back() {
    let golden: Golden = serde_json::from_str(GOLDEN).unwrap();
    let mut got = BTreeMap::new();
    for c in cases() {
        let out = Size::new(c.out[0], c.out[1]);
        let mut src = Patterns {
            sources: &c.sources,
            cache: HashMap::new(),
        };
        let r = reference::render(&c.plan, &mut src, out);
        assert_eq!(r.rgba8.len(), (out.width * out.height * 4) as usize);
        assert!(
            r.rgba8.chunks(4).all(|p| p[3] == 255),
            "{}: the frame is opaque",
            c.name
        );
        for item in &c.ids {
            let l = c.plan.layers.iter().find(|l| &l.item == item).unwrap();
            let spec = &c.sources[&l.source.as_ref().unwrap().asset];
            let want = src.index(l).unwrap();
            assert_eq!(
                read_id_rgba(&r.rgba8, out.width, out.height, landed(&c, l, spec)),
                Some(want),
                "{}: frame id of {item}",
                c.name
            );
        }
        if let Ok(dir) = std::env::var("FIN_REFERENCE_PNG") {
            write_png(&dir, &c.name, out, &r.rgba8);
        }
        got.insert(c.name.clone(), blake3::hash(&r.rgba8).to_hex().to_string());
    }
    if std::env::var("FIN_BLESS_T0").as_deref() == Ok("1") {
        let doc = json!({ "renderVersion": RENDER_VERSION, "hashes": got });
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/t0.json");
        std::fs::write(path, serde_json::to_string_pretty(&doc).unwrap() + "\n").unwrap();
        return;
    }
    let changed: Vec<&String> = got
        .iter()
        .filter(|(k, v)| golden.hashes.get(*k) != Some(*v))
        .map(|(k, _)| k)
        .collect();
    assert!(
        changed.is_empty() && got.len() == golden.hashes.len(),
        "T0 hashes changed for {changed:?} (render version {}). If the change is intended, bump RENDER_VERSION \
         (fin-media) and re-bless with FIN_BLESS_T0=1; otherwise the reference drifted",
        golden.render_version
    );
    assert_eq!(
        golden.render_version, RENDER_VERSION,
        "re-bless the goldens after bumping RENDER_VERSION"
    );
}

#[test]
fn layers_it_cannot_draw_are_reported() {
    let c = cases()
        .into_iter()
        .find(|c| c.name == "skips-non-clip-and-effects")
        .unwrap();
    let mut src = Patterns {
        sources: &c.sources,
        cache: HashMap::new(),
    };
    let r = reference::render(&c.plan, &mut src, Size::new(c.out[0], c.out[1]));
    let got: Vec<(&str, &str, serde_json::Value)> = r
        .skipped
        .iter()
        .map(|s| (s.item.as_str(), s.code.as_str(), s.details.clone()))
        .collect();
    assert_eq!(
        got,
        [
            (
                "c1",
                "LAYER_UNAVAILABLE",
                json!({ "reason": "notYet", "effect": "fx1", "effectKind": "colorAdjust" })
            ),
            ("cap1", "LAYER_UNAVAILABLE", json!({ "reason": "notYet" })),
            ("c9", "ORIGINALS_MISSING", json!({ "asset": "missing" })),
        ]
    );
    // The clip with an effect is still drawn: the frame is not black.
    assert!(r.rgba8.chunks(4).any(|p| p[0] > 40));
}

#[test]
fn transitions_at_their_ends_equal_the_single_layers() {
    let all = cases();
    let by = |n: &str| all.iter().find(|c| c.name == n).unwrap();
    let render = |c: &Case| {
        let mut src = Patterns {
            sources: &c.sources,
            cache: HashMap::new(),
        };
        reference::render(&c.plan, &mut src, Size::new(c.out[0], c.out[1])).rgba8
    };
    // Progress 0 shows only the outgoing layer; crossfade and dissolve agree there and at 1.
    assert_eq!(render(by("crossfade-0.0")), render(by("dissolve-0.0")));
    assert_eq!(render(by("crossfade-1.0")), render(by("dissolve-1.0")));
    assert_ne!(
        render(by("crossfade-0.5")),
        render(by("dissolve-0.5")),
        "the two looks differ mid-way"
    );
}
