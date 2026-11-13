//! The mixer (plan 6C Task 7; Decisions 3, 4, 16, 17): sample-exact item edges, gain curves,
//! fades and crossfades, the two pan laws, speed through WSOLA, plan-order summing and block
//! independence.

use std::collections::BTreeMap;

use fin_audio::channels::Canonical;
use fin_audio::testing::{analyse::*, signals::*, source::MemSource};
use fin_audio::{Mixer, PcmSource, RATE, SourceLayout, Stereo, Wsola, sample_at};
use fin_core::eval::PlanValue;
use fin_core::eval::{
    AudioEffectPlan, AudioItem, AudioPlan, GainSegment, PanSegment, Shape, ShapeKind,
};
use fin_core::{CoreError, Curve, Interp, Rational};

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d).unwrap()
}

fn s(n: i64) -> Rational {
    Rational::from_int(n)
}

fn gain(from: Rational, to: Rational, db: [f64; 2], shapes: Vec<Shape>) -> GainSegment {
    GainSegment {
        from,
        to,
        db,
        interp: Interp::Linear,
        u: [0.0, 1.0],
        shapes,
    }
}

fn pan(from: Rational, to: Rational, p: [f64; 2]) -> PanSegment {
    PanSegment {
        from,
        to,
        pan: p,
        interp: Interp::Linear,
        u: [0.0, 1.0],
    }
}

fn shape(kind: ShapeKind, curve: Curve) -> Shape {
    Shape {
        kind,
        curve,
        p: [0.0, 1.0],
    }
}

