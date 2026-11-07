//! dav1d with worker threads and frames in flight (spec §6.4): bit-exact against one thread,
//! read sequentially, shuffled and from the end of the stream; frames come out while packets
//! keep coming; the end of the stream drains; many multi-threaded decodes at once agree
//! (`FIN_DAV1D_STRESS` sets how many, default 50).

use std::io::Cursor;
use std::sync::Arc;

use fin_codecs::fixture::{ClipSpec, pattern_clip};
use fin_codecs::{DecodeThreads, DecodedFrame, FrameReader, capabilities, open_decoder_with};
use fin_container::{Demuxer, Limits};
use fin_media::Size;
use fin_media::testing::read_id_y;

const SIZE: Size = Size::new(640, 360);

fn clip(frames: u32) -> Vec<u8> {
    pattern_clip(&ClipSpec {
        frames,
        size: SIZE,
        keyint: 24,
        crf: 35,
        preset: 12,
        seed: 7,
        ..Default::default()
    })
    .unwrap()
}

fn reader(file: &[u8], threads: DecodeThreads) -> FrameReader<Cursor<Vec<u8>>> {
    FrameReader::open_with(
        Cursor::new(file.to_vec()),
        &Limits::default(),
        &capabilities(),
        threads,
    )
    .unwrap()
}

fn sequential(file: &[u8], threads: DecodeThreads) -> Vec<DecodedFrame> {
    let mut r = reader(file, threads);
    (0..r.table().len())
        .map(|i| r.frame(i).unwrap().clone())
        .collect()
}

fn digest(frames: &[DecodedFrame]) -> blake3::Hash {
    let mut h = blake3::Hasher::new();
    for f in frames {
        h.update(&f.pts.to_le_bytes());
        for p in &f.planes {
            h.update(&p.data);
        }
    }
    h.finalize()
}

const THREADED: [DecodeThreads; 4] = [
    DecodeThreads {
        threads: 4,
        frame_delay: 4,
    },
    DecodeThreads {
        threads: 8,
        frame_delay: 0,
    },
    DecodeThreads {
        threads: 0,
        frame_delay: 0,
    },
    DecodeThreads {
        threads: 3,
        frame_delay: 1,
    },
];

#[test]
fn frames_in_flight_decode_what_one_thread_does() {
    let file = clip(60);
    let one = sequential(&file, DecodeThreads::SINGLE);
    assert_eq!(one.len(), 60);
    for (i, f) in one.iter().enumerate() {
        assert_eq!(
            read_id_y(&f.planes[0].data, SIZE.width, SIZE.height),
            Some(i as u32)
        );
    }
    for t in THREADED {
        let many = sequential(&file, t);
        assert!(many == one, "{t:?} differs from one thread");
        // From the end backwards: each request restarts or resumes a run whose frames are
        // still in the decoder.
        let mut r = reader(&file, t);
        for i in (0..60).rev() {
            assert!(r.frame(i).unwrap() == &one[i], "{t:?} frame {i} backwards");
        }
        // Shuffled, and the last frame first (the end of the stream drains).
        let mut r = reader(&file, t);
        assert!(r.frame(59).unwrap() == &one[59]);
        let mut x: u64 = 0x5EED;
        for _ in 0..80 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let i = (x % 60) as usize;
            assert!(r.frame(i).unwrap() == &one[i], "{t:?} frame {i} shuffled");
        }
    }
}

#[test]
fn the_end_of_the_stream_drains_every_frame_in_flight() {
    let file = clip(30);
    let mut d = Demuxer::open(Cursor::new(file), &Limits::default()).unwrap();
    let track = d.video().unwrap().clone();
    let t = DecodeThreads {
        threads: 8,
        frame_delay: 8,
    };
    let mut dec = open_decoder_with(&track, &capabilities(), t).unwrap();
    let (mut out, mut buf) = (Vec::new(), Vec::new());
    for n in 0..track.sample_count() {
        d.read_sample(track.track, n as u32, &mut buf).unwrap();
        dec.send(&buf, track.sample(n).unwrap().cts).unwrap();
        while let Some(f) = dec.receive().unwrap() {
            out.push(f.pts);
        }
    }
    // With frames in flight, some are still in the decoder when the packets run out.
    let before = out.len();
    assert!(
        before < 30,
        "{before} frames before the end with 8 in flight"
    );
    dec.finish().unwrap();
    while let Some(f) = dec.receive().unwrap() {
        out.push(f.pts);
    }
    let want: Vec<i64> = (0..30).map(|n| track.sample(n).unwrap().cts).collect();
    assert_eq!(out, want);
    assert!(dec.receive().unwrap().is_none());
    // A flush starts over: the first keyframe decodes again.
    dec.flush().unwrap();
    d.read_sample(track.track, 0, &mut buf).unwrap();
    dec.send(&buf, 0).unwrap();
    dec.finish().unwrap();
    let f = dec.receive().unwrap().expect("the keyframe");
    assert_eq!(
        read_id_y(&f.planes[0].data, SIZE.width, SIZE.height),
        Some(0)
    );
}

#[test]
fn frames_come_out_while_packets_keep_coming() {
    // Frames in flight delay the output by at most the frame delay (and a temporal unit
    // dav1d holds part of): the decoder must not sit on packets until `finish`.
    let file = clip(72);
    let mut d = Demuxer::open(Cursor::new(file), &Limits::default()).unwrap();
    let track = d.video().unwrap().clone();
    for t in THREADED {
        let delay = fin_codecs::av1::Dav1dDecoder::frame_delay(t) as usize;
        let mut dec = open_decoder_with(&track, &capabilities(), t).unwrap();
        let (mut out, mut buf) = (0usize, Vec::new());
        for n in 0..track.sample_count() {
            d.read_sample(track.track, n as u32, &mut buf).unwrap();
            dec.send(&buf, track.sample(n).unwrap().cts).unwrap();
            while dec.receive().unwrap().is_some() {
                out += 1;
            }
            assert!(
                out + delay + 2 > n,
                "{t:?}: {out} frames out after {} packets",
                n + 1
            );
        }
        dec.finish().unwrap();
        while dec.receive().unwrap().is_some() {
            out += 1;
        }
        assert_eq!(out, track.sample_count(), "{t:?}");
    }
}

#[test]
fn repeated_multi_threaded_decodes_agree() {
    let runs: usize = std::env::var("FIN_DAV1D_STRESS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let file = Arc::new(clip(48));
    let want = digest(&sequential(&file, DecodeThreads::SINGLE));
    // Four decoders at a time, each with its own worker pool and frames in flight.
    let lanes = 4;
    let handles: Vec<_> = (0..lanes)
        .map(|lane| {
            let file = Arc::clone(&file);
            std::thread::spawn(move || {
                let mut done = 0;
                for run in (lane..runs).step_by(lanes) {
                    let t = THREADED[run % 3];
                    assert_eq!(digest(&sequential(&file, t)), want, "run {run} with {t:?}");
                    done += 1;
                }
                done
            })
        })
        .collect();
    let done: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
    assert_eq!(done, runs);
}
