//! Broken files (spec §6.1, plan 6A review focus 2): every one is a typed error
//! (`MEDIA_CORRUPT`, `MEDIA_LIMIT`, `UNSUPPORTED_CONTAINER`, `UNSUPPORTED_TRANSFORM`) within
//! 50 ms, never a panic, a hang or an allocation the file's bytes do not back.

mod common;

use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use common::*;
use fin_container::mux::knobs::{FileKnobs, TrackKnobs};
use fin_container::{Demuxer, Limits};
use fin_core::CoreError;

const TYPED: [&str; 4] = [
    "MEDIA_CORRUPT",
    "MEDIA_LIMIT",
    "UNSUPPORTED_CONTAINER",
    "UNSUPPORTED_TRANSFORM",
];

/// Opens `bytes`, failing the test on a panic, an untyped error or a slow answer.
fn attempt(bytes: &[u8], limits: &Limits, what: &str) -> Result<(), CoreError> {
    let start = Instant::now();
    let got = catch_unwind(AssertUnwindSafe(|| {
        Demuxer::open(Cursor::new(bytes.to_vec()), limits).map(|_| ())
    }));
    let took = start.elapsed();
    let got = got.unwrap_or_else(|_| panic!("{what}: the demuxer panicked"));
    if let Err(e) = &got {
        assert!(
            TYPED.contains(&e.code.as_str()),
            "{what}: untyped error {e:?}"
        );
    }
    // Generous under debug builds and loaded CI machines; the work is microseconds.
    assert!(
        took < Duration::from_millis(if cfg!(debug_assertions) { 250 } else { 50 }),
        "{what}: took {took:?}"
    );
    got
}

fn small_file() -> Vec<u8> {
    with_audio(12, 312)
}

#[test]
fn truncated_at_every_box_boundary_and_every_header_byte() {
    let bytes = small_file();
    assert!(attempt(&bytes, &Limits::default(), "whole").is_ok());
    let mut cuts = boundaries(&bytes);
    let moov_end = cuts
        .iter()
        .copied()
        .find(|&c| c > find_box(&bytes, b"moov"))
        .unwrap();
    cuts.extend(0..moov_end.min(4096));
    for cut in cuts {
        if cut >= bytes.len() {
            continue;
        }
        assert!(
            attempt(&bytes[..cut], &Limits::default(), &format!("cut at {cut}")).is_err(),
            "a file cut at {cut} of {} opened",
            bytes.len()
        );
    }
}

fn patch_u32(bytes: &mut [u8], at: usize, v: u32) {
    bytes[at..at + 4].copy_from_slice(&v.to_be_bytes());
}

#[test]
fn sizes_past_the_end_and_too_small() {
    let bytes = small_file();
    for kind in [b"moov", b"trak", b"stbl", b"stsz", b"stts", b"mdat"] {
        let at = find_box(&bytes, kind);
        for size in [u32::MAX, (bytes.len() - at + 1) as u32, 7, 2] {
            let mut b = bytes.clone();
            patch_u32(&mut b, at, size);
            let e = attempt(&b, &Limits::default(), &format!("{kind:?} size {size}")).unwrap_err();
            assert_eq!(e.code, "MEDIA_CORRUPT", "{kind:?} size {size}: {e:?}");
        }
    }
    // A 64-bit size past the end.
    let mut b = bytes.clone();
    let at = find_box(&b, b"moov");
    patch_u32(&mut b, at, 1);
    let e = attempt(&b, &Limits::default(), "largesize").unwrap_err();
    assert_eq!(e.code, "MEDIA_CORRUPT");
}

#[test]
fn chunk_offsets_past_the_end() {
    let mut b = small_file();
    let at = find_box(&b, b"stco");
    // version/flags, count, then the first offset.
    patch_u32(&mut b, at + 16, u32::MAX - 10);
    assert_eq!(
        attempt(&b, &Limits::default(), "stco").unwrap_err().code,
        "MEDIA_CORRUPT"
    );
}