/// An item playing `asset` from source time 0 at speed 1, 0 dB, centre.
fn item(id: &str, asset: &str, from: Rational, to: Rational) -> AudioItem {
    AudioItem {
        item: id.to_string(),
        track: "t1".to_string(),
        via: vec![],
        asset: asset.to_string(),
        from,
        to,
        source_from: s(0),
        speed: s(1),
        gain: vec![gain(from, to, [0.0, 0.0], vec![])],
        pan: vec![pan(from, to, [0.0, 0.0])],
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

fn mono(x: Vec<f32>) -> Canonical {
    Canonical::Mono(x)
}

fn stereo(l: Vec<f32>, r: Vec<f32>) -> Canonical {
    Canonical::Stereo(Stereo { l, r })
}

fn sources(list: Vec<(&str, Box<dyn PcmSource>)>) -> BTreeMap<String, Box<dyn PcmSource>> {
    list.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn mem(c: Canonical) -> Box<dyn PcmSource> {
    MemSource::boxed(c)
}

/// Renders `frames` frames in blocks of `block`.
fn render(m: &mut Mixer, frames: usize, block: usize) -> Stereo {
    let mut out = Stereo::default();
    let mut done = 0;
    while done < frames {
        let n = block.min(frames - done);
        let mut b = Stereo::silence(n);
        m.render(&mut b).unwrap();
        out.l.extend_from_slice(&b.l);
        out.r.extend_from_slice(&b.r);
        done += n;
    }
    out
}

fn mix(p: &AudioPlan, src: Vec<(&str, Box<dyn PcmSource>)>, frames: usize) -> Stereo {
    let mut m = Mixer::new(p, sources(src)).unwrap();
    render(&mut m, frames, 1_000)
}

fn db(a: f32) -> f64 {
    20.0 * f64::from(a).log10()
}

/// A source of constant frames `(l, r)`, any length, without holding samples.
struct Const(f32, f32);

impl PcmSource for Const {
    fn layout(&self) -> SourceLayout {
        SourceLayout::Stereo
    }

    fn read(&mut self, _at: i64, frames: usize, out: &mut Canonical) -> Result<(), CoreError> {
        *out = stereo(vec![self.0; frames], vec![self.1; frames]);
        Ok(())
    }
}

/// A mono source served from fixed chunks of 262,144 frames, as a `pcm48` 1-channel entry is
/// (Decision 7): every read is assembled across chunk boundaries.
struct Chunked(Vec<Vec<f32>>);

impl Chunked {
    fn new(x: &[f32]) -> Chunked {
        Chunked(x.chunks(262_144).map(<[f32]>::to_vec).collect())
    }
}

impl PcmSource for Chunked {
    fn layout(&self) -> SourceLayout {
        SourceLayout::Mono
    }

    fn read(&mut self, at: i64, frames: usize, out: &mut Canonical) -> Result<(), CoreError> {
        let v = (0..frames as i64)
            .map(|i| {
                let k = at + i;
                if k < 0 {
                    return 0.0;
                }
                let (c, o) = ((k / 262_144) as usize, (k % 262_144) as usize);
                self.0
                    .get(c)
                    .and_then(|ch| ch.get(o))
                    .copied()
                    .unwrap_or(0.0)
            })
            .collect();
        *out = mono(v);
        Ok(())
    }
}

#[test]
fn single_item_unity_passthrough() {
    let (l, rr) = (noise(1, 1.0, RATE), noise(2, 1.0, RATE));
    let p = plan(s(0), s(1), vec![item("i1", "a", s(0), s(1))]);
    let out = mix(&p, vec![("a", mem(stereo(l.clone(), rr.clone())))], 48_000);
    assert!(out.l == l, "left differs");
    assert!(out.r == rr, "right differs");
}

#[test]
fn item_edges_sample_exact() {
    let x = vec![0.5f32; 3 * 48_000];
    let p = plan(s(0), s(3), vec![item("i1", "a", s(1), s(2))]);
    let out = mix(&p, vec![("a", mem(stereo(x.clone(), x)))], 3 * 48_000);
    let nonzero: Vec<usize> = (0..out.len()).filter(|&i| out.l[i] != 0.0).collect();
    assert_eq!(nonzero.first(), Some(&48_000));
    assert_eq!(nonzero.last(), Some(&95_999));
    assert_eq!(nonzero.len(), 48_000);
    assert!(out.l[48_000..96_000].iter().all(|&v| v == 0.5));
}

#[test]
fn ntsc_rate_item_edges_exact() {
    // 10,000 one-frame items at 30000/1001 fps, alternating a left-only and a right-only
    // source: item k spans [k·1001/30000, (k + 1)·1001/30000).
    const K: i64 = 10_000;
    let edge = |k: i64| -> i64 {
        // round_half_up(k × 1001 × 48000 / 30000), computed independently of `sample_at`.
        (2 * k * 1001 * 48_000 + 30_000).div_euclid(2 * 30_000)
    };
    let items: Vec<AudioItem> = (0..K)
        .map(|k| {
            let asset = if k % 2 == 0 { "left" } else { "right" };
            item(
                &format!("i{k}"),
                asset,
                r(k * 1001, 30_000),
                r((k + 1) * 1001, 30_000),
            )
        })
        .collect();
    for k in [0, 1, 7, 4_999, K] {
        assert_eq!(sample_at(r(k * 1001, 30_000)).unwrap(), edge(k));
    }
    let p = plan(s(0), r(K * 1001, 30_000), items);
    let src: Vec<(&str, Box<dyn PcmSource>)> = vec![
        ("left", Box::new(Const(1.0, 0.0))),
        ("right", Box::new(Const(0.0, 1.0))),
    ];
    let mut m = Mixer::new(&p, sources(src)).unwrap();
    let total = edge(K) as usize;
    let mut k = 0i64;
    let mut n = 0usize;
    while n < total {
        let len = 48_000.min(total - n);
        let mut b = Stereo::silence(len);
        m.render(&mut b).unwrap();
        for i in 0..len {
            let at = (n + i) as i64;
            while at >= edge(k + 1) {
                k += 1;
            }
            let want = if k % 2 == 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            assert_eq!((b.l[i], b.r[i]), want, "sample {at} (item {k})");
        }
        n += len;
    }
    assert_eq!(k, K - 1);
    assert_eq!(m.position(), edge(K));
}

#[test]
fn gain_db_curve() {
    let x = vec![1.0f32; 48_000];
    let mut it = item("i1", "a", s(0), s(1));
    it.gain = vec![gain(s(0), s(1), [-20.0, 0.0], vec![])];
    let p = plan(s(0), s(1), vec![it]);
    let out = mix(&p, vec![("a", mem(stereo(x.clone(), x)))], 48_000);
    assert!((db(out.l[0]) + 20.0).abs() <= 0.01, "{}", db(out.l[0]));
    assert!(
        (db(out.l[24_000]) + 10.0).abs() <= 0.01,
        "{}",
        db(out.l[24_000])
    );
    assert!(
        (db(out.l[12_000]) + 15.0).abs() <= 0.01,
        "{}",
        db(out.l[12_000])
    );
    assert!(db(out.l[47_999]).abs() <= 0.01, "{}", db(out.l[47_999]));
    assert!(out.l == out.r);
}

#[test]
fn fade_in_linear() {
    let x = vec![1.0f32; 48_000];
    let mut it = item("i1", "a", s(0), s(1));
    it.gain = vec![gain(
        s(0),
        s(1),
        [0.0, 0.0],
        vec![shape(ShapeKind::FadeIn, Curve::Linear)],
    )];
    let p = plan(s(0), s(1), vec![it]);
    let out = mix(&p, vec![("a", mem(stereo(x.clone(), x)))], 48_000);
    assert_eq!(out.l[0], 0.0);
    for (n, &v) in out.l.iter().enumerate() {
        let want = n as f64 / 48_000.0;
        assert!(
            (f64::from(v) - want).abs() <= 1e-5,
            "sample {n}: {v} vs {want}"
        );
    }
    assert!(out.l.windows(2).all(|w| w[1] >= w[0]), "not monotone");
}

#[test]
fn crossfade_equal_power_constant_power() {
    // A (left only) crosses out over [1, 2) s as B (right only) crosses in.
    let mut a = item("a", "left", s(0), s(2));
    a.gain = vec![
        gain(s(0), s(1), [0.0, 0.0], vec![]),
        gain(
            s(1),
            s(2),
            [0.0, 0.0],
            vec![shape(ShapeKind::CrossOut, Curve::EqualPower)],
        ),
    ];
    let mut b = item("b", "right", s(1), s(3));
    b.gain = vec![
        gain(
            s(1),
            s(2),
            [0.0, 0.0],
            vec![shape(ShapeKind::CrossIn, Curve::EqualPower)],
        ),
        gain(s(2), s(3), [0.0, 0.0], vec![]),
    ];
    let p = plan(s(0), s(3), vec![a, b]);
    let src: Vec<(&str, Box<dyn PcmSource>)> = vec![
        ("left", Box::new(Const(1.0, 0.0))),
        ("right", Box::new(Const(0.0, 1.0))),
    ];
    let out = mix(&p, src, 3 * 48_000);
    for n in 48_000..96_000 {
        let (l, rr) = (f64::from(out.l[n]), f64::from(out.r[n]));
        let power_db = 10.0 * (l * l + rr * rr).log10();
        assert!(power_db.abs() <= 0.01, "sample {n}: {power_db} dB");
    }
    // The ends of the window: A alone, then B alone.
    assert_eq!((out.l[48_000], out.r[48_000]), (1.0, 0.0));
    assert_eq!(out.l[96_000], 0.0);
    assert_eq!(out.r[96_000], 1.0);
    // Equal power is not linear: the midpoint is at −3 dB on each side.
    assert!(
        (db(out.l[72_000]) + 3.0103).abs() <= 0.01,
        "{}",
        db(out.l[72_000])
    );
}

#[test]
fn mono_vs_dual_mono_pan() {
    let x = sine(440.0, -6.0, 0.5, RATE);
    let frames = x.len();
    let run = |src: Canonical, p: f64| -> Stereo {
        let mut it = item("i1", "a", s(0), r(1, 2));
        it.pan = vec![pan(s(0), r(1, 2), [p, p])];
        let pl = plan(s(0), r(1, 2), vec![it]);
        mix(&pl, vec![("a", mem(src))], frames)
    };
    let scaled = |k: f32| -> Vec<f32> { x.iter().map(|&v| v * k).collect() };
    let zero = vec![0.0f32; frames];
    // (pan, mono L, mono R, dual L, dual R)
    type Case = (f64, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>);
    let cases: [Case; 3] = [
        (
            0.0,
            scaled(0.70710677),
            scaled(0.70710677),
            x.clone(),
            x.clone(),
        ),
        (-1.0, x.clone(), zero.clone(), x.clone(), zero.clone()),
        (0.5, scaled(0.5), scaled(0.8660254), scaled(0.5), x.clone()),
    ];
    for (p, ml, mr, dl, dr) in cases {
        let m = run(mono(x.clone()), p);
        assert!(m.l == ml, "mono L at pan {p}");
        assert!(m.r == mr, "mono R at pan {p}");
        let d = run(stereo(x.clone(), x.clone()), p);
        assert!(d.l == dl, "dual mono L at pan {p}");
        assert!(d.r == dr, "dual mono R at pan {p}");
    }
}

/// An `eq` with every band off (Task 8 fills the effect; here it must be a no-op).
fn eq_all_off() -> AudioEffectPlan {
    let mut values = BTreeMap::new();
    for band in 1..=8 {
        values.insert(
            format!("band{band}Type"),
            PlanValue::Text("off".to_string()),
        );
    }
    AudioEffectPlan {
        id: "fx1".to_string(),
        kind: "eq".to_string(),
        numbers: BTreeMap::new(),
        values,
    }
}

#[test]
fn mono_upmixed_once() {
    let x = sine(1_000.0, -6.0, 1.0, RATE);
    let mut it = item("i1", "a", s(0), s(1));
    it.effects = vec![eq_all_off()];
    let p = plan(s(0), s(1), vec![it]);
    let out = mix(&p, vec![("a", mem(mono(x.clone())))], 48_000);
    let base = rms_dbfs(&x);
    for side in [&out.l, &out.r] {
        let d = rms_dbfs(side) - base;
        assert!(
            (d + 3.0103).abs() <= 0.01,
            "{d} dB (−6.02 would be a double upmix)"
        );
    }
}

#[test]
fn cached_and_decoded_mono_agree() {
    // The "decoded" source is one buffer; the "cached" one is `pcm48` 1-channel chunks.
    let x = pink(5, 7.0, RATE);
    let mut it = item("i1", "a", s(0), s(5));
    it.source_from = r(9, 2); // reads cross the chunk edge at 262,144 frames
    it.gain = vec![
        gain(
            s(0),
            s(1),
            [-12.0, 0.0],
            vec![shape(ShapeKind::FadeIn, Curve::EqualPower)],
        ),
        gain(s(1), s(5), [0.0, -6.0], vec![]),
    ];
    it.pan = vec![pan(s(0), s(5), [-0.8, 0.6])];
    let p = plan(s(0), s(5), vec![it]);
    let decoded = mix(&p, vec![("a", mem(mono(x.clone())))], 5 * 48_000);
    let cached = mix(&p, vec![("a", Box::new(Chunked::new(&x)))], 5 * 48_000);
    assert!(decoded.l.iter().any(|&v| v != 0.0));
    assert!(decoded == cached);
}

#[test]
fn two_items_sum_in_plan_order() {
    let (a, b, c) = (
        noise(3, 1.0, RATE),
        noise(4, 1.0, RATE),
        noise(5, 1.0, RATE),
    );
    let dbs = [-3.7, -9.1, 2.3];
    let mut items = vec![];
    for (k, (name, g)) in ["a", "b", "c"].iter().zip(dbs).enumerate() {
        let mut it = item(&format!("i{k}"), name, s(0), s(1));
        it.gain = vec![gain(s(0), s(1), [g, g], vec![])];
        items.push(it);
    }
    let p = plan(s(0), s(1), items);
    let src: Vec<(&str, Box<dyn PcmSource>)> = vec![
        ("a", mem(stereo(a.clone(), a.clone()))),
        ("b", mem(stereo(b.clone(), b.clone()))),
        ("c", mem(stereo(c.clone(), c.clone()))),
    ];
    let out = mix(&p, src, 48_000);
    let amp = |d: f64| libm::powf(10.0, (d / 20.0) as f32);
    let (ga, gb, gc) = (amp(dbs[0]), amp(dbs[1]), amp(dbs[2]));
    for n in 0..48_000 {
        let want = ((0.0 + a[n] * ga) + b[n] * gb) + c[n] * gc;
        assert_eq!(out.l[n].to_bits(), want.to_bits(), "sample {n}");
        assert_eq!(out.r[n].to_bits(), want.to_bits(), "sample {n}");
    }
}

#[test]
fn speed_three_halves_uses_wsola() {
    let x = sine(440.0, -6.0, 4.0, RATE);
    let speed = r(3, 2);
    let mut it = item("i1", "a", s(0), s(2));
    it.source_from = r(1, 2);
    it.speed = speed;
    let p = plan(s(0), s(3), vec![it]);
    let out = mix(
        &p,
        vec![("a", mem(stereo(x.clone(), x.clone())))],
        3 * 48_000,
    );
    // Length: exactly the item's span.
    let last = out.l.iter().rposition(|&v| v != 0.0).unwrap();
    assert!(last < 96_000 && last > 95_000, "last sound at {last}");
    assert!(out.l[96_000..].iter().all(|&v| v == 0.0));
    // Pitch kept.
    let hz = dominant_hz(&out.l[2_048..96_000 - 2_048], RATE);
    assert!((hz - 440.0).abs() <= 440.0 * 0.005, "{hz} Hz");
    // And it is WSOLA, anchored at sample_at(sourceFrom), bit for bit.
    let mut w = Wsola::new(speed);
    let mut want = Stereo::default();
    let tail = x[24_000..].to_vec();
    w.process(
        &Stereo {
            l: tail.clone(),
            r: tail,
        },
        &mut want,
    );
    assert!(want.len() >= 96_000);
    assert!(out.l[..96_000] == want.l[..96_000]);
    assert!(out.r[..96_000] == want.r[..96_000]);
}

#[test]
fn null_test_inverted_copy() {
    let x = noise(6, 2.0, RATE);
    let neg: Vec<f32> = x.iter().map(|v| -v).collect();
    let curve = |id: &str, asset: &str| {
        let mut it = item(id, asset, s(0), s(2));
        it.gain = vec![
            gain(
                s(0),
                r(1, 2),
                [-4.0, 0.0],
                vec![shape(ShapeKind::FadeIn, Curve::Linear)],
            ),
            gain(r(1, 2), s(2), [0.0, -9.0], vec![]),
        ];
        it.pan = vec![pan(s(0), s(2), [-0.3, 0.7])];
        it
    };
    let p = plan(s(0), s(2), vec![curve("i1", "x"), curve("i2", "neg")]);
    let src: Vec<(&str, Box<dyn PcmSource>)> = vec![
        ("x", mem(stereo(x.clone(), x.clone()))),
        ("neg", mem(stereo(neg.clone(), neg))),
    ];
    let out = mix(&p, src, 2 * 48_000);
    assert!(out.l.iter().chain(&out.r).all(|&v| v == 0.0));
    // And not because nothing played.
    let alone = mix(
        &plan(s(0), s(2), vec![curve("i1", "x")]),
        vec![("x", mem(stereo(x.clone(), x)))],
        2 * 48_000,
    );
    assert!(peak_dbfs(&alone.l) > -20.0);
}

/// Every feature at once, from a plan that does not start at 0.
fn busy_plan() -> (AudioPlan, Vec<(&'static str, Canonical)>) {
    let mut a = item("a", "st", r(1, 3), r(13, 5));
    a.source_from = r(1, 7);
    a.gain = vec![
        GainSegment {
            from: r(1, 3),
            to: r(4, 5),
            db: [-30.0, 2.0],
            interp: Interp::Bezier {
                x1: 0.42,
                y1: 0.0,
                x2: 0.58,
                y2: 1.0,
            },
            u: [0.1, 0.9],
            shapes: vec![shape(ShapeKind::FadeIn, Curve::EqualPower)],
        },
        GainSegment {
            from: r(4, 5),
            to: r(2, 1),
            db: [2.0, 2.0],
            interp: Interp::Hold,
            u: [0.0, 1.0],
            shapes: vec![],
        },
        gain(
            r(2, 1),
            r(13, 5),
            [-1.0, -1.0],
            vec![shape(ShapeKind::CrossOut, Curve::EqualPower)],
        ),
    ];
    a.pan = vec![
        pan(r(1, 3), r(1001, 1000), [-1.0, 1.0]),
        pan(r(1001, 1000), r(13, 5), [0.4, 0.4]),
    ];
    let mut b = item("b", "mo", r(2, 1), r(4, 1));
    b.speed = r(3, 2);
    b.source_from = r(1, 10);
    b.gain = vec![
        gain(
            r(2, 1),
            r(13, 5),
            [0.0, 0.0],
            vec![shape(ShapeKind::CrossIn, Curve::EqualPower)],
        ),
        gain(
            r(13, 5),
            r(4, 1),
            [0.0, -6.0],
            vec![shape(ShapeKind::FadeOut, Curve::Linear)],
        ),
    ];
    b.pan = vec![pan(r(2, 1), r(4, 1), [0.9, -0.9])];
    let mut c = item("c", "mo", r(7, 6), r(30_031, 10_000));
    c.speed = r(2, 3);
    c.effects = vec![eq_all_off()];
    let p = plan(r(1, 3), r(4, 1), vec![a, b, c]);
    let st = stereo(noise(7, 5.0, RATE), sine(330.0, -3.0, 5.0, RATE));
    let mo = mono(sweep(50.0, 15_000.0, 10.0, RATE));
    (p, vec![("st", st), ("mo", mo)])
}

#[test]
fn block_size_invariant() {
    let (p, srcs) = busy_plan();
    let frames = (sample_at(r(4, 1)).unwrap() - sample_at(r(1, 3)).unwrap()) as usize + 1_000;
    let mut outs = vec![];
    for block in [128, 1_000, 48_000] {
        let src = srcs.iter().map(|(k, c)| (*k, mem(c.clone()))).collect();
        let mut m = Mixer::new(&p, sources(src)).unwrap();
        assert_eq!(m.position(), 16_000);
        outs.push(render(&mut m, frames, block));
    }
    assert!(outs[0].l.iter().filter(|&&v| v != 0.0).count() > 100_000);
    for o in &outs[1..] {
        let bits = |x: &[f32]| x.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        assert!(bits(&o.l) == bits(&outs[0].l), "left");
        assert!(bits(&o.r) == bits(&outs[0].r), "right");
    }
}

#[test]
fn missing_source_is_silent_and_reported() {
    let x = vec![0.25f32; 48_000];
    let p = plan(
        s(0),
        s(1),
        vec![
            item("i1", "gone", s(0), s(1)),
            item("i2", "here", s(0), s(1)),
            item("i3", "gone", r(1, 2), s(1)),
        ],
    );
    let mut m = Mixer::new(&p, sources(vec![("here", mem(stereo(x.clone(), x)))])).unwrap();
    assert_eq!(m.missing(), ["gone".to_string()]);
    let out = render(&mut m, 48_000, 1_000);
    assert!(out.l.iter().all(|&v| v == 0.25));
}
