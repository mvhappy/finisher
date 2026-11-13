//! The CPU reference compositor against its corpus (spec §13.2, T0): every case's output
//! BLAKE3 equals `golden/t0.json` on every OS and on wasm32 (`wasm32-wasip1`); frame ids read
//! from the output equal `frame_at`'s choice; layers it cannot draw are reported.
//!
//! `FIN_BLESS_T0=1` rewrites the goldens (native only) after a `RENDER_VERSION` bump, and refuses
//! moved hashes at the goldens' own version (`FIN_BLESS_T0=same-version` re-blesses a version
//! not yet committed); `FIN_REFERENCE_PNG=<dir>` writes each output as a PNG for inspection.

mod common;

use std::collections::{BTreeMap, HashMap};

use common::{COLOUR_CASES, Case, Patterns, SourceSpec, cases, write_png};
use fin_compositor::{
    LayerIssue, SourceFrame, SourceProvider, base_box, layer_matrix, reference, transform_point,
};
use fin_core::Rational;
use fin_core::eval::Layer;
use fin_media::testing::{Rect, read_id_rgba};
use fin_media::{ColourTags, PixelFormat, RENDER_VERSION, Rotation, Size, display_size};
use serde::Deserialize;
use serde_json::json;

const GOLDEN: &str = include_str!("golden/t0.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Golden {
    render_version: u32,
    hashes: BTreeMap<String, String>,
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
    let bless = std::env::var("FIN_BLESS_T0").ok();
    if let Some(mode) = bless
        .as_deref()
        .filter(|m| *m == "1" || *m == "same-version")
    {
        // A plain bless refuses moved hashes at the golden's own version: bump first.
        if let Err(e) = t0_verdict(&golden, &got, RENDER_VERSION) {
            assert!(
                mode == "same-version" || golden.render_version != RENDER_VERSION,
                "{e}; FIN_BLESS_T0=same-version re-blesses an unreleased version"
            );
        }
        let doc = json!({ "renderVersion": RENDER_VERSION, "hashes": got });
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/t0.json");
        std::fs::write(path, serde_json::to_string_pretty(&doc).unwrap() + "\n").unwrap();
        return;
    }
    if let Err(e) = t0_verdict(&golden, &got, RENDER_VERSION) {
        panic!("{e}");
    }
}

/// The T0 rule (spec §13.4): every hash equals the golden's, and the golden was blessed at
/// `version`. Moved hashes fail whatever the version; a bumped version with unchanged
/// goldens fails until they are re-blessed.
fn t0_verdict(golden: &Golden, got: &BTreeMap<String, String>, version: u32) -> Result<(), String> {
    let changed: Vec<&String> = got
        .iter()
        .filter(|(k, v)| golden.hashes.get(*k) != Some(*v))
        .map(|(k, _)| k)
        .collect();
    let gone: Vec<&String> = golden
        .hashes
        .keys()
        .filter(|k| !got.contains_key(*k))
        .collect();
    if !changed.is_empty() || !gone.is_empty() {
        return Err(format!(
            "T0 hashes changed for {changed:?}, gone {gone:?} (render version {}). If the change is intended, bump \
             RENDER_VERSION (fin-media) and re-bless with FIN_BLESS_T0=1; otherwise the reference drifted",
            golden.render_version
        ));
    }
    if golden.render_version != version {
        return Err(format!(
            "the goldens are at render version {}, RENDER_VERSION is {version}: re-bless the goldens \
             after bumping RENDER_VERSION",
            golden.render_version
        ));
    }
    Ok(())
}

/// A T0 change without a `RENDER_VERSION` bump fails, and so does a bump whose goldens were
/// not re-blessed (spec §13.4). The corpus test applies this rule to the real goldens.
#[test]
fn t0_change_without_bump_fails() {
    let golden = Golden {
        render_version: 2,
        hashes: BTreeMap::from([("a".into(), "1".into()), ("b".into(), "2".into())]),
    };
    let same = golden.hashes.clone();
    assert_eq!(t0_verdict(&golden, &same, 2), Ok(()));
    let mut moved = same.clone();
    moved.insert("b".into(), "3".into());
    let e = t0_verdict(&golden, &moved, 2).unwrap_err();
    assert!(e.contains("bump") && e.contains("[\"b\"]"), "{e}");
    // Bumping alone does not pass: the goldens must be re-blessed at the new version.
    assert!(t0_verdict(&golden, &moved, 3).is_err());
    assert!(
        t0_verdict(&golden, &same, 3)
            .unwrap_err()
            .contains("re-bless")
    );
    // A case that disappears is a change too.
    let mut fewer = same.clone();
    fewer.remove("a");
    assert!(t0_verdict(&golden, &fewer, 2).is_err());
    // The committed goldens are at the current version.
    let committed: Golden = serde_json::from_str(GOLDEN).unwrap();
    assert_eq!(committed.render_version, RENDER_VERSION);
}

