//! Where decoded audio meets `fin-audio` (6C plan Decision 3, Frozen interfaces): the only place
//! in the worker that turns a decoder's frames into the mixer's canonical feed.
//!
//! - [`DecodedSource`] reads an original through `fin-codecs`' [`AudioReader`] (priming and
//!   padding already trimmed, the source's own rate and channel order), maps it by position to
//!   the canonical layout ([`fin_audio::channels::canonical`]: mono stays one channel, 3 to 8
//!   channels are downmixed with BS.775), and resamples each channel to 48 kHz, or folds it to
//!   mono ([`fin_audio::channels::fold_mono`]) and resamples to 16 kHz for the analysis feed.
//!   Output frame 0 is the first audible source sample and the length is exactly
//!   [`fin_audio::resampled_len`] of the source's frames.
//! - [`CachedSource`] reads a `pcm48` (or `pcm16m`) cache entry's chunks: `meta.json` must
//!   recompute its key and describe a PCM entry of this build ([`PcmEntry::of`]), and each chunk
//!   is opened no-follow (`fin_shm::checked_open`, capped at its listed size), read whole and
//!   checked against its listed size and BLAKE3 before a sample of it is used.
//!
//! Both are [`PcmSource`]s. A `DecodedSource` decodes forward only: a read before the frames it
//! still holds is `INTERNAL`, so each mixer item reads its own one (the mixer reads each source
//! forward). It drops decoded frames before the first one asked for as it goes, so a read far
//! into the source holds about one packet beyond the read, not everything before it.
//!
//! `media.pcm` streams a `DecodedSource`; the mixer's readers (`media.waveform`, `media.listen`:
//! Tasks 19 and 21) are the first callers of `CachedSource` and of the `PcmSource` reads.
// The `CachedSource` half is read by the tests until Tasks 19 and 21 call it.
#![cfg_attr(not(test), allow(dead_code))]

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use fin_audio::channels::{Canonical, canonical, fold_mono};
use fin_audio::{PcmSource, Resampler, SourceLayout, Stereo};
use fin_codecs::audio::{AudioLimits, AudioReader};
use fin_core::CoreError;
use fin_media::AudioTrackInfo;
use fin_media::backend::VendorClass;
use fin_media::cache::{CacheKey, EntryMeta, META_FILE};
use fin_media::jobs::{MAX_PCM_SECONDS, PcmEntry, PcmLayout, PcmRecipe};
use serde_json::json;

/// Spare capacity a decoded source keeps before giving memory back (frames).
const SHRINK_SLACK: usize = 16_384;

/// The most `meta.json` a worker reads for a PCM entry (its chunk list is long for long sources).
const META_CAP: u64 = fin_media::cache::MAX_META_BYTES;

fn internal(message: impl Into<String>) -> CoreError {
    CoreError::new("INTERNAL", message)
}

/// The output channels of a feed (one or two planar buffers).
#[derive(Default)]
struct Planar {
    l: Vec<f32>,
    r: Vec<f32>,
}

impl Planar {
    fn len(&self) -> usize {
        self.l.len()
    }
}

/// An original's audio as a canonical feed at 48 kHz, or folded to mono at 16 kHz.
pub struct DecodedSource {
    reader: AudioReader,
    recipe: PcmRecipe,
    layout: PcmLayout,
    /// One resampler per output channel.
    resamplers: Vec<Resampler>,
    source_rate: u32,
    /// Resampled frames not handed out yet; `held.l[0]` is output frame `base`.
    held: Planar,
    base: i64,
    ended: bool,
    scratch: Vec<f32>,
}

