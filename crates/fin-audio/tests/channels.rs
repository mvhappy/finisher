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
    for m in [0x7u32, 0x33, 0x37, 0x3F, 0x63F, 0x60F, 0x70F] {
        v.push((format!("mask {m:#x}"), ChannelLayout::wave_mask(m)));
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

#[test]
fn vorbis_and_wave_51_differ_by_index_not_by_result() {
    let w = ChannelLayout::wave_default(6); // L R C LFE Ls Rs
    let v = ChannelLayout::vorbis_order(6); // L C R Ls Rs LFE
    let wf = [0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6];
    let vf = [0.1f32, 0.3, 0.2, 0.5, 0.6, 0.4];
    assert_eq!(canonical(&wf, &w).unwrap(), canonical(&vf, &v).unwrap());
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