/// Wide-gamut SDR and HDR sources (spec §10.1) at `RENDER_VERSION` 2: the gamut compression
/// replaced 6A's hard clip (the BT.2020 case moved off its version-1 hash) and HDR is tone-mapped by
/// the BT.2390 EETF, its source peak from the metadata. Each case's hash is in the goldens.
#[test]
fn wide_gamut_softclip_t0() {
    let golden: Golden = serde_json::from_str(GOLDEN).unwrap();
    const { assert!(RENDER_VERSION >= 2) };
    let all = cases();
    let render = |name: &str| {
        let c = all.iter().find(|c| c.name == name).unwrap();
        reference::render(&c.plan, &mut c.patterns(), c.size()).rgba8
    };
    let mut seen = BTreeMap::new();
    for name in COLOUR_CASES {
        let px = render(name);
        let hash = blake3::hash(&px).to_hex().to_string();
        assert_eq!(golden.hashes.get(name), Some(&hash), "{name}: T0 golden");
        // The pictures are not flat (a broken conversion tends to black or white).
        let levels: std::collections::BTreeSet<u8> = px.chunks(4).map(|p| p[1]).collect();
        assert!(levels.len() > 64, "{name}: {} green levels", levels.len());
        assert!(
            seen.insert(hash, name).is_none(),
            "{name} renders like another case"
        );
    }
    // 6A hard-clipped the BT.2020 case to this hash (render version 1).
    assert_ne!(
        golden.hashes["bt2020-gamut"],
        "6a1bc264337a6e3866920f0c28263e020fe530c47b7db4167cd553292ef2ca79"
    );
}

/// Owner decision 2026-11-09 (spec §10.1): what Rec.709 can show passes through. The Display P3
/// case, tagged Rec.709 instead, renders every neutral pixel (white, the greys, black) to the
/// same bytes.
#[test]
fn wide_gamut_keeps_what_rec709_shows() {
    for name in ["p3-gamut", "bt2020-gamut"] {
        let mut c = cases().into_iter().find(|c| c.name == name).unwrap();
        let wide = reference::render(&c.plan, &mut c.patterns(), c.size()).rgba8;
        for spec in c.sources.values_mut() {
            spec.colour.primaries = fin_media::Primaries::Bt709;
        }
        let narrow = reference::render(&c.plan, &mut c.patterns(), c.size()).rgba8;
        let (mut neutral, mut moved) = (0, 0);
        for (w, n) in wide.chunks(4).zip(narrow.chunks(4)) {
            if n[0] == n[1] && n[1] == n[2] {
                assert_eq!(w, n, "{name}: a neutral pixel moved");
                neutral += 1;
            } else if w != n {
                moved += 1;
            }
        }
        assert!(neutral > 1000, "{name}: {neutral} neutral pixels");
        // The coloured pixels are different colours in the two gamuts, so they differ; the
        // neutral ones are the check (white itself: fin-media's tests/gamut.rs).
        assert!(moved > 0);
    }
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

/// A provider of one fixed picture.
struct Fixed {
    format: PixelFormat,
    planes: [Vec<u8>; 3],
    size: Size,
}

impl SourceProvider for Fixed {
    fn frame(&mut self, _: &Layer) -> Result<SourceFrame<'_>, LayerIssue> {
        Ok(SourceFrame {
            format: self.format,
            size: self.size,
            planes: [&self.planes[0], &self.planes[1], &self.planes[2]],
            strides: [
                self.size.width as usize,
                self.size.width.div_ceil(2) as usize,
                self.size.width.div_ceil(2) as usize,
            ],
            colour: ColourTags::BT709,
            hdr: None,
            rotation: Rotation::R0,
            pixel_aspect: Rational::from_int(1),
        })
    }
}

/// Review item 8: pictures the reference cannot read (other formats, a monochrome picture
/// with empty chroma, planes shorter than their size) are skipped, never a panic.
#[test]
fn unreadable_pictures_are_skipped_not_a_panic() {
    let c = cases()
        .into_iter()
        .find(|c| c.name == "single-clip")
        .unwrap();
    let size = Size::new(320, 180);
    for (format, planes) in [
        (
            PixelFormat::I444,
            [
                vec![0; 320 * 180],
                vec![128; 320 * 180],
                vec![128; 320 * 180],
            ],
        ),
        (
            PixelFormat::I420p10,
            [
                vec![0; 2 * 320 * 180],
                vec![0; 2 * 160 * 90],
                vec![0; 2 * 160 * 90],
            ],
        ),
        (PixelFormat::I420, [vec![0; 320 * 180], vec![], vec![]]),
        (
            PixelFormat::I420,
            [vec![0; 10], vec![128; 160 * 90], vec![128; 160 * 90]],
        ),
    ] {
        let r = reference::render(
            &c.plan,
            &mut Fixed {
                format,
                planes,
                size,
            },
            size,
        );
        assert_eq!(r.skipped.len(), 1, "{format:?}");
        assert_eq!(r.skipped[0].code, "UNSUPPORTED_PIXEL_FORMAT");
        assert!(
            r.rgba8.chunks(4).all(|p| p[..3] == [0, 0, 0]),
            "nothing drawn"
        );
    }
}
