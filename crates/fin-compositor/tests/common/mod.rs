//! The compositor corpus (`corpus/*.json`: plans and synthetic sources) and the pattern
//! provider both compositors' tests render it with.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};

use fin_compositor::{LayerIssue, SourceFrame, SourceProvider};
use fin_core::Rational;
use fin_core::eval::{FramePlan, Layer};
use fin_media::testing::{Yuv420, pattern};
use fin_media::{
    ColourTags, FrameEntry, HdrInfo, PixelFormat, Rotation, SampleTable, Size, frame_at,
};
use serde::Deserialize;
use serde_json::json;

pub const CORPUS: [&str; 5] = [
    include_str!("../corpus/basic.json"),
    include_str!("../corpus/blend.json"),
    include_str!("../corpus/transitions.json"),
    include_str!("../corpus/geometry.json"),
    include_str!("../corpus/colour.json"),
];

/// The cases that convert colour (wide-gamut SDR and HDR, `RENDER_VERSION` 2): the soft clip
/// and the tone mapping (spec §10.1).
pub const COLOUR_CASES: [&str; 8] = [
    "bt2020-gamut",
    "p3-gamut",
    "pq-default-peak",
    "pq-clli-4000",
    "pq-mdcv-600",
    "pq-bt709-primaries",
    "hlg-1000",
    "hlg-full-range",
];

#[derive(Debug, Deserialize)]
pub struct PatternSpec {
    pub width: u32,
    pub height: u32,
    pub seed: u64,
}

#[derive(Debug, Deserialize)]
pub struct SourceSpec {
    pub pattern: PatternSpec,
    pub rate: Rational,
    pub rotation: Rotation,
    pub colour: ColourTags,
    /// HDR metadata (`clli`, `mdcv`): the tone mapping's source peak.
    #[serde(default)]
    pub hdr: Option<HdrInfo>,
}

#[derive(Debug, Deserialize)]
pub struct Case {
    pub name: String,
    pub out: [u32; 2],
    pub plan: FramePlan,
    pub sources: BTreeMap<String, SourceSpec>,
    #[serde(default)]
    pub ids: Vec<String>,
}

/// A CFR table of 10,000 frames at `rate`.
pub fn table(rate: Rational) -> SampleTable {
    let (ts, d) = (rate.num() as u32, rate.den() as u32);
    let frames = (0..10_000u32)
        .map(|i| FrameEntry {
            ct: i as i64 * d as i64,
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
pub struct Patterns<'a> {
    pub sources: &'a BTreeMap<String, SourceSpec>,
    pub cache: HashMap<(String, u32), Yuv420>,
}

impl Patterns<'_> {
    pub fn index(&self, l: &Layer) -> Option<u32> {
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
            hdr: spec.hdr,
            rotation: spec.rotation,
            pixel_aspect: Rational::from_int(1),
        })
    }
}

pub fn cases() -> Vec<Case> {
    CORPUS
        .iter()
        .flat_map(|c| serde_json::from_str::<Vec<Case>>(c).unwrap())
        .collect()
}

impl Case {
    pub fn size(&self) -> Size {
        Size::new(self.out[0], self.out[1])
    }

    pub fn patterns(&self) -> Patterns<'_> {
        Patterns {
            sources: &self.sources,
            cache: HashMap::new(),
        }
    }
}

pub fn write_png(dir: &str, name: &str, size: Size, rgba: &[u8]) {
    let f = std::fs::File::create(format!("{dir}/{name}.png")).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), size.width, size.height);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}