impl DecodedSource {
    /// `file`'s first audio track as the `recipe` feed; the reader's own errors
    /// (`UNSUPPORTED_CONTAINER`, `MEDIA_CORRUPT`, `CODEC_UNAVAILABLE`, `MEDIA_LIMIT`).
    pub fn open(file: File, recipe: PcmRecipe) -> Result<DecodedSource, CoreError> {
        // At most the longest feed the cache can list (refused from a declared length here,
        // before anything is decoded; bounded while decoding otherwise).
        let limits = AudioLimits {
            max_seconds: MAX_PCM_SECONDS,
            ..AudioLimits::default()
        };
        let reader = AudioReader::open(file, &limits)?;
        let info = reader.info().clone();
        let layout = recipe.layout(info.channels);
        let resamplers = (0..layout.channels())
            .map(|_| Resampler::new(info.sample_rate, recipe.rate()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(DecodedSource {
            reader,
            recipe,
            layout,
            resamplers,
            source_rate: info.sample_rate,
            held: Planar::default(),
            base: 0,
            ended: false,
            scratch: Vec::new(),
        })
    }

    pub fn info(&self) -> &AudioTrackInfo {
        self.reader.info()
    }

    pub fn feed_layout(&self) -> PcmLayout {
        self.layout
    }

    pub fn backend(&self) -> &'static str {
        self.reader.backend().0
    }

    pub fn decoder_class(&self) -> VendorClass {
        self.reader.decoder_class()
    }

    /// The frames the source holds allocated (its buffer's capacity): bounded by a read's
    /// length plus one packet, whatever the position read (6C Task 21 review B1).
    pub fn held_capacity(&self) -> usize {
        self.held.l.capacity().max(self.held.r.capacity())
    }

    /// The output frames the source will have, when its container states its length.
    pub fn expected_frames(&self) -> Option<u64> {
        self.reader
            .declared_frames()
            .map(|n| fin_audio::resampled_len(n, self.source_rate, self.recipe.rate()))
    }

    /// Decodes the next packet's frames into `held`; `false` once the stream is done.
    fn pull(&mut self) -> Result<bool, CoreError> {
        if self.ended {
            return Ok(false);
        }
        let Some(d) = self.reader.next()? else {
            for (i, r) in self.resamplers.iter_mut().enumerate() {
                let out = if i == 0 {
                    &mut self.held.l
                } else {
                    &mut self.held.r
                };
                r.finish(out);
            }
            self.ended = true;
            return Ok(true);
        };
        if d.rate != self.source_rate {
            return Err(CoreError::new(
                "MEDIA_CORRUPT",
                format!(
                    "the audio changes rate mid-stream ({} Hz, then {} Hz)",
                    self.source_rate, d.rate
                ),
            )
            .with_details(json!({ "at": d.pts, "reason": "rateChange" })));
        }
        let c = canonical(&d.interleaved, &d.layout)?;
        match (self.layout, &c) {
            (PcmLayout::Mono, _) if self.recipe == PcmRecipe::Pcm16m => {
                fold_mono(&c, &mut self.scratch);
                self.resamplers[0].process(&self.scratch, &mut self.held.l);
            }
            (PcmLayout::Mono, Canonical::Mono(m)) => {
                self.resamplers[0].process(m, &mut self.held.l);
            }
            (PcmLayout::Stereo, Canonical::Stereo(s)) => {
                self.resamplers[0].process(&s.l, &mut self.held.l);
                self.resamplers[1].process(&s.r, &mut self.held.r);
            }
            _ => {
                return Err(CoreError::new(
                    "MEDIA_CORRUPT",
                    "the audio changes its channel count mid-stream",
                )
                .with_details(json!({ "at": d.pts, "reason": "channelsChange" })));
            }
        }
        Ok(true)
    }

    /// The next output frames in order (at least one), `None` after the last. Interleaved
    /// `L R` for a stereo feed, plain samples for a mono one.
    pub fn next_interleaved(&mut self) -> Result<Option<Vec<f32>>, CoreError> {
        while self.held.len() == 0 {
            if !self.pull()? {
                return Ok(None);
            }
        }
        let n = self.held.len();
        let out = match self.layout {
            PcmLayout::Mono => std::mem::take(&mut self.held.l),
            PcmLayout::Stereo => {
                let mut v = Vec::with_capacity(2 * n);
                for (l, r) in self.held.l.iter().zip(&self.held.r) {
                    v.push(*l);
                    v.push(*r);
                }
                self.held.l.clear();
                self.held.r.clear();
                v
            }
        };
        self.base += n as i64;
        Ok(Some(out))
    }
}

/// Copies `src[from..]` into `dst` at `to` (both clipped), the rest of `dst` untouched.
fn copy_into(src: &[f32], src_at: i64, dst: &mut [f32], dst_at: i64) {
    let start = src_at.max(dst_at);
    let end = (src_at + src.len() as i64).min(dst_at + dst.len() as i64);
    for k in start..end {
        dst[(k - dst_at) as usize] = src[(k - src_at) as usize];
    }
}

fn canonical_of(layout: PcmLayout, l: Vec<f32>, r: Vec<f32>) -> Canonical {
    match layout {
        PcmLayout::Mono => Canonical::Mono(l),
        PcmLayout::Stereo => Canonical::Stereo(Stereo { l, r }),
    }
}

fn source_layout(layout: PcmLayout) -> SourceLayout {
    match layout {
        PcmLayout::Mono => SourceLayout::Mono,
        PcmLayout::Stereo => SourceLayout::Stereo,
    }
}

impl PcmSource for DecodedSource {
    fn layout(&self) -> SourceLayout {
        source_layout(self.layout)
    }

