//! AV1 decoding with dav1d, multi-threaded.
//!
//! dav1d (1.0 and later) runs tile, frame and post-filter work on one pool of `threads`
//! workers, with up to `frame_delay` frames in flight (frame threading). With more than one
//! frame in flight, two `dav1d_get_picture` calls in a row with no packet sent between them
//! *drain*: the second waits for the oldest frame in flight. `receive` therefore asks once
//! per packet sent, and drains only after `finish` (the end of the stream), so frames keep
//! decoding in parallel while packets keep coming.

use std::collections::VecDeque;

use fin_core::CoreError;
use serde_json::json;

use super::dav1d_ffi::{Context, Status};
use crate::frame::DecodedFrame;
use crate::traits::{BackendId, VideoDecoder};

pub(crate) const BACKEND: BackendId = BackendId("dav1d");

/// The largest picture decoded (16384 × 16384 pixels): a header asking for more is
/// `DECODE_FAILED` before dav1d allocates for it.
pub const MAX_PIXELS: u32 = 16384 * 16384;

/// How a decoder uses threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DecodeThreads {
    /// Worker threads for tiles, frames and post-filters; 0: one per logical processor.
    pub threads: u32,
    /// Frames decoded in parallel; 0: dav1d's choice (⌈√threads⌉, at most 8); 1: none, the
    /// lowest latency (a frame comes out as soon as its packet is in).
    pub frame_delay: u32,
}

impl DecodeThreads {
    /// One thread, one frame at a time.
    pub const SINGLE: DecodeThreads = DecodeThreads {
        threads: 1,
        frame_delay: 1,
    };
}

fn failed(code: i32, what: &str) -> CoreError {
    CoreError::new(
        "DECODE_FAILED",
        format!("dav1d could not {what} (error {code})"),
    )
    .with_details(json!({ "backend": BACKEND.0, "error": code }))
}

/// A dav1d decoder: packets waiting for room, then pictures in presentation order.
pub struct Dav1dDecoder {
    ctx: Context,
    waiting: VecDeque<(Vec<u8>, i64)>,
    /// The last call into dav1d asked for a picture: asking again would drain.
    polled: bool,
    /// `finish` was called: no more packets until `flush`.
    finished: bool,
    /// A waiting packet failed to go in right after a picture came out: the picture is
    /// returned first, the error on the next `receive`.
    failed: Option<CoreError>,
}

impl Dav1dDecoder {
    /// The linked dav1d's version (the pinned release, `1.5.4`).
    pub fn version() -> String {
        super::dav1d_ffi::version()
    }

    /// The frames in flight dav1d resolves for `threads` (at least 1).
    pub fn frame_delay(threads: DecodeThreads) -> u32 {
        let clamp = |n: u32| n.min(256) as i32;
        super::dav1d_ffi::frame_delay(clamp(threads.threads), clamp(threads.frame_delay)).max(1)
            as u32
    }

    pub fn new(threads: DecodeThreads) -> Result<Dav1dDecoder, CoreError> {
        // dav1d's limits: 256 threads, 256 frames in flight.
        let clamp = |n: u32| n.min(256) as i32;
        let ctx = Context::open(
            clamp(threads.threads),
            clamp(threads.frame_delay),
            MAX_PIXELS,
        )
        .map_err(|e| failed(e, "start"))?;
        Ok(Dav1dDecoder {
            ctx,
            waiting: VecDeque::new(),
            polled: false,
            finished: false,
            failed: None,
        })
    }

    /// Sends waiting packets until dav1d wants a picture taken first; whether any went in.
    fn pump(&mut self) -> Result<bool, CoreError> {
        let mut sent = false;
        while let Some((data, pts)) = self.waiting.front() {
            match self.ctx.send(data, *pts) {
                Status::Done(()) => {
                    self.waiting.pop_front();
                    self.polled = false;
                    sent = true;
                }
                Status::Again => {
                    // dav1d clears its drain flag on every send with data, taken or not
                    // (`dav1d_send_data`), so the next ask does not wait: it parses what dav1d
                    // still holds of the last packet, which is what makes room for this one.
                    self.polled = false;
                    break;
                }
                Status::Error(e) => {
                    self.waiting.pop_front();
                    self.polled = false;
                    return Err(failed(e, "decode a packet"));
                }
                Status::Refused(e) => {
                    self.waiting.pop_front();
                    return Err(e);
                }
            }
        }
        Ok(sent)
    }
}

impl VideoDecoder for Dav1dDecoder {
    fn send(&mut self, data: &[u8], pts: i64) -> Result<(), CoreError> {
        self.finished = false;
        self.waiting.push_back((data.to_vec(), pts));
        self.pump().map(|_| ())
    }

    fn receive(&mut self) -> Result<Option<DecodedFrame>, CoreError> {
        if let Some(e) = self.failed.take() {
            return Err(e);
        }
        loop {
            // Asking twice with nothing sent in between waits on the frames in flight: only
            // at the end of the stream.
            if self.polled && !self.pump()? && !self.finished {
                return Ok(None);
            }
            let asked = self.polled;
            self.polled = true;
            match self.ctx.receive() {
                Status::Done(f) => {
                    // The picture is out of dav1d already: a packet failing now must not
                    // lose it.
                    self.failed = self.pump().err();
                    return Ok(Some(f));
                }
                Status::Again => {
                    // At the end of the stream, a first ask that found nothing is followed
                    // by a second, which drains; otherwise send what is waiting and ask again.
                    let drain_next = self.finished && !asked;
                    if !drain_next && !self.pump()? {
                        return Ok(None);
                    }
                }
                Status::Error(e) => return Err(failed(e, "decode a picture")),
                Status::Refused(e) => return Err(e),
            }
        }
    }

    fn finish(&mut self) -> Result<(), CoreError> {
        self.finished = true;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), CoreError> {
        self.waiting.clear();
        self.ctx.flush();
        self.polled = false;
        self.finished = false;
        self.failed = None;
        Ok(())
    }

    fn backend(&self) -> BackendId {
        BACKEND
    }
}