#[test]
fn declared_counts_beyond_the_box() {
    let bytes = small_file();
    for (kind, count_at) in [
        (b"stsz", 16),
        (b"stts", 12),
        (b"stco", 12),
        (b"stsc", 12),
        (b"stss", 12),
        (b"elst", 12),
    ] {
        let Some(at) = bytes.windows(4).position(|w| w == kind).map(|p| p - 4) else {
            continue;
        };
        for count in [u32::MAX, 1_000_000, 20_000] {
            let mut b = bytes.clone();
            patch_u32(&mut b, at + count_at, count);
            let e =
                attempt(&b, &Limits::default(), &format!("{kind:?} count {count}")).unwrap_err();
            assert!(
                ["MEDIA_CORRUPT", "MEDIA_LIMIT"].contains(&e.code.as_str()),
                "{kind:?} {count}: {e:?}"
            );
        }
    }
    // stsz with a count past the sample limit is MEDIA_LIMIT before anything is allocated.
    let mut b = bytes.clone();
    let at = find_box(&b, b"stsz");
    patch_u32(&mut b, at + 16, u32::MAX);
    assert_eq!(
        attempt(&b, &Limits::default(), "stsz max")
            .unwrap_err()
            .code,
        "MEDIA_LIMIT"
    );
}

#[test]
fn box_count_depth_and_box_size_limits() {
    let bytes = small_file();
    let tight = |f: fn(&mut Limits)| {
        let mut l = Limits::default();
        f(&mut l);
        attempt(&bytes, &l, "limits").unwrap_err()
    };
    assert_eq!(tight(|l| l.max_boxes = 10).code, "MEDIA_LIMIT");
    assert_eq!(tight(|l| l.max_depth = 3).code, "MEDIA_LIMIT");
    assert_eq!(tight(|l| l.max_box_bytes = 64).code, "MEDIA_LIMIT");
    assert_eq!(tight(|l| l.max_samples = 5).code, "MEDIA_LIMIT");
    // Ten thousand empty `free` boxes before the moov count against max_boxes.
    let mut many = Vec::new();
    many.extend_from_slice(&bytes[..find_box(&bytes, b"moov")]);
    for _ in 0..10_000 {
        many.extend_from_slice(&[0, 0, 0, 8, b'f', b'r', b'e', b'e']);
    }
    many.extend_from_slice(&bytes[find_box(&bytes, b"moov")..]);
    let l = Limits {
        max_boxes: 5_000,
        ..Limits::default()
    };
    assert_eq!(
        attempt(&many, &l, "many boxes").unwrap_err().code,
        "MEDIA_LIMIT"
    );
}

#[test]
fn a_box_of_size_zero_runs_to_its_parent_end() {
    // A trailing `free` of size 0 reaches the end of the file: fine.
    let mut b = small_file();
    b.extend_from_slice(&[0, 0, 0, 0, b'f', b'r', b'e', b'e', 1, 2, 3]);
    assert!(attempt(&b, &Limits::default(), "size 0 at the end").is_ok());
    // An `stbl` child of size 0 swallows the rest of the stbl, leaving it without stco.
    let bytes = small_file();
    let mut b = bytes.clone();
    let at = find_box(&b, b"stsc");
    patch_u32(&mut b, at, 0);
    assert!(attempt(&b, &Limits::default(), "size 0 inside").is_err());
}

