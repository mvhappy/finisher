//! `media.proxy`'s pure parts (plan 6B-1 Task 14): tier sizes (Decision 6), GOP rules
//! (Decision 8), the key (Decision 11), the params' own checks and `table.bin` (Decision 7).

use fin_core::Rational;
use fin_media::backend::VendorClass;
use fin_media::cache::{CacheKey, PROXY_540, PROXY_1080};
use fin_media::jobs::{
    ProxyCodec, ProxyLibraryFile, ProxyParams, ProxyTier, gop, plan_key, proxy_key,
};
use fin_media::table_bin::{TABLE_VERSION, decode, encode};
use fin_media::{FrameEntry, SampleTable, Size, SyncPoint, VideoCodec, frame_at};
use serde_json::json;

const S: ProxyTier = ProxyTier::Scrub;
const P: ProxyTier = ProxyTier::Preview;

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d).unwrap()
}

#[test]
fn tiers_fit_the_coded_size_keep_the_ratio_and_never_upscale() {
    let fit = |t: ProxyTier, w, h| {
        let s = t.fit(Size::new(w, h));
        (s.width, s.height)
    };
    // 16:9 sources.
    assert_eq!(fit(S, 1920, 1080), (960, 540));
    assert_eq!(fit(P, 1920, 1080), (1920, 1080));
    assert_eq!(fit(S, 3840, 2160), (960, 540));
    assert_eq!(fit(P, 3840, 2160), (1920, 1080));
    assert_eq!(fit(S, 1280, 720), (960, 540));
    // Portrait coded pictures fit the portrait box.
    assert_eq!(fit(S, 1080, 1920), (540, 960));
    assert_eq!(fit(P, 2160, 3840), (1080, 1920));
    // Other ratios: bound by the tighter side, the other rounded to the nearest even.
    assert_eq!(fit(S, 1440, 1080), (720, 540));
    assert_eq!(fit(S, 4096, 2160), (960, 506)); // 506.25
    assert_eq!(fit(S, 2048, 858), (960, 402)); // 402.19
    assert_eq!(fit(P, 1998, 1080), (1920, 1038)); // 1037.8
    // Never upscaled: a small source keeps its size, made even.
    assert_eq!(fit(S, 320, 180), (320, 180));
    assert_eq!(fit(P, 640, 360), (640, 360));
    assert_eq!(fit(S, 321, 181), (320, 180));
    assert_eq!(fit(P, 959, 541), (958, 540));
    // Degenerate sizes stay at least 2 × 2.
    assert_eq!(fit(S, 1, 1), (2, 2));
    assert_eq!(fit(S, 20_000, 2), (960, 2));
    // Every answer is even and inside the box.
    for (w, h) in [
        (1919, 1079),
        (7680, 4320),
        (1000, 3000),
        (999, 999),
        (5000, 7),
    ] {
        for t in ProxyTier::ALL {
            let s = t.fit(Size::new(w, h));
            let b = t.bounds();
            let (bw, bh) = if h > w {
                (b.height, b.width)
            } else {
                (b.width, b.height)
            };
            assert!(s.width % 2 == 0 && s.height % 2 == 0, "{w}x{h} → {s:?}");
            assert!(s.width <= bw && s.height <= bh, "{w}x{h} → {s:?}");
            assert!(
                s.width <= w.max(2) && s.height <= h.max(2),
                "{w}x{h} → {s:?}"
            );
        }
    }
}

