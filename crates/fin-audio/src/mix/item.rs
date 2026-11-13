//! One plan item in the mixer (plan 6C Decision 16): read the source (through WSOLA when the
//! speed is not 1), run the effect chain, apply gain × shapes and pan, add into the bus.
//!
//! **Timing (Decision 4).** The item sounds over timeline samples `[sample_at(from),
//! sample_at(to))`. Output sample `n` plays source frame `sample_at(sourceFrom) + (n −
//! sample_at(from))` at speed 1, and at other speeds what [`Wsola`] makes of the source from
//! that same integer anchor (its module doc: "Driving it from the mixer"), so the two paths
//! agree on where the item starts in its source.

use fin_core::eval::AudioItem;
use fin_core::{CoreError, Rational};

use crate::channels::Canonical;
use crate::effects::Chain;
use crate::mix::curves::{Control, Gain, Pan, gain_at, gain_curve, interval, pan_at, pan_curve};
use crate::mix::pan::sides;
use crate::mix::{PcmSource, SourceLayout};
use crate::time::range_end;
use crate::{Stereo, Wsola, sample_at};

fn internal(message: &str) -> CoreError {
    CoreError::new("INTERNAL", message.to_string())
}

/// Checks a source's answer: its own layout, exactly `frames` frames.
fn check_read(buf: &Canonical, layout: SourceLayout, frames: usize) -> Result<(), CoreError> {
    let ok = match (buf, layout) {
        (Canonical::Mono(m), SourceLayout::Mono) => m.len() == frames,
        (Canonical::Stereo(s), SourceLayout::Stereo) => s.l.len() == frames && s.r.len() == frames,
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(internal(
            "a PCM source answered with another layout or length than asked",
        ))
    }
}

/// The first source frame of a read of `frames` frames at `rel` frames past `anchor`;
/// `TIME_OVERFLOW` unless the whole range's exclusive end fits an `i64`.
fn read_at(anchor: i64, rel: u64, frames: usize) -> Result<i64, CoreError> {
    let at = range_end(anchor, rel)?;
    range_end(at, frames as u64)?;
    Ok(at)
}

/// The stretch path of an item: its WSOLA, the source frames pushed so far (relative to the
/// anchor) and the output frames made but not yet played.
struct Stretch {
    w: Wsola,
    pushed: u64,
    /// Output frames `[base, base + pending.len())` of the item.
    pending: Stereo,
    base: u64,
    feed: Canonical,
    feed_st: Stereo,
}

impl Stretch {
    fn new(speed: Rational) -> Stretch {
        Stretch {
            w: Wsola::new(speed),
            pushed: 0,
            pending: Stereo::default(),
            base: 0,
            feed: Canonical::Stereo(Stereo::default()),
            feed_st: Stereo::default(),
        }
    }

    /// Item output frames `[rel, rel + n)` into `buf` (in the source's layout). Calls come in
    /// order; frames before `rel` not yet played are made and dropped (an item is always run
    /// from its first output frame).
    fn take(
        &mut self,
        src: &mut dyn PcmSource,
        anchor: i64,
        layout: SourceLayout,
        rel: u64,
        n: usize,
        buf: &mut Canonical,
    ) -> Result<(), CoreError> {
        let need = self.w.input_needed(rel + n as u64);
        if need > self.pushed {
            let k = usize::try_from(need - self.pushed)
                .map_err(|_| internal("a stretch read does not fit memory"))?;
            src.read(read_at(anchor, self.pushed, k)?, k, &mut self.feed)?;
            check_read(&self.feed, layout, k)?;
            self.feed_st.clear();
            match &self.feed {
                // A mono source is stretched as both channels and read from `l` (Wsola docs).
                Canonical::Mono(m) => {
                    self.feed_st.l.extend_from_slice(m);
                    self.feed_st.r.extend_from_slice(m);
                }
                Canonical::Stereo(s) => {
                    self.feed_st.l.extend_from_slice(&s.l);
                    self.feed_st.r.extend_from_slice(&s.r);
                }
            }
            self.w.process(&self.feed_st, &mut self.pending);
            self.pushed = need;
        }
        assert!(rel >= self.base, "stretch reads come in order");
        let skip = usize::try_from(rel - self.base).expect("pending fits memory");
        assert!(
            self.pending.len() >= skip + n,
            "WSOLA emitted what input_needed promised"
        );
        let (l, r) = (
            &self.pending.l[skip..skip + n],
            &self.pending.r[skip..skip + n],
        );
        match layout {
            SourceLayout::Mono => *buf = Canonical::Mono(l.to_vec()),
            SourceLayout::Stereo => {
                *buf = Canonical::Stereo(Stereo {
                    l: l.to_vec(),
                    r: r.to_vec(),
                })
            }
        }
        self.pending.l.drain(..skip + n);
        self.pending.r.drain(..skip + n);
        self.base = rel + n as u64;
        Ok(())
    }
}