    fn read(&mut self, at: i64, frames: usize, out: &mut Canonical) -> Result<(), CoreError> {
        let first = at.max(0);
        if first < self.base && at.saturating_add(frames as i64) > first {
            return Err(internal(
                "a decoded source reads forward only: the frames asked for were dropped",
            ));
        }
        let end = at.saturating_add(frames as i64);
        // Frames before the first one asked for are dropped as they are decoded, never held: a
        // read far into the source holds about one packet beyond the read (review B1).
        self.forget_before(first);
        while self.base + (self.held.len() as i64) < end && self.pull()? {
            self.forget_before(first);
        }
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        copy_into(&self.held.l, self.base, &mut l, at);
        if self.layout == PcmLayout::Stereo {
            copy_into(&self.held.r, self.base, &mut r, at);
        }
        // Frames before `at` are never read again.
        self.forget_before(at);
        *out = canonical_of(self.layout, l, r);
        Ok(())
    }
}

impl DecodedSource {
    /// Drops the held frames before output frame `at` and gives back the memory a large buffer
    /// no longer needs.
    fn forget_before(&mut self, at: i64) {
        let drop = (at - self.base).clamp(0, self.held.len() as i64) as usize;
        if drop > 0 {
            self.held.l.drain(..drop);
            if self.layout == PcmLayout::Stereo {
                self.held.r.drain(..drop);
            }
            self.base += drop as i64;
        }
        for v in [&mut self.held.l, &mut self.held.r] {
            if v.capacity() > 2 * v.len() + SHRINK_SLACK {
                v.shrink_to(v.len() + SHRINK_SLACK / 2);
            }
        }
    }
}

/// `PLAN_MISMATCH {reason: cacheEntry, file?}` for an entry that is not what it says.
fn bad_entry(message: impl Into<String>, file: Option<&str>) -> CoreError {
    let mut details = json!({ "reason": "cacheEntry" });
    if let Some(f) = file {
        details["file"] = json!(f);
    }
    CoreError::new("PLAN_MISMATCH", message).with_details(details)
}

/// Reads `path` whole through the checked open, at most `cap` bytes.
fn read_checked(path: &Path, cap: u64) -> Result<Vec<u8>, CoreError> {
    let mut f = fin_shm::checked_open(path, cap)?;
    let mut bytes = Vec::new();
    f.by_ref()
        .take(cap.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| CoreError::new("IO_ERROR", e.to_string()))?;
    Ok(bytes)
}

/// A PCM cache entry's chunks as a [`PcmSource`] (module docs).
pub struct CachedSource {
    dir: PathBuf,
    meta: EntryMeta,
    entry: PcmEntry,
    /// The chunk held decoded, as `(index, samples)`.
    chunk: Option<(usize, Vec<f32>)>,
}

impl CachedSource {
    /// The entry in `dir` whose key is `key`: its `meta.json` checked (module docs).
    pub fn open(dir: &Path, key: &str) -> Result<CachedSource, CoreError> {
        let bytes = read_checked(&dir.join(META_FILE), META_CAP)?;
        let meta: EntryMeta = serde_json::from_slice(&bytes)
            .map_err(|e| bad_entry(format!("meta.json: {e}"), Some(META_FILE)))?;
        let checked = meta.check()?;
        if CacheKey::parse(key)? != checked {
            return Err(bad_entry("the entry is not the one asked for", None));
        }
        let entry = PcmEntry::of(&meta)?;
        Ok(CachedSource {
            dir: dir.to_path_buf(),
            meta,
            entry,
            chunk: None,
        })
    }

    pub fn entry(&self) -> &PcmEntry {
        &self.entry
    }

    /// The entry's `meta.json` as checked by [`CachedSource::open`].
    pub fn meta(&self) -> &EntryMeta {
        &self.meta
    }

    /// Chunk `i`'s samples, read and verified on first use.
    fn chunk(&mut self, i: usize) -> Result<&[f32], CoreError> {
        if self.chunk.as_ref().is_none_or(|(k, _)| *k != i) {
            let f = &self.meta.files[i];
            let bytes = read_checked(&self.dir.join(&f.name), f.bytes)?;
            if bytes.len() as u64 != f.bytes
                || blake3::hash(&bytes).to_hex().as_str() != f.blake3.as_str()
            {
                return Err(bad_entry(
                    "a chunk is not its listed size and digest",
                    Some(&f.name),
                ));
            }
            let samples = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect();
            self.chunk = Some((i, samples));
        }
        Ok(self
            .chunk
            .as_ref()
            .map(|(_, s)| s.as_slice())
            .unwrap_or(&[]))
    }
}

impl PcmSource for CachedSource {
    fn layout(&self) -> SourceLayout {
        source_layout(self.entry.layout)
    }

    fn read(&mut self, at: i64, frames: usize, out: &mut Canonical) -> Result<(), CoreError> {
        let layout = self.entry.layout;
        let ch = layout.channels();
        let per = layout.chunk_frames() as i64;
        let total = self.entry.frames as i64;
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        let start = at.max(0);
        let end = at.saturating_add(frames as i64).min(total);
        let mut k = start;
        while k < end {
            let i = (k / per) as usize;
            let in_chunk = k - i as i64 * per;
            let take = (end - k).min(per - in_chunk);
            let samples = self.chunk(i)?;
            for j in 0..take {
                let s = ((in_chunk + j) as usize) * ch;
                let o = (k + j - at) as usize;
                l[o] = samples[s];
                if ch == 2 {
                    r[o] = samples[s + 1];
                }
            }
            k += take;
        }
        *out = canonical_of(layout, l, r);
        Ok(())
    }
}

#[cfg(test)]
#[path = "audio_src_tests.rs"]
mod tests;
