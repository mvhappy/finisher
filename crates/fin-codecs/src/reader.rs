//! Random access by presentation index: `FrameReader` decodes from the sync sample
//! `decode_start` names, keeps the frames it decoded since (up to a cap), and continues
//! forward when the next request is ahead in the same run.

use std::collections::VecDeque;
use std::io::{Read, Seek};

use fin_container::{Demuxer, Limits, VideoTrack};
use fin_core::CoreError;
use fin_media::{MediaInfo, SampleTable, decode_start};
use serde_json::json;

use crate::caps::{Capabilities, open_decoder};
use crate::frame::DecodedFrame;
use crate::traits::VideoDecoder;

/// Decoded frames kept for reuse.
const CACHE: usize = 64;

/// A demuxer, a decoder and the track's tables.
pub struct FrameReader<R: Read + Seek> {
    demux: Demuxer<R>,
    track: VideoTrack,
    decoder: Box<dyn VideoDecoder>,
    /// The run in progress: the sample it began at (a sync sample) and the next one to send,
    /// both in decode order.
    run: Option<(usize, usize)>,
    cache: VecDeque<DecodedFrame>,
    buf: Vec<u8>,
}

impl<R: Read + Seek> FrameReader<R> {
    /// Opens the file's first video track with a decoder from `caps`.
    pub fn open(r: R, limits: &Limits, caps: &Capabilities) -> Result<Self, CoreError> {
        let demux = Demuxer::open(r, limits)?;
        let track = demux.video().cloned().ok_or_else(|| {
            CoreError::new("NOT_FOUND", "the file has no video track")
                .with_details(json!({ "kind": "videoTrack" }))
        })?;
        let decoder = open_decoder(&track, caps)?;
        Ok(FrameReader {
            demux,
            track,
            decoder,
            run: None,
            cache: VecDeque::new(),
            buf: Vec::new(),
        })
    }

    pub fn info(&self) -> &MediaInfo {
        self.demux.info()
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
        // Continue only a run that began at or before `start` and has not passed the frame:
        // it holds every reference the frame needs.
        let resume = matches!(self.run, Some((began, n)) if began <= start && n >= start && n <= d);
        if !resume {
            self.decoder.flush()?;
            self.cache.clear();
            self.run = Some((start, start));
        }
        let total = self.track.sample_count();
        loop {
            while let Some(frame) = self.decoder.receive()? {
                let hit = frame.pts == want;
                if self.cache.len() == CACHE {
                    self.cache.pop_front();
                }
                self.cache.push_back(frame);
                if hit {
                    return Ok(self.cache.back().expect("just pushed"));
                }
            }
            let (began, n) = self.run.expect("a run is in progress");
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
            if let Err(e) = self.decoder.send(&self.buf, s.cts) {
                self.run = None;
                return Err(
                    e.with_details(json!({ "backend": self.decoder.backend().0, "frame": index }))
                );
            }
        }
    }
}