/// An item's mixing state.
pub(crate) struct ItemMix {
    pub asset: String,
    /// `None` when the asset has no source (silent; reported by `Mixer::missing`).
    layout: Option<SourceLayout>,
    pub start: i64,
    pub end: i64,
    anchor: i64,
    gain: Gain,
    pan: Pan,
    chain: Chain,
    stretch: Option<Stretch>,
    buf: Canonical,
}

impl ItemMix {
    pub fn new(item: &AudioItem, layout: Option<SourceLayout>) -> Result<ItemMix, CoreError> {
        if !item.speed.is_positive() {
            return Err(internal("an audio item's speed is not positive"));
        }
        let (start, end) = (sample_at(item.from)?, sample_at(item.to)?);
        let unity = item.speed == Rational::from_int(1);
        Ok(ItemMix {
            asset: item.asset.clone(),
            layout,
            start,
            end,
            anchor: sample_at(item.source_from)?,
            gain: gain_curve(&item.gain, start, end)?,
            pan: pan_curve(&item.pan, start, end)?,
            chain: Chain::new(item, layout.unwrap_or(SourceLayout::Stereo))?,
            stretch: (!unity).then(|| Stretch::new(item.speed)),
            buf: Canonical::Stereo(Stereo::default()),
        })
    }

    /// The item's effects the chain skipped (unknown kinds).
    pub fn skipped_effects(&self) -> &[crate::effects::SkippedEffect] {
        self.chain.skipped()
    }

    /// Adds the item's part of the block `[b0, b1)` (`b1 − b0 = out.len()`, checked by the
    /// mixer) into `out`.
    pub fn render(
        &mut self,
        src: &mut dyn PcmSource,
        b0: i64,
        b1: i64,
        out: &mut Stereo,
    ) -> Result<(), CoreError> {
        let Some(layout) = self.layout else {
            return Ok(());
        };
        let (s, e) = (self.start.max(b0), self.end.min(b1));
        if s >= e {
            return Ok(());
        }
        let n = (e - s) as usize;
        let rel = (s - self.start) as u64;
        match &mut self.stretch {
            None => {
                src.read(read_at(self.anchor, rel, n)?, n, &mut self.buf)?;
                check_read(&self.buf, layout, n)?;
            }
            Some(st) => st.take(src, self.anchor, layout, rel, n, &mut self.buf)?,
        }
        self.chain.process(s, &mut self.buf);
        self.apply(layout, s, e, b0, out);
        Ok(())
    }

    /// Gain × shapes and pan, ramped between control points, added into `out`.
    fn apply(&self, layout: SourceLayout, s: i64, e: i64, b0: i64, out: &mut Stereo) {
        let mut n = s;
        while n < e {
            let (g, p) = (self.gain.locate(n), self.pan.locate(n));
            let (c0, c1) = interval(n, &g, &p);
            let a = sides(layout, gain_at(&g, c0), pan_at(&p, c0));
            let b = sides(layout, gain_at(&g, c1), pan_at(&p, c1));
            let ctl = Control {
                c0,
                c1,
                l: [a.0, b.0],
                r: [a.1, b.1],
            };
            let stop = c1.min(e);
            for m in n..stop {
                let (gl, gr) = ctl.at(m);
                let (i, o) = ((m - s) as usize, (m - b0) as usize);
                let (xl, xr) = match &self.buf {
                    Canonical::Mono(x) => (x[i], x[i]),
                    Canonical::Stereo(st) => (st.l[i], st.r[i]),
                };
                out.l[o] += xl * gl;
                out.r[o] += xr * gr;
            }
            n = stop;
        }
    }
}
