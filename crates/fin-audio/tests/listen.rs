//! `fin listen`'s measurements (plan 6C Task 21, Decision 21): per-track and mix loudness,
//! clipping, the silence detector and the loudest moments, over plans mixed from in-memory
//! sources. Tolerances are stated per assertion (LU, dB; times exact at the detector's
//! resolution).

use std::collections::BTreeMap;

use fin_audio::channels::Canonical;
use fin_audio::listen::{Pass, filter_plan, measure, stretch_extreme};
use fin_audio::testing::signals::{db_to_amp, noise, sine};
use fin_audio::testing::source::MemSource;
use fin_audio::{PcmSource, RATE, Stereo};
use fin_core::eval::{AudioItem, AudioPlan, GainSegment, PanSegment};
use fin_core::{Interp, Rational};
use fin_media::jobs::{ListenSpan, ListenStats};

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d).unwrap()
}

fn s(n: i64) -> Rational {
    Rational::from_int(n)
}

/// An item of `track` at `db` dB, centre, playing `asset` from `source_from` at speed 1.
fn item(
    id: &str,
    track: &str,
    asset: &str,
    from: Rational,
    to: Rational,
    source_from: Rational,
    db: f64,
) -> AudioItem {
    AudioItem {
        item: id.to_string(),
        track: track.to_string(),
        via: vec![],
        asset: asset.to_string(),
        from,
        to,
        source_from,
        speed: s(1),
        gain: vec![GainSegment {
            from,
            to,
            db: [db, db],
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
}

fn plan(from: Rational, to: Rational, items: Vec<AudioItem>) -> AudioPlan {
    AudioPlan {
        timeline: "tl1".to_string(),
        from,
        to,
        items,
    }
}

/// The same signal on both channels.
fn stereo(x: Vec<f32>) -> Canonical {
    Canonical::Stereo(Stereo { l: x.clone(), r: x })
}

fn sources(list: Vec<(&str, Canonical)>) -> BTreeMap<String, Box<dyn PcmSource>> {
    list.into_iter()
        .map(|(a, c)| (a.to_string(), MemSource::boxed(c)))
        .collect()
}

/// A stereo sine whose RMS on each channel is `rms` dBFS.
fn tone(hz: f64, rms: f64, secs: f64) -> Canonical {
    stereo(sine(hz, rms + 3.0103, secs, RATE))
}

/// The raw signal measured without the mixer.
fn raw(c: Canonical, secs: i64) -> ListenStats {
    let p = plan(
        s(0),
        s(secs),
        vec![item("i", "t", "a", s(0), s(secs), s(0), 0.0)],
    );
    // At 0 dB and centre a stereo source is passed as is (balance at 0 is unity).
    measure(&p, sources(vec![("a", c)]), 48_000).unwrap()
}

/// `|H(e^{jω})|²` of a biquad `b`/`a` at `hz` (48 kHz).
fn biquad_power(b: [f64; 3], a: [f64; 3], hz: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / 48_000.0;
    let at = |c: [f64; 3]| {
        let re = c[0] + c[1] * w.cos() + c[2] * (2.0 * w).cos();
        let im = -c[1] * w.sin() - c[2] * (2.0 * w).sin();
        re * re + im * im
    };
    at(b) / at(a)
}

/// The BS.1770 loudness of a stereo sine at `hz` whose RMS is `rms` dBFS on each channel.
fn bs1770(hz: f64, rms: f64) -> f64 {
    let shelf = biquad_power(
        [1.53512485958697, -2.69169618940638, 1.19839281085285],
        [1.0, -1.69065929318241, 0.73248077421585],
        hz,
    );
    let hp = biquad_power(
        [1.0, -2.0, 1.0],
        [1.0, -1.99004745483398, 0.99007225036621],
        hz,
    );
    let ms = 10f64.powf(rms / 10.0);
    -0.691 + 10.0 * (2.0 * ms * shelf * hp).log10()
}

#[test]
fn listen_mix_and_tracks() {
    let p = plan(
        s(0),
        s(10),
        vec![
            item("i1", "t1", "a", s(0), s(10), s(0), 0.0),
            item("i2", "t2", "b", s(0), s(10), s(0), 0.0),
        ],
    );
    let (a, b) = (|| tone(1000.0, -20.0, 10.0), || tone(1250.0, -26.0, 10.0));
    let mix = measure(&p, sources(vec![("a", a()), ("b", b())]), 48_000).unwrap();
    let t1 = measure(
        &filter_plan(&p, |i| i.track == "t1"),
        sources(vec![("a", a())]),
        1_000,
    )
    .unwrap();
    let t2 = measure(
        &filter_plan(&p, |i| i.track == "t2"),
        sources(vec![("b", b())]),
        48_000,
    )
    .unwrap();
    let (i1, i2) = (t1.integrated.unwrap(), t2.integrated.unwrap());
    // A 1 kHz stereo sine at −20 dBFS RMS a channel is −16.99 LUFS (BS.1770: the two channels'
    // power sum, K-weighting ≈ 0 dB at 1 kHz); within 0.2 LU.
    assert!((i1 - -16.99).abs() <= 0.2, "{i1}");
    // Each track reads what its source alone reads, within 0.2 LU.
    assert!((i1 - raw(a(), 10).integrated.unwrap()).abs() <= 0.2, "{i1}");
    assert!((i2 - raw(b(), 10).integrated.unwrap()).abs() <= 0.2, "{i2}");
    // Both against BS.1770 computed: −0.691 + 10·log10(Σ channels' mean square × |K(f)|²), with
    // the K filter's published 48 kHz coefficients; within 0.2 LU.
    assert!(
        (i1 - bs1770(1000.0, -20.0)).abs() <= 0.2,
        "{i1} vs {}",
        bs1770(1000.0, -20.0)
    );
    assert!(
        (i2 - bs1770(1250.0, -26.0)).abs() <= 0.2,
        "{i2} vs {}",
        bs1770(1250.0, -26.0)
    );
    // The mix is their power sum (uncorrelated tones), within 0.2 LU.
    let sum = 10.0 * (10f64.powf(i1 / 10.0) + 10f64.powf(i2 / 10.0)).log10();
    let m = mix.integrated.unwrap();
    assert!((m - sum).abs() <= 0.2, "mix {m}, power sum {sum}");
    // Nothing clips or is silent; the peaks are reported.
    assert_eq!(mix.clipped.samples, 0);
    assert!(mix.silences.is_empty(), "{:?}", mix.silences);
    assert!(mix.true_peak.unwrap() < 0.0 && mix.sample_peak.unwrap() < 0.0);
}

#[test]
fn listen_detects_clipping() {
    // A full-scale cosine (its first sample is 1.0) at +3 dB from 1.5 s: the item's first frame
    // clips, so `firstAt` is exactly 3/2.
    let x: Vec<f32> = (0..2 * RATE as usize)
        .map(|n| libm::cos(2.0 * std::f64::consts::PI * 1000.0 * n as f64 / f64::from(RATE)) as f32)
        .collect();
    let p = plan(
        s(0),
        s(4),
        vec![item("i1", "t1", "a", r(3, 2), r(7, 2), s(0), 3.0)],
    );
    let st = measure(&p, sources(vec![("a", stereo(x.clone()))]), 1_000).unwrap();
    assert!(st.clipped.samples > 0);
    assert_eq!(st.clipped.first_at, Some(r(3, 2)));
    assert!(st.sample_peak.unwrap() > 2.9, "{:?}", st.sample_peak);
    // At 0 dB it never passes 1.0.
    let p = plan(
        s(0),
        s(4),
        vec![item("i1", "t1", "a", r(3, 2), r(7, 2), s(0), 0.0)],
    );
    let st = measure(&p, sources(vec![("a", stereo(x))]), 48_000).unwrap();
    assert_eq!((st.clipped.samples, st.clipped.first_at), (0, None));
}

fn span(from: Rational, to: Rational) -> ListenSpan {
    ListenSpan { from, to }
}

/// A −20 dBFS tone track over `spans` (each played from source time 0), measured over `[0, end)`.
fn gaps(spans: &[(Rational, Rational)], end: Rational) -> ListenStats {
    let items = spans
        .iter()
        .enumerate()
        .map(|(k, &(a, b))| item(&format!("i{k}"), "t1", "a", a, b, s(0), 0.0))
        .collect();
    let p = plan(s(0), end, items);
    measure(&p, sources(vec![("a", tone(1000.0, -20.0, 10.0))]), 48_000).unwrap()
}

#[test]
fn listen_silences_over_half_second() {
    // A 0.7 s gap from 2.000 s is [2, 27/10) exactly.
    let st = gaps(&[(s(0), s(2)), (r(27, 10), s(5))], s(5));
    assert_eq!(st.silences, [span(s(2), r(27, 10))]);
    // A 0.4 s gap and a 0.495 s one (49.5 blocks: 49 silent ones) are not silences; a silence
    // running into the range end is reported, cut there.
    let st = gaps(&[(s(0), s(2)), (r(12, 5), s(4)), (r(899, 200), s(6))], s(7));
    assert_eq!(st.silences, [span(s(6), s(7))]);
    // A range ending mid-block: the last block is shorter and the silence ends at the range end.
    let st = gaps(&[(s(0), s(1))], r(16_005, 10_000));
    assert_eq!(st.silences, [span(s(1), r(16_005, 10_000))]);
    // Noise at −59 dBFS RMS is not silence; at −61 dBFS it is.
    for (db, silent) in [(-59.0, false), (-61.0, true)] {
        // Uniform noise in [−1, 1] has an RMS of 1/√3.
        let k = (db_to_amp(db) * 3f64.sqrt()) as f32;
        let x: Vec<f32> = noise(7, 5.0, RATE).into_iter().map(|v| v * k).collect();
        let p = plan(
            s(0),
            s(5),
            vec![item("i1", "t1", "a", s(0), s(5), s(0), 0.0)],
        );
        let st = measure(&p, sources(vec![("a", stereo(x))]), 48_000).unwrap();
        let want = if silent {
            vec![span(s(0), s(5))]
        } else {
            vec![]
        };
        assert_eq!(st.silences, want, "{db} dBFS");
    }
}

#[test]
fn listen_loudest_moments_top5_spaced() {
    // A −50 dBFS bed with 1 s bursts every 4 s, the first the loudest.
    let secs: i32 = 30;
    let bed = sine(500.0, -50.0, f64::from(secs), RATE);
    let mut x = bed;
    let bursts = [
        (2, -10.0),
        (6, -12.0),
        (10, -14.0),
        (14, -16.0),
        (18, -18.0),
        (22, -20.0),
    ];
    for &(at, db) in &bursts {
        let b = sine(1000.0, db, 1.0, RATE);
        for (k, v) in b.into_iter().enumerate() {
            x[at * RATE as usize + k] += v;
        }
    }
    let p = plan(
        s(0),
        s(secs as i64),
        vec![item("i1", "t1", "a", s(0), s(secs as i64), s(0), 0.0)],
    );
    let st = measure(&p, sources(vec![("a", stereo(x))]), 4_321).unwrap();
    assert_eq!(st.loudest.len(), 5, "{:?}", st.loudest);
    // Loudest first, at least 3 s apart.
    for w in st.loudest.windows(2) {
        assert!(w[0].short_term >= w[1].short_term, "{:?}", st.loudest);
    }
    for (i, a) in st.loudest.iter().enumerate() {
        for b in &st.loudest[i + 1..] {
            assert!(
                (a.at.to_f64() - b.at.to_f64()).abs() >= 3.0,
                "{:?}",
                st.loudest
            );
        }
    }
    // The five loudest bursts each lie in the 3 s window of the moment of their rank.
    for (m, &(at, _)) in st.loudest.iter().zip(&bursts) {
        let (w0, w1) = (m.at.to_f64(), m.at.to_f64() + 3.0);
        assert!(
            w0 <= at as f64 && at as f64 + 1.0 <= w1 + 1e-9,
            "burst at {at}: {m:?}"
        );
    }
}

#[test]
fn block_length_does_not_change_the_statistics() {
    let p = plan(
        s(0),
        s(6),
        vec![item("i1", "t1", "a", s(1), s(4), s(0), 0.0)],
    );
    let a = measure(&p, sources(vec![("a", tone(1000.0, -20.0, 5.0))]), 128).unwrap();
    let b = measure(&p, sources(vec![("a", tone(1000.0, -20.0, 5.0))]), 48_000).unwrap();
    assert_eq!(a.silences, b.silences);
    assert_eq!(a.silences, [span(s(0), s(1)), span(s(4), s(6))]);
    assert_eq!(a.clipped, b.clipped);
    assert!((a.integrated.unwrap() - b.integrated.unwrap()).abs() < 1e-9);
}

#[test]
fn a_pass_lists_its_missing_sources_and_extreme_speeds_are_named() {
    let p = plan(
        s(0),
        s(1),
        vec![item("i1", "t1", "gone", s(0), s(1), s(0), 0.0)],
    );
    let mut pass = Pass::new(&p, BTreeMap::new()).unwrap();
    assert_eq!(pass.mixer().missing(), ["gone"]);
    pass.render(48_000).unwrap();
    let st = pass.finish();
    assert_eq!(st.integrated, None);
    assert_eq!(st.silences, [span(s(0), s(1))]);
    assert!(stretch_extreme(r(1, 5)) && stretch_extreme(s(5)));
    assert!(!stretch_extreme(r(1, 4)) && !stretch_extreme(s(4)) && !stretch_extreme(s(1)));
}

#[test]
fn a_silence_touching_a_range_edge_is_reported_at_any_length() {
    // 6C Task 21 review S3 (Decision 21: "a run touching a range edge is reported clipped, not
    // dropped"): 0.3 s of silence at the start and at the end of the range.
    let st = gaps(&[(r(3, 10), r(27, 10))], s(3));
    assert_eq!(st.silences, [span(s(0), r(3, 10)), span(r(27, 10), s(3))]);
    // Inside the range the 0.5 s rule holds.
    let st = gaps(&[(s(0), s(1)), (r(13, 10), s(3))], s(3));
    assert!(st.silences.is_empty(), "{:?}", st.silences);
}

#[test]
fn an_item_that_has_ended_holds_no_buffer_and_its_source_is_closed() {
    // 6C Task 21 review S2: many short items on their own sources; once each has ended, the mixer
    // holds nothing of it.
    let items: Vec<AudioItem> = (0..40)
        .map(|k| {
            item(
                &format!("i{k}"),
                "t1",
                &format!("a{k}"),
                r(k, 10),
                r(k + 1, 10),
                s(0),
                0.0,
            )
        })
        .collect();
    let p = plan(s(0), s(5), items);
    let srcs = sources(
        (0..40)
            .map(|k| (format!("a{k}"), tone(1000.0, -20.0, 1.0)))
            .collect::<Vec<_>>()
            .iter()
            .map(|(a, c)| (a.as_str(), c.clone()))
            .collect(),
    );
    let mut pass = Pass::new(&p, srcs).unwrap();
    assert_eq!(pass.mixer().open_sources(), 40);
    // Half a second in: the first five items have ended.
    pass.render(24_000).unwrap();
    assert_eq!(pass.mixer().open_sources(), 35);
    // Past every item: no buffer, no source.
    pass.render(4 * 48_000).unwrap();
    assert_eq!(pass.mixer().buffered_frames(), 0);
    assert_eq!(pass.mixer().open_sources(), 0);
}
