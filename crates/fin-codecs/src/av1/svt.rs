//! AV1 encoding with SVT-AV1.

use std::collections::VecDeque;

use fin_container::CodecConfig;
use fin_core::{CoreError, Rational};
use fin_media::{ColourTags, PixelFormat, Size};
use serde_json::json;

use super::dav1d_ffi::parse_sequence_header;
use super::svt_ffi::{Encoder, FLAG_EOS, KEY_PICTURE};
use crate::frame::{EncodedPacket, RawFrame};
use crate::traits::VideoEncoder;

/// What an AV1 encode is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Av1EncoderConfig {
    pub size: Size,
    /// Frames per second (VFR sources pass their nominal rate).
    pub rate: Rational,
    /// 0 (slowest) … 13 (fastest).
    pub preset: u8,
    /// 1 … 63.
    pub crf: u8,
    /// Frames between keyframes.
    pub keyint: u32,
    /// SVT-AV1's level of parallelism; 1 makes the output depend only on the input.
    pub threads: u32,
    pub colour: ColourTags,
}

fn failed(code: i32, what: &str) -> CoreError {
    CoreError::new(
        "ENCODE_FAILED",
        format!("SVT-AV1 could not {what} (error {code:#x})"),
    )
    .with_details(json!({ "backend": "svt-av1", "error": code }))
}

struct SvtEncoder {
    enc: Encoder,
    size: Size,
    av1c: Vec<u8>,
    out: VecDeque<EncodedPacket>,
    finished: bool,
    ended: bool,
}

/// The `av1C` record for a sequence header OBU (ISO/IEC AV1 in ISOBMFF §2.3).
fn av1c(seq: &[u8]) -> Result<Vec<u8>, CoreError> {
    let s = parse_sequence_header(seq).ok_or_else(|| failed(-1, "produce a sequence header"))?;
    let b2 = ((s.tier & 1) << 7)
        | ((s.high_bitdepth as u8) << 6)
        | ((s.twelve_bit as u8) << 5)
        | ((s.monochrome as u8) << 4)
        | ((s.subsampling.0 as u8) << 3)
        | ((s.subsampling.1 as u8) << 2)
        | (s.chroma_position & 3);
    let mut out = vec![0x81, (s.profile << 5) | (s.level & 0x1f), b2, 0];
    out.extend_from_slice(seq);
    Ok(out)
}

/// An SVT-AV1 encoder for 8-bit 4:2:0 input.
pub fn open_av1_encoder(cfg: &Av1EncoderConfig) -> Result<Box<dyn VideoEncoder>, CoreError> {
    if !cfg.size.width.is_multiple_of(2)
        || !cfg.size.height.is_multiple_of(2)
        || cfg.size.width < 64
        || cfg.size.height < 64
    {
        return Err(CoreError::new(
            "ENCODE_FAILED",
            "AV1 encoding needs an even size of at least 64×64",
        )
        .with_details(json!({ "backend": "svt-av1", "size": [cfg.size.width, cfg.size.height] })));
    }
    let mut enc = Encoder::new().map_err(|e| failed(e, "start"))?;
    let (p, t, m, full) = cfg.colour.to_cicp();
    let params = [
        ("width", cfg.size.width.to_string()),
        ("height", cfg.size.height.to_string()),
        ("fps-num", cfg.rate.num().to_string()),
        ("fps-denom", cfg.rate.den().to_string()),
        ("input-depth", "8".into()),
        ("preset", cfg.preset.to_string()),
        ("crf", cfg.crf.to_string()),
        ("keyint", cfg.keyint.max(1).to_string()),
        ("lp", cfg.threads.max(1).to_string()),
        ("color-primaries", p.to_string()),
        ("transfer-characteristics", t.to_string()),
        ("matrix-coefficients", m.to_string()),
        ("color-range", (full as u8).to_string()),
    ];
    for (k, v) in params {
        enc.param(k, &v)
            .map_err(|e| failed(e, &format!("set {k} = {v}")))?;
    }
    enc.init().map_err(|e| failed(e, "initialise"))?;
    let seq = enc
        .header()
        .map_err(|e| failed(e, "write its stream header"))?;
    let av1c = av1c(&seq)?;
    Ok(Box::new(SvtEncoder {
        enc,
        size: cfg.size,
        av1c,
        out: VecDeque::new(),
        finished: false,
        ended: false,
    }))
}

impl SvtEncoder {
    fn collect(&mut self, wait: bool) -> Result<(), CoreError> {
        while !self.ended {
            match self
                .enc
                .packet(wait)
                .map_err(|e| failed(e, "produce a packet"))?
            {
                Some(p) => {
                    self.ended = p.flags & FLAG_EOS != 0;
                    if !p.data.is_empty() {
                        self.out.push_back(EncodedPacket {
                            data: p.data,
                            pts: p.pts,
                            dts: p.dts,
                            keyframe: p.pic_type == KEY_PICTURE,
                        });
                    }
                }
                None => break,
            }
            if !wait {
                break;
            }
        }
        Ok(())
    }
}

impl VideoEncoder for SvtEncoder {
    fn send(&mut self, frame: &RawFrame, pts: i64, force_key: bool) -> Result<(), CoreError> {
        if frame.format != PixelFormat::I420 || frame.size != self.size {
            return Err(CoreError::new(
                "ENCODE_FAILED",
                "the encoder takes 8-bit 4:2:0 at its configured size",
            )
            .with_details(json!({ "backend": "svt-av1" })));
        }
        let [y, u, v] = &frame.planes;
        if y.stride != self.size.width as usize || u.stride != self.size.width.div_ceil(2) as usize
        {
            return Err(
                CoreError::new("ENCODE_FAILED", "the encoder takes tight planes")
                    .with_details(json!({ "backend": "svt-av1" })),
            );
        }
        self.enc
            .send([&y.data, &u.data, &v.data], self.size, pts, force_key)
            .map_err(|e| failed(e, "take a picture"))?;
        self.collect(false)
    }

    fn receive(&mut self) -> Result<Option<EncodedPacket>, CoreError> {
        if self.out.is_empty() {
            self.collect(self.finished)?;
        }
        Ok(self.out.pop_front())
    }

    fn finish(&mut self) -> Result<(), CoreError> {
        if !self.finished {
            self.enc
                .send_eos()
                .map_err(|e| failed(e, "end the stream"))?;
            self.finished = true;
        }
        Ok(())
    }

    fn config(&self) -> CodecConfig {
        CodecConfig::Av1 {
            av1c: self.av1c.clone(),
        }
    }
}
