//! AV1 decoding with `rav1d`.

use std::collections::VecDeque;

use fin_core::CoreError;
use serde_json::json;

use super::rav1d_ffi::{Context, Status};
use crate::frame::DecodedFrame;
use crate::traits::{BackendId, VideoDecoder};

pub(crate) const BACKEND: BackendId = BackendId("rav1d");

fn failed(code: i32, what: &str) -> CoreError {
    CoreError::new(
        "DECODE_FAILED",
        format!("rav1d could not {what} (error {code})"),
    )
    .with_details(json!({ "backend": BACKEND.0, "error": code }))
}

/// A `rav1d` decoder: packets waiting for room, then pictures in presentation order.
pub struct Rav1dDecoder {
    ctx: Context,
    waiting: VecDeque<(Vec<u8>, i64)>,
}

impl Rav1dDecoder {
    pub fn new() -> Result<Rav1dDecoder, CoreError> {
        let ctx = Context::open(0).map_err(|e| failed(e, "start"))?;
        Ok(Rav1dDecoder {
            ctx,
            waiting: VecDeque::new(),
        })
    }

    /// Sends waiting packets until the decoder wants a picture taken first.
    fn pump(&mut self) -> Result<(), CoreError> {
        while let Some((data, pts)) = self.waiting.front() {
            match self.ctx.send(data, *pts) {
                Status::Done(()) => {
                    self.waiting.pop_front();
                }
                Status::Again => break,
                Status::Error(e) => {
                    self.waiting.pop_front();
                    return Err(failed(e, "decode a packet"));
                }
            }
        }
        Ok(())
    }
}

impl VideoDecoder for Rav1dDecoder {
    fn send(&mut self, data: &[u8], pts: i64) -> Result<(), CoreError> {
        self.waiting.push_back((data.to_vec(), pts));
        self.pump()
    }

    fn receive(&mut self) -> Result<Option<DecodedFrame>, CoreError> {
        loop {
            match self.ctx.receive() {
                Status::Done(f) => {
                    self.pump()?;
                    return Ok(Some(f));
                }
                Status::Again if !self.waiting.is_empty() => {
                    let before = self.waiting.len();
                    self.pump()?;
                    if self.waiting.len() == before {
                        return Ok(None);
                    }
                }
                Status::Again => return Ok(None),
                Status::Error(e) => return Err(failed(e, "decode a picture")),
            }
        }
    }

    fn flush(&mut self) -> Result<(), CoreError> {
        self.waiting.clear();
        self.ctx.flush();
        Ok(())
    }

    fn backend(&self) -> BackendId {
        BACKEND
    }
}
