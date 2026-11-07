//! The nearest-frame rule (spec §10.2), `decode_start`, the untagged colour rule and display
//! sizes.

use fin_core::Rational;
use fin_media::{
    ChromaSiting, ColourTags, FrameEntry, Matrix, Primaries, Rotation, SampleTable, Size,
    SyncPoint, Transfer, decode_start, display_size, frame_at,
};
use proptest::prelude::*;

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d).unwrap()
}

fn add(a: Rational, b: Rational) -> Rational {
    a.checked_add(b).unwrap()
}

fn sub(a: Rational, b: Rational) -> Rational {
    a.checked_sub(b).unwrap()
}

fn entry(pts: i64, duration: u32, decode_index: u32, keyframe: bool) -> FrameEntry {
    FrameEntry {
        pts,
        duration,
        keyframe,
        decode_index,
        offset: 0,
        size: 1,
    }
}

/// `n` frames of `duration` ticks from `start`, all keyframes.
fn cfr(timescale: u32, duration: u32, n: usize, start: i64) -> SampleTable {
    let frames = (0..n)
        .map(|i| entry(start + i as i64 * duration as i64, duration, i as u32, true))
        .collect();
    SampleTable::new(timescale, frames).unwrap()
}

fn table_from_durations(timescale: u32, start: i64, durations: &[u32]) -> SampleTable {
    let mut pts = start;
    let frames = durations
        .iter()
        .enumerate()
        .map(|(i, &d)| {
            let e = entry(pts, d, i as u32, i % 5 == 0);
            pts += d as i64;
            e
        })
        .collect();
    SampleTable::new(timescale, frames).unwrap()
}

/// The reference: the frame whose pts is nearest to `s`, the earlier on a tie.
fn linear_nearest(t: &SampleTable, s: Rational) -> usize {
    let mut best = 0;
    let mut best_d: Option<Rational> = None;
    for i in 0..t.len() {
        let d = sub(s, t.pts_seconds(i));
        let d = if d.is_negative() {
            d.checked_neg().unwrap()
        } else {
            d
        };
        if best_d.is_none_or(|b| d < b) {
            best = i;
            best_d = Some(d);
        }
    }
    best
}

fn midpoint(t: &SampleTable, i: usize) -> Rational {
    let f = t.frames();
    r(f[i].pts + f[i + 1].pts, 2 * t.timescale() as i64)
}

#[test]
fn cfr_rates_exact_midpoint_and_one_tick_either_side() {
    // (timescale, frame duration): 24, 25, 30000/1001 and 60 fps.
    for (ts, dur) in [
        (12_288u32, 512u32),
        (25_000, 1_000),
        (30_000, 1_001),
        (6_000, 100),
    ] {
        let t = cfr(ts, dur, 50, 0);
        let tick = r(1, ts as i64);
        for i in 0..49 {
            assert_eq!(
                frame_at(&t, t.pts_seconds(i)).index,
                i,
                "on pts {i} at {ts}"
            );
            let m = midpoint(&t, i);
            assert_eq!(
                frame_at(&t, m).index,
                i,
                "midpoint {i} at {ts} goes to the earlier"
            );
            assert_eq!(
                frame_at(&t, add(m, tick)).index,
                i + 1,
                "one tick after {i} at {ts}"
            );
            assert_eq!(
                frame_at(&t, sub(m, tick)).index,
                i,
                "one tick before {i} at {ts}"
            );
            assert_eq!(frame_at(&t, m).index, linear_nearest(&t, m));
        }
    }
}

#[test]
fn cfr_is_the_last_frame_with_pts_before_s_plus_half_a_frame() {
    let t = cfr(30_000, 1_001, 40, 0);
    let d = r(1_001, 30_000);
    for k in 0..400 {
        let s = r(k, 300);
        let want = (0..t.len())
            .rev()
            .find(|&i| t.pts_seconds(i) < add(s, r(d.num(), 2 * d.den())))
            .unwrap_or(0);
        assert_eq!(frame_at(&t, s).index, want, "s = {s}");
    }
}

#[test]
fn half_speed_shows_every_frame_twice() {
    let t = cfr(24, 1, 30, 0);
    let got: Vec<usize> = (0..40).map(|k| frame_at(&t, r(k, 48)).index).collect();
    let want: Vec<usize> = (0..40).map(|k| (k / 2) as usize).collect();
    assert_eq!(got, want);
}

