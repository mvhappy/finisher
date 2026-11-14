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
    for (i, d) in dst.iter_mut().enumerate() {
        *d = match at.checked_add(i as i64) {
            Some(k) if k >= 0 && (k as u64) < src.len() as u64 => src[k as usize],
            _ => 0.0,
        };
    }
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
            Canonical::Mono(m) => {
                let mut v = vec![0.0; frames];
                copy_at(m, at, &mut v);
                *out = Canonical::Mono(v);
            }
            Canonical::Stereo(s) => {
                let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
                copy_at(&s.l, at, &mut l);
                copy_at(&s.r, at, &mut r);
                *out = Canonical::Stereo(crate::Stereo { l, r });
            }
        }
        Ok(())
    }
}