#[test]
fn gop_keyframe_intervals() {
    // 540: a keyframe at least every 0.25 s; 1080: at least every 0.5 s.
    assert_eq!(gop(S, r(30, 1), false, false), (7, false));
    assert_eq!(gop(P, r(30, 1), false, false), (15, false));
    assert_eq!(gop(S, r(24, 1), false, false), (6, false));
    assert_eq!(gop(P, r(24, 1), false, false), (12, false));
    assert_eq!(gop(S, r(30_000, 1001), false, false), (7, false));
    assert_eq!(gop(P, r(60, 1), false, false), (30, false));
    // SVT-AV1 keeps spec §7.1's closed GOP of at most 15 frames.
    assert_eq!(gop(P, r(60, 1), false, true), (15, false));
    assert_eq!(gop(S, r(120, 1), false, true), (15, false));
    assert_eq!(gop(P, r(24, 1), false, true), (12, false));
    // Low rates still give a keyframe every frame at least.
    assert_eq!(gop(S, r(2, 1), false, false), (1, false));
    // All-intra 540 only when the encoder was measured fast enough; never for 1080.
    assert_eq!(gop(S, r(30, 1), true, false), (1, true));
    assert_eq!(gop(P, r(30, 1), true, false), (15, false));
}

#[test]
fn the_key_is_the_cache_key_of_the_recipe_and_codec() {
    let k = proxy_key("fp", S, ProxyCodec::Av1, VendorClass::Software);
    assert_eq!(k.as_str().len(), 64);
    // The tier is the recipe, the codec its one parameter, the decoder class in the key.
    let input = fin_media::jobs::key_input("fp", S, ProxyCodec::Av1, VendorClass::Software);
    assert_eq!(input.recipe, PROXY_540.name);
    assert_eq!(input.params, json!({ "codec": "av1" }));
    assert_eq!(CacheKey::compute(&input), k);
    assert_eq!(
        fin_media::jobs::key_input("fp", P, ProxyCodec::Av1, VendorClass::Software).recipe,
        PROXY_1080.name
    );
    for other in [
        proxy_key("fp2", S, ProxyCodec::Av1, VendorClass::Software),
        proxy_key("fp", P, ProxyCodec::Av1, VendorClass::Software),
        proxy_key("fp", S, ProxyCodec::H264, VendorClass::Software),
        proxy_key("fp", S, ProxyCodec::Av1, VendorClass::Os),
    ] {
        assert_ne!(other, k);
    }
}

fn params() -> ProxyParams {
    serde_json::from_value(json!({
        "fingerprint": "fp",
        "tier": "540",
        "codec": "av1",
        "decoderClass": "software",
        "key": proxy_key("fp", S, ProxyCodec::Av1, VendorClass::Software).to_string(),
        "asset": "a1",
    }))
    .unwrap()
}

#[test]
fn params_name_one_source_and_a_key() {
    let p = params();
    assert_eq!(p.tier, S);
    p.check().unwrap();
    let mut both = p.clone();
    both.file = Some(ProxyLibraryFile {
        library: "l1".into(),
        root: "/r".into(),
        rel: json!({ "components": [] }),
        expect: json!({ "size": 1, "mtimeNs": "1" }),
        hydrate: false,
    });
    assert_eq!(both.check().unwrap_err().code, "JOB_PARAMS_INVALID");
    let mut none = p.clone();
    none.asset = None;
    assert_eq!(none.check().unwrap_err().code, "JOB_PARAMS_INVALID");
    let mut bad = p.clone();
    bad.key = "nope".into();
    assert_eq!(bad.check().unwrap_err().details["field"], "key");
    // Unknown fields and tiers are refused by serde.
    assert!(serde_json::from_value::<ProxyParams>(json!({ "tier": "720" })).is_err());
    let mut v = serde_json::to_value(&p).unwrap();
    v["path"] = json!("/etc/passwd");
    assert!(serde_json::from_value::<ProxyParams>(v).is_err());
}