#[test]
fn vfr_with_random_durations_matches_the_linear_scan() {
    let mut x: u64 = 0x2545_F491_4F6C_DD1D;
    let durations: Vec<u32> = (0..200)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            1 + (x % 97) as u32
        })
        .collect();
    let t = table_from_durations(90_000, 0, &durations);
    for k in -20..(t.end_ticks() + 20) {
        let s = r(k, 90_000);
        assert_eq!(frame_at(&t, s).index, linear_nearest(&t, s), "tick {k}");
    }
}

#[test]
fn a_negative_first_pts_and_out_of_range() {
    // An edit list moved the first frame to −2 ticks.
    let t = cfr(1_000, 40, 10, -2);
    assert_eq!(frame_at(&t, r(0, 1)).index, 0);
    assert!(!frame_at(&t, r(0, 1)).out_of_range);
    let before = frame_at(&t, r(-3, 1_000));
    assert_eq!((before.index, before.out_of_range), (0, true));
    let first = frame_at(&t, r(-2, 1_000));
    assert_eq!((first.index, first.out_of_range), (0, false));
    // The last frame ends at −2 + 400 = 398 ticks.
    let last = frame_at(&t, r(397, 1_000));
    assert_eq!((last.index, last.out_of_range), (9, false));
    let end = frame_at(&t, r(398, 1_000));
    assert_eq!((end.index, end.out_of_range), (9, true));
    let far = frame_at(&t, r(1_000_000, 1));
    assert_eq!((far.index, far.out_of_range), (9, true));
    let far_before = frame_at(&t, r(-1_000_000, 1));
    assert_eq!((far_before.index, far_before.out_of_range), (0, true));
}

#[test]
fn construction_refuses_duplicates_and_sorts() {
    let dup = SampleTable::new(1_000, vec![entry(0, 40, 0, true), entry(0, 40, 1, false)]);
    assert_eq!(dup.unwrap_err().code, "MEDIA_CORRUPT");
    // An edit list that repeats media presents one sample twice.
    let twice = SampleTable::new(1_000, vec![entry(0, 40, 0, true), entry(40, 40, 0, true)]);
    assert_eq!(twice.unwrap().len(), 2);
    assert_eq!(
        SampleTable::new(1_000, vec![]).unwrap_err().code,
        "MEDIA_CORRUPT"
    );
    assert_eq!(
        SampleTable::new(0, vec![entry(0, 1, 0, true)])
            .unwrap_err()
            .code,
        "MEDIA_CORRUPT"
    );
    // B-frame order in decode order: I0 P3 B1 B2 → sorted by pts.
    let t = SampleTable::new(
        1,
        vec![
            entry(0, 1, 0, true),
            entry(3, 1, 1, false),
            entry(1, 1, 2, false),
            entry(2, 1, 3, false),
        ],
    )
    .unwrap();
    let pts: Vec<i64> = t.frames().iter().map(|f| f.pts).collect();
    assert_eq!(pts, [0, 1, 2, 3]);
    assert_eq!(frame_at(&t, r(3, 2)).index, 1);
}

#[test]
fn decode_start_with_closed_and_open_gops() {
    // Closed GOPs of 4: I P P P | I P P P (decode order = presentation order).
    let closed = SampleTable::new(
        1,
        (0..8).map(|i| entry(i, 1, i as u32, i % 4 == 0)).collect(),
    )
    .unwrap();
    let starts: Vec<usize> = (0..8).map(|i| decode_start(&closed, i)).collect();
    assert_eq!(starts, [0, 0, 0, 0, 4, 4, 4, 4]);

    // Open GOP: decode order I0(pts 0) P(3) B(1) B(2) I(6) B(4) B(5) P(9) B(7) B(8): the B
    // frames at pts 4 and 5 follow I(6) in decode order but precede it on screen, so they
    // need the previous GOP.
    let order = [
        (0, true),
        (3, false),
        (1, false),
        (2, false),
        (6, true),
        (4, false),
        (5, false),
        (9, false),
        (7, false),
        (8, false),
    ];
    let open = SampleTable::new(
        1,
        order
            .iter()
            .enumerate()
            .map(|(d, &(pts, key))| entry(pts, 1, d as u32, key))
            .collect(),
    )
    .unwrap();
    let starts: Vec<usize> = (0..10).map(|i| decode_start(&open, i)).collect();
    // Presented frames by pts 0..9: decode indices 0 2 3 1 5 6 4 8 9 7.
    assert_eq!(starts, [0, 0, 0, 0, 0, 0, 4, 4, 4, 4]);

    // An edit list dropped the keyframe at decode index 0: the first presented frame still
    // decodes from it.
    let edited = SampleTable::with_sync(
        1,
        vec![entry(0, 1, 1, false), entry(1, 1, 2, false)],
        vec![SyncPoint {
            decode_index: 0,
            pts: -1,
        }],
    )
    .unwrap();
    assert_eq!(decode_start(&edited, 0), 0);
    assert_eq!(decode_start(&edited, 1), 0);
}

