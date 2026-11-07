//! `FrameReader` random access: frames in shuffled order equal a sequential decode, bit for
//! bit, across GOPs and for a VFR clip.

use std::io::Cursor;

use fin_codecs::fixture::{ClipSpec, pattern_clip};
use fin_codecs::{FrameReader, capabilities};
use fin_container::Limits;
use fin_media::testing::read_id_y;
use fin_media::{Size, frame_at};

fn open(spec: &ClipSpec) -> FrameReader<Cursor<Vec<u8>>> {
    FrameReader::open(
        Cursor::new(pattern_clip(spec).unwrap()),
        &Limits::default(),
        &capabilities(),
    )
    .unwrap()
}

#[test]
fn shuffled_random_access_equals_sequential_decode() {
    let spec = ClipSpec {
        frames: 45,
        size: Size::new(320, 180),
        keyint: 10,
        ..Default::default()
    };
    let mut seq = open(&spec);
    let sequential: Vec<_> = (0..45).map(|i| seq.frame(i).unwrap().clone()).collect();
    let mut order: Vec<usize> = (0..45).collect();
    let mut x: u64 = 0xDEAD_BEEF;
    for i in (1..order.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        order.swap(i, (x % (i as u64 + 1)) as usize);
    }
    let mut r = open(&spec);
    for &i in &order {
        let f = r.frame(i).unwrap();
        assert_eq!(f.planes, sequential[i].planes, "frame {i}");
        assert_eq!(read_id_y(&f.planes[0].data, 320, 180), Some(i as u32));
    }
    // Repeats come from the cache.
    assert_eq!(
        r.frame(order[0]).unwrap().planes,
        sequential[order[0]].planes
    );
    assert_eq!(r.frame(45).unwrap_err().code, "INVALID_RANGE");
}

#[test]
fn a_vfr_clip_reads_the_frame_frame_at_chooses() {
    let spec = ClipSpec {
        frames: 20,
        vfr: true,
        ..Default::default()
    };
    let mut r = open(&spec);
    assert!(r.info().video.as_ref().unwrap().vfr);
    let table = r.table().clone();
    for i in 0..table.len() - 1 {
        // The midpoint between frame i and i + 1 picks frame i.
        let f = table.frames();
        let mid =
            fin_core::Rational::new(f[i].pts + f[i + 1].pts, 2 * table.timescale() as i64).unwrap();
        let at = frame_at(&table, mid);
        assert_eq!(at.index, i);
        let frame = r.frame(at.index).unwrap();
        assert_eq!(read_id_y(&frame.planes[0].data, 320, 180), Some(i as u32));
    }
}
