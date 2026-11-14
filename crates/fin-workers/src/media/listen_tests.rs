//! The listen's sources (6C Task 21 re-check): decoders pooled per asset within a mix, so many
//! clips of one original at increasing offsets decode it about once; a cached entry that fails
//! to open after verifying falls back to the decode.
// Fixtures are written and opened by path (`scripts/clippy-allow.json`: `fin-workers/src` tests).
#![allow(clippy::disallowed_methods)]

use super::*;
use fin_codecs::audio::testing::wav::{WavFormat, wav};
use fin_core::Interp;
use fin_core::eval::{GainSegment, PanSegment};
use fin_media::jobs::{ManifestAsset, ManifestFonts, ManifestPcm};

fn s(n: i64) -> Rational {
    Rational::from_int(n)
}

/// A 20 s stereo 1 kHz WAV in `dir`.
fn original(dir: &Path) -> OriginalFile {
    let dir = std::fs::canonicalize(dir).unwrap();
    let path = dir.join("long.wav");
    let pcm: Vec<f32> = (0..20 * 48_000)
        .flat_map(|i| {
            let v =
                (0.1 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0).sin()) as f32;
            [v, v]
        })
        .collect();
    std::fs::write(&path, wav(&pcm, 2, 48_000, WavFormat::Pcm16, None)).unwrap();
    // The facts as the library's native source lists them (what the host's manifest records).
    use fin_library::Source;
    let src = fin_library::source::NativeSource::new(&dir);
    let root = src.open_dir(&fin_library::RawRel::root()).unwrap();
    let e = src.stat_child(&root, b"long.wav").unwrap().expect("listed");
    let expect = fin_library::Expect {
        size: e.size,
        mtime_ns: e.mtime_ns,
        file_id: e.file_id,
        dev: None,
    };
    let wire = serde_json::to_value(&expect).unwrap();
    OriginalFile {
        path: path.to_str().unwrap().into(),
        size: wire["size"].as_u64().unwrap(),
        mtime_ns: wire["mtimeNs"].as_str().unwrap().into(),
        file_id: wire
            .get("fileId")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

fn manifest(o: OriginalFile, pcm: Option<ManifestPcm>) -> RenderManifest {
    RenderManifest {
        version: MANIFEST_VERSION,
        assets: vec![ManifestAsset {
            asset: "a1".into(),
            kind: fin_core::AssetKind::Media,
            fingerprint: "fp".into(),
            original: Some(o),
            proxies: Vec::new(),
        }],
        fonts: ManifestFonts::default(),
        pcm: pcm.into_iter().map(|p| ("a1".to_string(), p)).collect(),
    }
}

/// Item `k` of `a1` over `[k·len, (k + 1)·len)` from source time `k·len`: consecutive cuts of
/// the original, `len` seconds each (not a multiple of the mixer's blocks: a cut starts in the
/// block where the one before it ends).
fn cuts(n: i64, len: Rational) -> AudioPlan {
    let at = |k: i64| Rational::from_int(k).checked_mul(len).unwrap();
    let items = (0..n)
        .map(|k| {
            let (from, to) = (at(k), at(k + 1));
            AudioItem {
                item: format!("c{k}"),
                track: "t2".into(),
                via: vec![],
                asset: "a1".into(),
                from,
                to,
                source_from: from,
                speed: s(1),
                gain: vec![GainSegment {
                    from,
                    to,
                    db: [0.0, 0.0],
                    interp: Interp::Linear,
                    u: [0.0, 1.0],
                    shapes: vec![],
                }],
                pan: vec![PanSegment {
                    from,
                    to,
                    pan: [0.0, 0.0],
                    interp: Interp::Linear,
                    u: [0.0, 1.0],
                }],
                effects: vec![],
            }
        })
        .collect();
    AudioPlan {
        timeline: "tl1".into(),
        from: s(0),
        to: at(n),
        items,
    }
}

fn run_pass(opener: &mut Opener<'_>, plan: &AudioPlan) -> fin_media::jobs::ListenStats {
    let (plan, sources) = opener.sources(plan);
    let mut pass = Pass::new(&plan, sources).unwrap();
    let (start, end) = range(&plan).unwrap();
    let mut at = start;
    while at < end {
        let n = (end - at).min(48_000) as usize;
        pass.render(n).unwrap();
        at += n as i64;
    }
    pass.finish()
}

#[test]
fn cuts_of_one_original_at_increasing_offsets_decode_it_about_once() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manifest(original(tmp.path()), None);
    let mut opener = Opener {
        manifest: &m,
        confine: None,
        verdicts: BTreeMap::new(),
        counters: Rc::default(),
    };
    // 25 cuts of 0.7 s: 17.5 s of the original.
    let st = run_pass(&mut opener, &cuts(25, Rational::new(7, 10).unwrap()));
    assert!(st.silences.is_empty(), "{:?}", st.silences);
    // A source is given back when the mixer drops it, after the block its item ended in, so the
    // cuts sharing one 1 s block (up to 3 of 0.7 s) hold a decoder each: a few decoders leapfrog
    // forward, each about the 17.5 s once, not 0 + 0.7 + … + 16.8 s (≈ 210 s) from 25 decoders.
    // The bound does not grow with the number of cuts.
    assert!(
        opener.decoders_opened() <= 3,
        "{} decoders",
        opener.decoders_opened()
    );
    assert!(
        opener.frames_decoded() <= 3 * 18 * 48_000,
        "{} frames decoded",
        opener.frames_decoded()
    );
}

#[test]
fn a_cached_entry_that_fails_to_open_falls_back_to_the_decode() {
    let tmp = tempfile::tempdir().unwrap();
    let gone = ManifestPcm {
        key: "0".repeat(64),
        dir: std::fs::canonicalize(tmp.path())
            .unwrap()
            .join("gone")
            .to_str()
            .unwrap()
            .into(),
    };
    let m = manifest(original(tmp.path()), Some(gone));
    // As if the entry verified, then went before its open.
    let mut opener = Opener {
        manifest: &m,
        confine: None,
        verdicts: BTreeMap::from([("a1".to_string(), Verdict::Cached)]),
        counters: Rc::default(),
    };
    let st = run_pass(&mut opener, &cuts(3, s(1)));
    assert!(st.integrated.is_some(), "the asset played silent");
    assert!(st.silences.is_empty(), "{:?}", st.silences);
    assert_eq!((opener.count(true), opener.count(false)), (0, 1));
    assert!(opener.missing().is_empty());
}
