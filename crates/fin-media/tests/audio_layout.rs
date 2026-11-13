use fin_media::audio_layout::{ChannelLayout, ChannelPos::*};

#[test]
fn wave_defaults() {
    assert_eq!(ChannelLayout::wave_default(2).0, [L, R]);
    assert_eq!(ChannelLayout::wave_default(3).0, [L, R, C]);
    assert_eq!(ChannelLayout::wave_default(4).0, [L, R, Ls, Rs]);
    assert_eq!(ChannelLayout::wave_default(6).0, [L, R, C, Lfe, Ls, Rs]);
    assert_eq!(
        ChannelLayout::wave_default(8).0,
        [L, R, C, Lfe, Lb, Rb, Ls, Rs]
    );
    assert_eq!(ChannelLayout::wave_default(9).0, vec![Unknown; 9]);
}

#[test]
fn wave_mask_bits_in_order() {
    assert_eq!(ChannelLayout::wave_mask(0x603, 4).0, [L, R, Ls, Rs]);
    assert_eq!(ChannelLayout::wave_mask(1 << 12, 1).0, [Unknown]);
}

#[test]
fn vorbis_and_aac_orders() {
    assert_eq!(ChannelLayout::vorbis_order(6).0, [L, C, R, Ls, Rs, Lfe]);
    assert_eq!(ChannelLayout::aac_config(6).0, [C, L, R, Ls, Rs, Lfe]);
    // ISO/IEC 14496-3 Table 1.19 config 7: SCE CPE CPE CPE LFE (front-wide 7.1).
    assert_eq!(
        ChannelLayout::aac_config(7).0,
        [C, Lw, Rw, L, R, Ls, Rs, Lfe]
    );
    assert_eq!(ChannelLayout::aac_config(8).channels(), 0);
    assert_eq!(ChannelLayout::aac_config(12).channels(), 0);
    assert_eq!(ChannelLayout::aac_config(0).channels(), 0);
}

#[test]
fn wave_mask_count_and_fallback() {
    // Bits beyond n are ignored, channels beyond the popcount are Unknown, mask 0 is the default.
    assert_eq!(ChannelLayout::wave_mask(0x3F, 2).0, [L, R]);
    assert_eq!(ChannelLayout::wave_mask(0x3, 4).0, [L, R, Unknown, Unknown]);
    assert_eq!(
        ChannelLayout::wave_mask(0, 6),
        ChannelLayout::wave_default(6)
    );
    assert_eq!(
        ChannelLayout::wave_mask(0xFF, 8).0,
        [L, R, C, Lfe, Ls, Rs, Lw, Rw]
    );
}