#[test]
fn untagged_colour_follows_the_height_rule() {
    let sd = ColourTags::default().resolve_defaults(576);
    assert_eq!(
        (sd.primaries, sd.matrix, sd.transfer, sd.siting),
        (
            Primaries::Bt601_625,
            Matrix::Bt601,
            Transfer::Bt709,
            ChromaSiting::Left
        )
    );
    assert_eq!(
        ColourTags::default().resolve_defaults(480).primaries,
        Primaries::Bt601_525
    );
    let hd = ColourTags::default().resolve_defaults(720);
    assert_eq!((hd.primaries, hd.matrix), (Primaries::Bt709, Matrix::Bt709));
    // Known fields stay.
    let tagged = ColourTags {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Hlg,
        ..ColourTags::default()
    };
    let got = tagged.resolve_defaults(480);
    assert_eq!(
        (got.primaries, got.transfer, got.matrix),
        (Primaries::Bt2020, Transfer::Hlg, Matrix::Bt601)
    );
    // CICP round trip.
    for code in [1u16, 5, 6, 9, 12] {
        assert_eq!(Primaries::from_cicp(code).cicp(), code);
    }
    assert_eq!(
        ColourTags::from_cicp(9, 18, 9, true).to_cicp(),
        (9, 18, 9, true)
    );
}

#[test]
fn display_size_for_each_rotation_and_a_4_3_pixel_aspect() {
    let coded = Size::new(1920, 1080);
    let square = r(1, 1);
    assert_eq!(
        display_size(coded, Rotation::R0, square),
        Size::new(1920, 1080)
    );
    assert_eq!(
        display_size(coded, Rotation::R90, square),
        Size::new(1080, 1920)
    );
    assert_eq!(
        display_size(coded, Rotation::R180, square),
        Size::new(1920, 1080)
    );
    assert_eq!(
        display_size(coded, Rotation::R270, square),
        Size::new(1080, 1920)
    );
    // 1440×1080 anamorphic at 4:3 → 1920 wide; rotated, the scaled width becomes the height.
    let hdv = Size::new(1440, 1080);
    assert_eq!(
        display_size(hdv, Rotation::R0, r(4, 3)),
        Size::new(1920, 1080)
    );
    assert_eq!(
        display_size(hdv, Rotation::R90, r(4, 3)),
        Size::new(1080, 1920)
    );
    // DV NTSC 720×480 at 10:11 → 654.5 → nearest even 654; odd results round to even.
    assert_eq!(
        display_size(Size::new(720, 480), Rotation::R0, r(10, 11)).width,
        654
    );
    assert_eq!(
        display_size(Size::new(3, 2), Rotation::R0, r(1, 1)).width,
        3
    );
    assert_eq!(
        display_size(Size::new(3, 2), Rotation::R0, r(5, 4)).width,
        4
    );
    // Serialised as degrees.
    assert_eq!(serde_json::to_string(&Rotation::R270).unwrap(), "270");
    assert!(serde_json::from_str::<Rotation>("45").is_err());
}

fn cases(default: u32) -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    ProptestConfig {
        cases,
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(cases(256))]

    /// The binary search equals a linear nearest-pts scan with the earlier-wins tie rule, for
    /// random VFR tables, offsets and times (some at exact midpoints and a tick either side).
    #[test]
    fn frame_at_matches_linear_scan(
        timescale in prop::sample::select(vec![1u32, 24, 1_000, 30_000, 90_000]),
        start in -1_000i64..1_000,
        durations in prop::collection::vec(1u32..500, 1..60),
        picks in prop::collection::vec((0usize..60, -1i64..=1, 1i64..7), 1..20),
        free in prop::collection::vec((-2_000i64..40_000, 1i64..1_000), 1..20),
    ) {
        let t = table_from_durations(timescale, start, &durations);
        let tick = r(1, timescale as i64);
        for (i, side, _) in picks {
            if t.len() < 2 { break; }
            let i = i % (t.len() - 1);
            let s = add(midpoint(&t, i), r(side * tick.num(), tick.den()));
            prop_assert_eq!(frame_at(&t, s).index, linear_nearest(&t, s), "s = {}", s);
        }
        for (n, d) in free {
            let s = r(n, d);
            prop_assert_eq!(frame_at(&t, s).index, linear_nearest(&t, s), "s = {}", s);
        }
    }
}
