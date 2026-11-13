use fin_audio::channels::{Canonical, canonical, fold_mono};
use fin_media::audio_layout::{ChannelLayout, ChannelPos, ChannelPos::*};

const M3: f32 = 0.70710677;

fn expect_of(p: ChannelPos) -> (f32, f32) {
    match p {
        L => (1.0, 0.0),
        R => (0.0, 1.0),
        C => (M3, M3),
        Ls | Lb => (M3, 0.0),
        Rs | Rb => (0.0, M3),
        Cb => (0.5, 0.5),
        Lw => (M3, 0.0),
        Rw => (0.0, M3),
        Lfe => (0.0, 0.0),
        other => panic!("not in this test: {other:?}"),
    }
}

fn reason(e: &fin_core::CoreError) -> (String, String) {
    (
        e.code.to_string(),
        e.details["reason"].as_str().unwrap().to_string(),
    )
}

fn layouts() -> Vec<(String, ChannelLayout)> {
    let mut v = Vec::new();
    for n in 3..=8 {
        v.push((format!("wave default {n}"), ChannelLayout::wave_default(n)));
        v.push((format!("vorbis {n}"), ChannelLayout::vorbis_order(n)));
    }
    for c in 3..=7 {
        v.push((format!("aac {c}"), ChannelLayout::aac_config(c)));
    }
    for m in [0x7u32, 0x33, 0x37, 0x3F, 0x63F, 0x60F, 0x70F, 0xFF] {
        let n = m.count_ones() as usize;
        v.push((format!("mask {m:#x}"), ChannelLayout::wave_mask(m, n)));
    }
    v
}

#[test]
fn each_channel_isolated_per_layout() {
    for (name, layout) in layouts() {
        let n = layout.channels();
        for i in 0..n {
            let mut frame = vec![0.0f32; n];
            frame[i] = 1.0;
            let Canonical::Stereo(s) = canonical(&frame, &layout).unwrap() else {
                panic!()
            };
            let (l, r) = expect_of(layout.0[i]);
            assert_eq!(
                (s.l[0], s.r[0]),
                (l, r),
                "{name} channel {i} ({:?})",
                layout.0[i]
            );
        }
    }
}

/// Random frames of per-position values rendered in two layouts must downmix bit-identically.
fn same_positions_same_bits(a: &ChannelLayout, b: &ChannelLayout) {
    use fin_audio::testing::signals::Rng;
    let mut rng = Rng::new(0xC0FFEE);
    let (mut fa, mut fb) = (Vec::new(), Vec::new());
    for _ in 0..2000 {
        let vals: Vec<(ChannelPos, f32)> = a.0.iter().map(|p| (*p, rng.uniform())).collect();
        let at = |layout: &ChannelLayout, out: &mut Vec<f32>| {
            for p in &layout.0 {
                out.push(vals.iter().find(|(q, _)| q == p).unwrap().1);
            }
        };
        at(a, &mut fa);
        at(b, &mut fb);
    }
    assert_eq!(canonical(&fa, a).unwrap(), canonical(&fb, b).unwrap());
}

#[test]
fn vorbis_and_wave_51_differ_by_index_not_by_result() {
    same_positions_same_bits(
        &ChannelLayout::wave_default(6),
        &ChannelLayout::vorbis_order(6),
    );
    same_positions_same_bits(
        &ChannelLayout::wave_default(6),
        &ChannelLayout::aac_config(6),
    );
    same_positions_same_bits(
        &ChannelLayout::wave_mask(0x63F, 8),
        &ChannelLayout::vorbis_order(8),
    );
}

#[test]
fn mono_stays_mono() {
    let x = [0.25f32, -0.5, 0.125];
    assert_eq!(
        canonical(&x, &ChannelLayout::wave_default(1)).unwrap(),
        Canonical::Mono(x.to_vec())
    );
}

#[test]
fn stereo_passthrough_bit_exact() {
    let x = [0.1f32, 0.2, -0.3, 1e-30];
    let Canonical::Stereo(s) = canonical(&x, &ChannelLayout::wave_default(2)).unwrap() else {
        panic!()
    };
    assert_eq!((s.l, s.r), (vec![0.1, -0.3], vec![0.2, 1e-30]));
}

#[test]
fn unknown_position_unavailable() {
    let e = canonical(&[0.0; 3], &ChannelLayout(vec![L, R, Unknown])).unwrap_err();
    assert_eq!(reason(&e), ("CODEC_UNAVAILABLE".into(), "channels".into()));
}

#[test]
fn nine_channels_unavailable() {
    let e = canonical(&[0.0; 9], &ChannelLayout::wave_default(9)).unwrap_err();
    assert_eq!(reason(&e), ("CODEC_UNAVAILABLE".into(), "channels".into()));
    let e = canonical(&[0.0; 4], &ChannelLayout::aac_config(0)).unwrap_err();
    assert_eq!(reason(&e).1, "channels");
}

#[test]
fn mono_fold_is_half_sum() {
    let mut out = vec![9.0];
    fold_mono(&Canonical::Mono(vec![0.5, -1.0]), &mut out);
    assert_eq!(out, [0.5, -1.0]);
    let s = fin_audio::Stereo {
        l: vec![1.0, 0.25],
        r: vec![0.5, -0.25],
    };
    fold_mono(&Canonical::Stereo(s), &mut out);
    assert_eq!(out, [0.75, 0.0]);
}

#[test]
fn unknown_refused_at_any_count_and_two_channel_by_position() {
    for n in 1..=8 {
        let e = canonical(&vec![0.0; n], &ChannelLayout(vec![Unknown; n])).unwrap_err();
        assert_eq!(reason(&e).1, "channels", "{n}");
    }
    let top = ChannelLayout::wave_mask(1 << 11, 1);
    assert_eq!(reason(&canonical(&[0.0], &top).unwrap_err()).1, "channels");
    let Canonical::Stereo(s) = canonical(&[0.5, 0.25], &ChannelLayout(vec![L, Lfe])).unwrap()
    else {
        panic!()
    };
    assert_eq!((s.l, s.r), (vec![0.5], vec![0.0]));
}
