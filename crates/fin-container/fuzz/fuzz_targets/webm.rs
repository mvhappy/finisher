//! WebM/Matroska (6B-1 Task 12): any input that starts with the EBML magic (prepended when
//! missing, so every run reaches the EBML demuxer) opens or fails with a typed error; reading
//! every sample of an opened file never panics either.
#![no_main]

use std::io::Cursor;

use fin_container::{Demuxer, Limits};
use libfuzzer_sys::fuzz_target;

const TYPED: [&str; 5] = [
    "MEDIA_CORRUPT",
    "MEDIA_LIMIT",
    "UNSUPPORTED_CONTAINER",
    "UNSUPPORTED_TRANSFORM",
    "INVALID_RANGE",
];

const MAGIC: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

fuzz_target!(|data: &[u8]| {
    let bytes = if data.starts_with(&MAGIC) {
        data.to_vec()
    } else {
        [&MAGIC[..], data].concat()
    };
    let limits = Limits {
        max_box_bytes: 1 << 20,
        max_samples: 100_000,
        ..Limits::default()
    };
    match Demuxer::open(Cursor::new(bytes), &limits) {
        Err(e) => assert!(TYPED.contains(&e.code.as_str()), "untyped error {e:?}"),
        Ok(mut d) => {
            let tracks: Vec<_> = [
                d.video().map(|v| (v.track, v.sample_count())),
                d.audio().map(|a| (a.track, a.sample_count())),
            ]
            .into_iter()
            .flatten()
            .collect();
            let mut buf = Vec::new();
            for (track, n) in tracks {
                for i in 0..n.min(64) {
                    if let Err(e) = d.read_sample(track, i as u32, &mut buf) {
                        assert!(TYPED.contains(&e.code.as_str()), "untyped error {e:?}");
                    }
                }
            }
        }
    }
});
