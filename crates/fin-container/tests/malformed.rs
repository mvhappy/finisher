//! Broken files (spec §6.1, plan 6A review focus 2): every one is a typed error
//! (`MEDIA_CORRUPT`, `MEDIA_LIMIT`, `UNSUPPORTED_CONTAINER`, `UNSUPPORTED_TRANSFORM`) within
//! 50 ms, never a panic, a hang or an allocation the file's bytes do not back.

mod common;

use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use common::*;
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
