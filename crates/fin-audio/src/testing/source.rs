//! `MemSource`: a [`PcmSource`] over an in-memory canonical signal (mono or stereo).

use fin_core::CoreError;

use crate::channels::Canonical;
use crate::mix::{PcmSource, SourceLayout};

/// Frame `i` of the signal is source frame `i`; frames outside read as zero.
pub struct MemSource(pub Canonical);

impl MemSource {
    pub fn new(c: Canonical) -> MemSource {
        MemSource(c)
    }

    pub fn boxed(c: Canonical) -> Box<dyn PcmSource> {
        Box::new(MemSource(c))
    }
}

/// Copies `src[at .. at + dst.len()]` into `dst`, zeros outside `src`.
pub fn copy_at(src: &[f32], at: i64, dst: &mut [f32]) {
    dst.fill(0.0);
    // The part of `dst` that lies inside `src`, in i128 so no edge can overflow.
    let len = dst.len() as i128;
    let lo = (-i128::from(at)).clamp(0, len);
    let hi = (src.len() as i128 - i128::from(at)).clamp(lo, len);
    if lo < hi {
        let from = (i128::from(at) + lo) as usize;
        let (lo, hi) = (lo as usize, hi as usize);
        dst[lo..hi].copy_from_slice(&src[from..from + (hi - lo)]);
    }
}

/// `out` as a mono buffer of `frames` frames, reusing its allocation.
fn mono(out: &mut Canonical, frames: usize) -> &mut Vec<f32> {
    if !matches!(out, Canonical::Mono(_)) {
        *out = Canonical::Mono(Vec::new());
    }
    let Canonical::Mono(m) = out else {
        unreachable!()
    };
    m.resize(frames, 0.0);
    m
}

/// `out` as a stereo buffer of `frames` frames, reusing its allocation.
fn stereo(out: &mut Canonical, frames: usize) -> &mut crate::Stereo {
    if !matches!(out, Canonical::Stereo(_)) {
        *out = Canonical::Stereo(crate::Stereo::default());
    }
    let Canonical::Stereo(s) = out else {
        unreachable!()
    };
    s.l.resize(frames, 0.0);
    s.r.resize(frames, 0.0);
    s
}

impl PcmSource for MemSource {
    fn layout(&self) -> SourceLayout {
        match self.0 {
            Canonical::Mono(_) => SourceLayout::Mono,
            Canonical::Stereo(_) => SourceLayout::Stereo,
        }
    }

    fn read(&mut self, at: i64, frames: usize, out: &mut Canonical) -> Result<(), CoreError> {
        match &self.0 {
            Canonical::Mono(m) => copy_at(m, at, mono(out, frames)),
            Canonical::Stereo(s) => {
                let o = stereo(out, frames);
                copy_at(&s.l, at, &mut o.l);
                copy_at(&s.r, at, &mut o.r);
            }
        }
        Ok(())
    }
}
