//! Random access by presentation index: `FrameReader` decodes from the sync sample
//! `decode_start` names, keeps the frames it decoded since (up to a cap), and continues
//! forward when the next request is ahead in the same run or still in the decoder (a
//! frame-threaded decoder holds several frames in flight).

use std::collections::VecDeque;
use std::io::{Read, Seek};

use fin_container::{Demuxer, Limits, VideoTrack};
use fin_core::CoreError;
use fin_media::{MediaInfo, SampleTable, decode_start};
use serde_json::json;

use crate::av1::DecodeThreads;
use crate::caps::{Capabilities, open_decoder_limited};
use crate::frame::DecodedFrame;
use crate::traits::{BackendId, VideoDecoder};

/// Decoded frames kept for reuse.
const CACHE: usize = 64;

/// The bytes of a frame's planes.
fn frame_bytes(f: &DecodedFrame) -> usize {
    f.planes.iter().map(|p| p.data.len()).sum()
}

/// A demuxer, a decoder and the track's tables.
pub struct FrameReader<R: Read + Seek> {
    demux: Demuxer<R>,
    track: VideoTrack,
    decoder: Box<dyn VideoDecoder>,
    /// The run in progress: the sample it began at (a sync sample) and the next one to send,
    /// both in decode order.
    run: Option<(usize, usize)>,
    /// Composition times sent in this run whose frames have not come out yet.
    in_flight: VecDeque<i64>,
    /// The run reached the last sample and the decoder was told the stream ended.
    ended: bool,
    cache: VecDeque<DecodedFrame>,
    /// The bytes of the frames in `cache`.
    cached: usize,
    /// The most bytes `cache` keeps (the newest frame is always kept).
    cache_cap: usize,
    buf: Vec<u8>,
}

impl<R: Read + Seek> FrameReader<R> {
    /// Opens the file's first video track with a decoder from `caps` (default threading).
    pub fn open(r: R, limits: &Limits, caps: &Capabilities) -> Result<Self, CoreError> {
        Self::open_with(r, limits, caps, DecodeThreads::default())
    }

    /// [`FrameReader::open`] with the decoder's threads and frames in flight chosen.
    pub fn open_with(
        r: R,
        limits: &Limits,
        caps: &Capabilities,
        threads: DecodeThreads,
    ) -> Result<Self, CoreError> {
        Self::open_with_limit(r, limits, caps, threads, crate::av1::MAX_PIXELS)
    }

    /// [`FrameReader::open_with`] refusing pictures of more than `max_pixels` pixels
    /// (`DECODE_FAILED` before the decoder allocates for them).
    pub fn open_with_limit(
        r: R,
        limits: &Limits,
        caps: &Capabilities,
        threads: DecodeThreads,
        max_pixels: u32,
    ) -> Result<Self, CoreError> {
        let demux = Demuxer::open(r, limits)?;
        let track = demux.video().cloned().ok_or_else(|| {
            CoreError::new("NOT_FOUND", "the file has no video track")
                .with_details(json!({ "kind": "videoTrack" }))
        })?;
        let decoder = open_decoder_limited(&track, caps, threads, max_pixels)?;
        Ok(FrameReader {
            demux,
            track,
            decoder,
            run: None,
            in_flight: VecDeque::new(),
            ended: false,
            cache: VecDeque::new(),
            cached: 0,
            cache_cap: usize::MAX,
            buf: Vec::new(),
        })
    }

    /// Keeps at most `bytes` of decoded frames (the newest is kept whatever its size); older
    /// ones are dropped as new ones arrive.
    pub fn set_cache_cap(&mut self, bytes: usize) {
        self.cache_cap = bytes;
        self.shrink(0);
    }

    /// The bytes of decoded frames held for reuse.
    pub fn cached_bytes(&self) -> usize {
        self.cached
    }

    /// Drops every cached frame (the decoder's run goes on; a later request for a dropped
    /// frame decodes it again).
    pub fn drop_cache(&mut self) {
        self.cache.clear();
        self.cached = 0;
    }

    /// Drops the oldest frames until `incoming` more bytes fit under the cap and the count
    /// under [`CACHE`]; with nothing incoming the newest frame stays whatever its size.
    fn shrink(&mut self, incoming: usize) {
        let floor = usize::from(incoming == 0);
        while self.cache.len() > floor
            && (self.cache.len() >= CACHE || self.cached + incoming > self.cache_cap)
        {
            if let Some(old) = self.cache.pop_front() {
                self.cached -= frame_bytes(&old);
            }
        }
    }

    pub fn info(&self) -> &MediaInfo {
        self.demux.info()
    }

    /// The decoder's backend (`dav1d`, …).
    pub fn backend(&self) -> BackendId {
        self.decoder.backend()
    }

    pub fn table(&self) -> &SampleTable {
        &self.track.table
    }

    /// Presented frame `index` (an index into `table().frames()`, as `frame_at` gives it).
    pub fn frame(&mut self, index: usize) -> Result<&DecodedFrame, CoreError> {
        let f = *self.track.table.frames().get(index).ok_or_else(|| {
            CoreError::new("INVALID_RANGE", "no such frame")
                .with_details(json!({ "frame": index, "frames": self.track.table.len() }))
        })?;
        let d = f.decode_index as usize;
        // Frames come out with the composition time they were sent with.
        let want = self.track.sample(d).map(|s| s.cts).unwrap_or(f.pts);
        if let Some(k) = self.cache.iter().position(|c| c.pts == want) {
            return Ok(&self.cache[k]);
        }
        let start = decode_start(&self.track.table, index);
        // Continue only a run that began at or before `start` (it holds every reference the
        // frame needs) and has not passed the frame, or has sent it and the frame is still in
        // the decoder.
        let ahead = |n: usize| n <= d || self.in_flight.contains(&want);
        let resume =
            matches!(self.run, Some((began, n)) if began <= start && n >= start && ahead(n));
        if !resume {
            self.decoder.flush()?;
            self.drop_cache();
            self.in_flight.clear();
            self.ended = false;
            self.run = Some((start, start));
        }
        let total = self.track.sample_count();
        loop {
            while let Some(frame) = self.decoder.receive()? {
                if let Some(k) = self.in_flight.iter().position(|&p| p == frame.pts) {
                    self.in_flight.remove(k);
                }
                let hit = frame.pts == want;
                let bytes = frame_bytes(&frame);
                self.shrink(bytes);
                self.cached += bytes;
                self.cache.push_back(frame);
                if hit {
                    return Ok(self.cache.back().expect("just pushed"));
                }
            }
            let (began, n) = self.run.expect("a run is in progress");
            if n >= total && !self.ended {
                self.decoder.finish()?;
                self.ended = true;
                continue;
            }
            if n >= total {
                self.run = None;
                return Err(CoreError::new(
                    "DECODE_FAILED",
                    "the decoder did not produce the frame",
                )
                .with_details(json!({ "backend": self.decoder.backend().0, "frame": index })));
            }
            let s = self.track.sample(n).expect("n < total");
            self.demux
                .read_sample(self.track.track, n as u32, &mut self.buf)?;
            self.run = Some((began, n + 1));
            self.in_flight.push_back(s.cts);
            if let Err(e) = self.decoder.send(&self.buf, s.cts) {
                self.run = None;
                return Err(
                    e.with_details(json!({ "backend": self.decoder.backend().0, "frame": index }))
                );
            }
        }
    }
}
