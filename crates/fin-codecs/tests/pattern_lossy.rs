//! The frame-id code survives lossy AV1 (plan 6A Task 3, checked here because it needs the
//! encoder): every frame reads back at CRF 40 at 540p and 1080p, and at least 99 % at CRF 50.

use std::io::Cursor;

use fin_codecs::fixture::{ClipSpec, pattern_clip};
use fin_codecs::{FrameReader, capabilities};
use fin_container::Limits;
use fin_media::Size;
use fin_media::testing::read_id_y;

/// Frames whose id reads back, of `frames`.
fn readable(size: Size, crf: u8, frames: u32) -> u32 {
    let spec = ClipSpec {
        frames,
        size,
        crf,
        keyint: 30,
        preset: 12,
        seed: 4,
        ..Default::default()
    };
    let mut r = FrameReader::open(
        Cursor::new(pattern_clip(&spec).unwrap()),
        &Limits::default(),
        &capabilities(),
    )
    .unwrap();
    (0..frames as usize)
        .filter(|&i| {
            read_id_y(&r.frame(i).unwrap().planes[0].data, size.width, size.height)
                == Some(i as u32)
        })
        .count() as u32
}

#[test]
fn every_frame_reads_back_at_crf_40() {
    for size in [Size::new(960, 540), Size::new(1920, 1080)] {
        let n = 30;
        assert_eq!(readable(size, 40, n), n, "{size:?} at CRF 40");
    }
}

#[test]
fn ninety_nine_percent_read_back_at_crf_50() {
    let n = 60;
    let got = readable(Size::new(960, 540), 50, n);
    eprintln!("CRF 50, 540p: {got}/{n} frame ids read back");
    assert!(got * 100 >= n * 99, "{got}/{n} at CRF 50");
}