#[test]
fn random_mutations_never_panic() {
    let bytes = small_file();
    let moov = find_box(&bytes, b"moov");
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let cases: usize = std::env::var("FIN_MUTATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3_000);
    for case in 0..cases {
        let mut b = bytes.clone();
        for _ in 0..1 + next() % 4 {
            let at = moov + (next() as usize) % (b.len() - moov);
            b[at] = match next() % 4 {
                0 => 0,
                1 => 0xff,
                2 => b[at] ^ (1 << (next() % 8)),
                _ => next() as u8,
            };
        }
        let _ = attempt(&b, &Limits::default(), &format!("mutation {case}"));
    }
}

/// Reviewer repro (6A-1 review, blocker 1): a small file whose 1,024 full-length edits would
/// present 30,000 samples 1,024 times is `MEDIA_LIMIT` before the frames are stored.
#[test]
fn edits_that_repeat_the_media_are_capped() {
    let n = 30_000u32;
    let frames: Vec<_> = (0..n)
        .map(|i| (i as i64, i as i64, 1, i % 30 == 0))
        .collect();
    let mut cfg = video(fin_media::Rotation::R0);
    cfg.timescale = 30;
    let knobs = TrackKnobs {
        edits: Some(vec![(n as u64, Some(0)); 1024]),
        ..Default::default()
    };
    let bytes = write_sized(cfg, knobs, &frames, 1);
    assert!(
        bytes.len() < 100_000,
        "a small file ({} bytes)",
        bytes.len()
    );
    let start = Instant::now();
    let e = Demuxer::open(Cursor::new(bytes.clone()), &Limits::default())
        .err()
        .unwrap();
    assert_eq!(e.code, "MEDIA_LIMIT", "{e:?}");
    assert_eq!(e.details["limit"], "maxPresentedFrames");
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "took {:?}",
        start.elapsed()
    );
    // Under a tighter limit it fails at once; with room, three repeats present 90,000 frames.
    let tight = Limits {
        max_samples: 50_000,
        ..Limits::default()
    };
    assert_eq!(
        attempt(&bytes, &tight, "tight").unwrap_err().code,
        "MEDIA_LIMIT"
    );
    let mut cfg = video(fin_media::Rotation::R0);
    cfg.timescale = 30;
    let three = TrackKnobs {
        edits: Some(vec![(n as u64, Some(0)); 3]),
        ..Default::default()
    };
    let d = Demuxer::open(
        Cursor::new(write_sized(cfg, three, &frames, 1)),
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(d.video().unwrap().table.len(), 3 * n as usize);
}

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    [&((body.len() + 8) as u32).to_be_bytes()[..], kind, body].concat()
}

fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = flags.to_be_bytes();
    b[0] = version;
    bx(kind, &[&b[..], body].concat())
}

/// A fragmented file (one real fragment) with `moof` appended: `tfhd` (track 1, base is the
/// moof, default duration 1000 and size `size`), an optional `tfdt` v1 and a `trun` of
/// `count` samples with no per-sample fields and data offset 0.
fn with_fragment(size: u32, count: u32, tfdt: Option<u64>) -> Vec<u8> {
    let mut bytes = write(
        video(fin_media::Rotation::R0),
        TrackKnobs::default(),
        FileKnobs {
            fragment_every: Some(1),
        },
        &cfr_frames(1),
    );
    let tfhd = full(
        b"tfhd",
        0,
        0x02_0000 | 0x08 | 0x10,
        &[
            1u32.to_be_bytes(),
            1000u32.to_be_bytes(),
            size.to_be_bytes(),
        ]
        .concat(),
    );
    let mut traf = tfhd;
    if let Some(t) = tfdt {
        traf.extend(full(b"tfdt", 1, 0, &t.to_be_bytes()));
    }
    traf.extend(full(
        b"trun",
        0,
        0x1,
        &[count.to_be_bytes(), 0i32.to_be_bytes()].concat(),
    ));
    let moof = bx(
        b"moof",
        &[full(b"mfhd", 0, 0, &9u32.to_be_bytes()), bx(b"traf", &traf)].concat(),
    );
    bytes.extend(moof);
    bytes
}

/// Reviewer repro (blocker 2): a `trun` with no per-sample fields and default size 0 declared
/// 10 M samples in 748 bytes.
#[test]
fn zero_size_fragment_samples_are_refused_and_counts_capped_by_the_file() {
    let huge = with_fragment(0, 10_000_000, None);
    assert!(huge.len() < 2_000);
    assert_eq!(
        attempt(&huge, &Limits::default(), "10M zero-size")
            .unwrap_err()
            .code,
        "MEDIA_LIMIT"
    );
    let sized = with_fragment(1, 10_000_000, None);
    assert_eq!(
        attempt(&sized, &Limits::default(), "10M one-byte")
            .unwrap_err()
            .code,
        "MEDIA_LIMIT"
    );
    let few = with_fragment(0, 5, None);
    let e = attempt(&few, &Limits::default(), "5 zero-size").unwrap_err();
    assert_eq!(e.code, "MEDIA_CORRUPT", "{e:?}");
}

/// Review item 7: decode times near `i64::MAX` are `MEDIA_CORRUPT`, not an overflow.
#[test]
fn fragment_times_that_overflow_are_corrupt() {
    for t in [i64::MAX as u64 - 10, i64::MAX as u64, u64::MAX] {
        let b = with_fragment(1, 2, Some(t));
        assert_eq!(
            attempt(&b, &Limits::default(), "tfdt").unwrap_err().code,
            "MEDIA_CORRUPT",
            "tfdt {t}"
        );
    }
}