fn table() -> SampleTable {
    // A VFR table with an open-GOP leading frame and a sync point no frame presents.
    let f = |pts: i64, ct: i64, dur: u32, key: bool, di: u32| FrameEntry {
        pts,
        ct,
        duration: dur,
        keyframe: key,
        decode_index: di,
        offset: 1000 + di as u64 * 10,
        size: 10 + di,
    };
    let frames = vec![
        f(0, 0, 1000, true, 0),
        f(1000, 1000, 2000, false, 2),
        f(3000, 3000, 1000, false, 1),
        f(4000, 4000, 3000, true, 3),
    ];
    let sync = vec![
        SyncPoint {
            decode_index: 0,
            ct: 0,
        },
        SyncPoint {
            decode_index: 3,
            ct: 4000,
        },
        SyncPoint {
            decode_index: 9,
            ct: -5,
        },
    ];
    SampleTable::with_sync(30_000, frames, sync).unwrap()
}

#[test]
fn table_bin_round_trips_and_keeps_frame_at() {
    let t = table();
    let bytes = encode(&t);
    assert_eq!(&bytes[..8], b"FINSTBL\0");
    assert_eq!(
        u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        TABLE_VERSION
    );
    assert_eq!(bytes.len(), 24 + 37 * 4 + 12 * 3);
    let back = decode(&bytes).unwrap();
    assert_eq!(back, t);
    for s in [r(0, 1), r(1, 60), r(1, 30), r(7, 60), r(1, 1)] {
        assert_eq!(frame_at(&back, s), frame_at(&t, s));
    }
    // The same table, the same bytes.
    assert_eq!(encode(&back), bytes);
}

#[test]
fn table_bin_refuses_what_it_did_not_write() {
    let bytes = encode(&table());
    let code = |b: &[u8]| decode(b).unwrap_err().code;
    assert_eq!(code(&bytes[..10]), "MEDIA_CORRUPT");
    assert_eq!(code(&bytes[..bytes.len() - 1]), "MEDIA_CORRUPT");
    let mut longer = bytes.clone();
    longer.push(0);
    assert_eq!(code(&longer), "MEDIA_CORRUPT");
    let mut magic = bytes.clone();
    magic[0] = b'X';
    assert_eq!(code(&magic), "MEDIA_CORRUPT");
    let mut version = bytes.clone();
    version[8] = 2;
    assert!(decode(&version).unwrap_err().message.contains("version 2"));
    // A count that claims more than the bytes hold.
    let mut count = bytes.clone();
    count[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(code(&count), "MEDIA_CORRUPT");
    // A keyframe flag that is not 0 or 1, frames out of order, a zero timescale.
    let mut flag = bytes.clone();
    flag[24 + 36] = 2;
    assert_eq!(code(&flag), "MEDIA_CORRUPT");
    let mut order = bytes.clone();
    order[24..32].copy_from_slice(&5000i64.to_le_bytes());
    assert_eq!(code(&order), "MEDIA_CORRUPT");
    let mut ts = bytes.clone();
    ts[12..16].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(code(&ts), "MEDIA_CORRUPT");
}

#[test]
fn the_host_plans_the_key_with_the_report_s_first_decoder() {
    let (k, b) = plan_key(
        "fp",
        S,
        ProxyCodec::Av1,
        VideoCodec::Av1,
        &["dav1d", "svt-av1"],
    )
    .unwrap();
    assert_eq!((b.name, b.class), ("dav1d", VendorClass::Software));
    assert_eq!(
        k,
        proxy_key("fp", S, ProxyCodec::Av1, VendorClass::Software)
    );
    // An encoder listed first, or an unknown name, is skipped.
    let (_, b) = plan_key(
        "fp",
        S,
        ProxyCodec::Av1,
        VideoCodec::Av1,
        &["svt-av1", "nope", "videotoolbox"],
    )
    .unwrap();
    assert_eq!(b.class, VendorClass::Os);
    let e = plan_key(
        "fp",
        S,
        ProxyCodec::Av1,
        VideoCodec::H264,
        &["dav1d", "svt-av1"],
    )
    .unwrap_err();
    assert_eq!(e.code, "CODEC_UNAVAILABLE");
    assert_eq!(e.details["direction"], "decode");
    assert!(e.fix.is_some());
}
