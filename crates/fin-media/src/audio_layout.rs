//! Channel positions and per-format channel orders (6C plan Decision 3). Decoders return their
//! frames in the container's own order together with a [`ChannelLayout`]; `fin-audio` maps by
//! position, never by index. Pure tables: no I/O.

/// Where a channel sits in the room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelPos {
    L,
    R,
    C,
    Lfe,
    /// Left surround (side, or back when the layout has no sides).
    Ls,
    Rs,
    /// Left back, in layouts that also have sides.
    Lb,
    Rb,
    /// Back centre.
    Cb,
    /// Front left/right of centre (WAVE bits 6/7, AAC Lc/Rc).
    Lw,
    Rw,
    /// A position this engine has no rule for (the layout is refused).
    Unknown,
}

/// The positions of an interleaved frame, in the decoder's channel order.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ChannelLayout(pub Vec<ChannelPos>);

impl ChannelLayout {
    pub fn channels(&self) -> usize {
        self.0.len()
    }

    fn of(p: &[ChannelPos]) -> Self {
        ChannelLayout(p.to_vec())
    }

    /// `n` channels of unknown position (what a table returns outside its range).
    pub fn unknown(n: usize) -> Self {
        ChannelLayout(vec![ChannelPos::Unknown; n])
    }

    /// The WAVE default order for `n` channels (`dwChannelMask` absent): 1 C, 2 L R, 3 L R C,
    /// 4 quad, 5 L R C Ls Rs, 5.1, 7 = 6.1, 8 = 7.1 (Windows' default masks); other counts
    /// are all `Unknown`.
    pub fn wave_default(n: usize) -> Self {
        match n {
            1 => Self::of(&[ChannelPos::C]),
            2 => Self::of(&[ChannelPos::L, ChannelPos::R]),
            3 => Self::wave_mask(0x7, 3),
            4 => Self::wave_mask(0x33, 4),
            5 => Self::wave_mask(0x37, 5),
            6 => Self::wave_mask(0x3F, 6),
            7 => Self::wave_mask(0x70F, 7),
            8 => Self::wave_mask(0x63F, 8),
            _ => Self::unknown(n),
        }
    }

    /// A `WAVEFORMATEXTENSIBLE` `dwChannelMask` for `n` channels: one channel per set bit,
    /// lowest bit first; set bits beyond the `n`th are ignored and channels beyond the mask's
    /// popcount are `Unknown`. Mask 0 falls back to [`ChannelLayout::wave_default`]. Back
    /// left/right (bits 4, 5) are `Ls`/`Rs` unless the mask also has side left/right (bits 9,
    /// 10), when they are `Lb`/`Rb`. Bits 11 and up (the top speakers) are `Unknown`.
    pub fn wave_mask(mask: u32, n: usize) -> Self {
        use ChannelPos::*;
        if mask == 0 {
            return Self::wave_default(n);
        }
        let sides = mask & (1 << 9) != 0 && mask & (1 << 10) != 0;
        let mut out = Vec::new();
        for bit in 0..32 {
            if mask & (1 << bit) == 0 {
                continue;
            }
            out.push(match bit {
                0 => L,
                1 => R,
                2 => C,
                3 => Lfe,
                4 if sides => Lb,
                4 => Ls,
                5 if sides => Rb,
                5 => Rs,
                6 => Lw,
                7 => Rw,
                8 => Cb,
                9 => Ls,
                10 => Rs,
                _ => Unknown,
            });
        }
        out.truncate(n);
        out.resize(n, Unknown);
        ChannelLayout(out)
    }

    /// Vorbis order (RFC 7845 §5.1.1.2; Opus mapping family 1) for `n` channels, 1 to 8.
    pub fn vorbis_order(n: usize) -> Self {
        use ChannelPos::*;
        match n {
            1 => Self::of(&[C]),
            2 => Self::of(&[L, R]),
            3 => Self::of(&[L, C, R]),
            4 => Self::of(&[L, R, Ls, Rs]),
            5 => Self::of(&[L, C, R, Ls, Rs]),
            6 => Self::of(&[L, C, R, Ls, Rs, Lfe]),
            7 => Self::of(&[L, C, R, Ls, Rs, Cb, Lfe]),
            8 => Self::of(&[L, C, R, Ls, Rs, Lb, Rb, Lfe]),
            _ => Self::unknown(n),
        }
    }

    /// AAC `channelConfiguration` (ISO/IEC 14496-3 Table 1.19), 1 to 7. Configuration 7 is
    /// the front-wide 7.1 (SCE CPE CPE CPE LFE: C, Lc, Rc, L, R, Ls, Rs, LFE); the plan text
    /// said back 7.1, which Table 1.19 does not have (deviation recorded by the orchestrator). Other values give an empty layout (refused by the conversion).
    pub fn aac_config(config: usize) -> Self {
        use ChannelPos::*;
        match config {
            1 => Self::of(&[C]),
            2 => Self::of(&[L, R]),
            3 => Self::of(&[C, L, R]),
            4 => Self::of(&[C, L, R, Cb]),
            5 => Self::of(&[C, L, R, Ls, Rs]),
            6 => Self::of(&[C, L, R, Ls, Rs, Lfe]),
            7 => Self::of(&[C, Lw, Rw, L, R, Ls, Rs, Lfe]),
            _ => ChannelLayout(Vec::new()),
        }
    }
}
