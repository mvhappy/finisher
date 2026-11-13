//! Proxy frames to a CFR MP4 through one encoder session (plan 6B-1 Task 14; Decisions 5, 7, 8,
//! 20): the session comes from `fin_codecs::open_encoder` (the one place an encoder is opened,
//! under `encoder_allowed`) for `EncodePurpose::Proxy` only, the one purpose 6B-1 encodes
//! (exports are 6E's, with their own pipeline), frame `i` is sent with pts `i` and a forced
//! keyframe every `gop` frames, and each packet is muxed as it comes out at `i × tick` in a track
//! of `timescale` (fast start: the `moov` goes in the reserve after `ftyp`). Packets must come
//! out one per frame in order (no B-frames, no reordering): anything else is `ENCODE_FAILED`.
//!
//! When the session moves to another encoder (`ENCODER_HUNG`'s fallback, Decision 20), the
//! output is made again from frame 0 with the new encoder's configuration record.

use std::fs::File;

use fin_codecs::{EncoderConfig, EncoderSession, Fed, Output, ProbeResult, RawFrame, open_encoder};
use fin_container::{Mp4Options, Mp4Writer, TrackRef, VideoTrackConfig};
use fin_core::{CoreError, Rational};
use fin_media::backend::{BackendEntry, EncodePurpose};
use fin_media::jobs::ProxyFallback;
use fin_media::{ColourTags, Rotation};
use serde_json::json;

/// Where and how the frames are muxed.
pub struct Track {
    pub timescale: u32,
    /// Ticks per frame.
    pub tick: u32,
    /// The encoded stream's colour tags (the `colr` box).
    pub colour: ColourTags,
}

/// `(timescale, ticks per frame)` for a CFR track at `rate`: `rate.num × 1000` ticks a second
/// and `rate.den × 1000` a frame when they fit, else the rate's own terms.
pub fn timing(rate: Rational) -> Result<(u32, u32), CoreError> {
    let (num, den) = (rate.num(), rate.den());
    if num <= 0 || den <= 0 {
        return Err(
            CoreError::new("MEDIA_CORRUPT", "the source has no frame rate")
                .with_details(json!({ "at": "nominalRate" })),
        );
    }
    let fit = |v: i64| u32::try_from(v).ok().filter(|v| *v > 0);
    match (fit(num * 1000), fit(den * 1000)) {
        (Some(t), Some(d)) => Ok((t, d)),
        _ => match (fit(num), fit(den)) {
            (Some(t), Some(d)) => Ok((t, d)),
            _ => Err(CoreError::new(
                "MEDIA_LIMIT",
                "the source's frame rate does not fit a track",
            )
            .with_details(json!({ "limit": "rate" }))),
        },
    }
}

/// What an encode produced.
#[derive(Debug)]
pub struct Encoded {
    /// The encoder that made the output.
    pub entry: &'static BackendEntry,
    pub fell_back: Vec<ProxyFallback>,
    pub frames: u64,
}

/// The caller's side of an encode: the pictures, a check between frames (progress, cancel) and
/// the output file (made new for every start, so a restart truncates).
pub trait Frames {
    fn frame(&mut self, i: u64) -> Result<RawFrame, CoreError>;
    fn between(&mut self, done: u64) -> Result<(), CoreError>;
    fn output(&mut self) -> Result<File, CoreError>;
}

fn encode_failed(message: impl Into<String>, entry: &BackendEntry) -> CoreError {
    CoreError::new("ENCODE_FAILED", message).with_details(json!({ "backend": entry.name }))
}

/// The bytes the `moov` of `frames` samples may need, reserved after `ftyp`.
fn moov_reserve(frames: u64) -> u64 {
    (64 << 10) + frames.saturating_mul(24)
}

/// Writes one packet; `next` is the frame index the packet must carry.
fn write(
    w: &mut Mp4Writer<File>,
    track: TrackRef,
    t: &Track,
    p: &fin_codecs::EncodedPacket,
    next: &mut u64,
    entry: &BackendEntry,
) -> Result<(), CoreError> {
    if p.pts < 0 || p.pts as u64 != *next {
        return Err(encode_failed(
            format!(
                "the encoder gave frame {} where frame {next} was due (reordered or dropped)",
                p.pts
            ),
            entry,
        ));
    }
    let at = (*next as i64)
        .checked_mul(t.tick as i64)
        .ok_or_else(|| encode_failed("the track's time overflows", entry))?;
    w.write(track, &p.data, at, at, t.tick, p.keyframe)?;
    *next += 1;
    Ok(())
}

fn start(
    session: &mut EncoderSession,
    t: &Track,
    cfg: &EncoderConfig,
    frames: u64,
    src: &mut dyn Frames,
) -> Result<(Mp4Writer<File>, TrackRef), CoreError> {
    let config = session.config()?;
    let opts = Mp4Options {
        moov_reserve: moov_reserve(frames),
        ..Mp4Options::default()
    };
    let mut w = Mp4Writer::new(src.output()?, opts)?;
    let track = w.add_video(VideoTrackConfig {
        config,
        coded: cfg.size,
        timescale: t.timescale,
        rotation: Rotation::R0,
        pixel_aspect: Rational::from_int(1),
        colour: t.colour,
        hdr: None,
    })?;
    Ok((w, track))
}

/// Encodes `frames` frames from `src` with `cfg` (keyframes forced every `cfg.gop.interval()`)
/// into `src.output()`. `cfg.purpose` must be `Proxy` (`INTERNAL` otherwise).
pub fn encode(
    caps: &ProbeResult,
    cfg: &EncoderConfig,
    t: &Track,
    frames: u64,
    src: &mut dyn Frames,
) -> Result<Encoded, CoreError> {
    if cfg.purpose != EncodePurpose::Proxy {
        return Err(CoreError::new(
            "INTERNAL",
            "the proxy pipeline encodes for EncodePurpose::Proxy only",
        ));
    }
    let mut session = open_encoder(cfg, caps)?;
    let every = u64::from(cfg.gop.interval());
    'again: loop {
        let (mut w, track) = start(&mut session, t, cfg, frames, src)?;
        let entry = session.entry();
        let mut next = 0u64;
        for i in 0..frames {
            src.between(i)?;
            let frame = src.frame(i)?;
            if session.send(frame, i as i64, i % every == 0)? == Fed::Restart {
                continue 'again;
            }
            loop {
                match session.receive()? {
                    Output::Packet(p) => write(&mut w, track, t, &p, &mut next, entry)?,
                    Output::Pending | Output::End => break,
                    Output::Restart => continue 'again,
                }
            }
        }
        src.between(frames)?;
        if session.finish()? == Fed::Restart {
            continue 'again;
        }
        loop {
            match session.receive()? {
                Output::Packet(p) => write(&mut w, track, t, &p, &mut next, entry)?,
                Output::End => break,
                Output::Pending => {}
                Output::Restart => continue 'again,
            }
        }
        if next != frames {
            return Err(encode_failed(
                format!("{next} packets for {frames} frames"),
                entry,
            ));
        }
        drop(w.finish()?);
        let fell_back = session
            .fell_back()
            .iter()
            .map(|n| ProxyFallback {
                from: n.from.to_string(),
                to: n.to.to_string(),
                reason: match n.reason {
                    fin_codecs::FallbackReason::OpenFailed => "openFailed",
                    fin_codecs::FallbackReason::Hung => "hung",
                }
                .to_string(),
            })
            .collect();
        return Ok(Encoded {
            entry,
            fell_back,
            frames,
        });
    }
}

#[cfg(test)]
#[path = "encode_pipeline_tests.rs"]
mod tests;
