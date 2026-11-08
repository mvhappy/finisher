//! WAV: a RIFF/`WAVE` parser for `fmt ` and `data`. The duration is the data's whole frames
//! over the sample rate; other chunks are skipped. A `data` length of `0xFFFFFFFF` is a
//! streamed (unfinalised) WAV: its data runs to the end of the file.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use fin_core::{AssetKind, AudioInfo, CoreError, Rational};

use crate::{Probe, io_error, unsupported};

/// `fmt ` format tags whose frames are `block_align` bytes: PCM, IEEE float, A-law, µ-law and
/// WAVE_FORMAT_EXTENSIBLE (whose sub-format must be PCM or float).
const PCM: u16 = 1;
const FLOAT: u16 = 3;
const ALAW: u16 = 6;
const MULAW: u16 = 7;
const EXTENSIBLE: u16 = 0xFFFE;

struct Fmt {
    channels: u16,
    sample_rate: u32,
    block_align: u16,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn fmt(body: &[u8], path: &Path) -> Result<Fmt, CoreError> {
    let bad = |why: &str| unsupported(path, why);
    if body.len() < 16 {
        return Err(bad("WAV fmt chunk is too short"));
    }
    let mut tag = u16_at(body, 0);
    if tag == EXTENSIBLE {
        // cbSize(2) validBits(2) channelMask(4), then the sub-format GUID: its first 2 bytes.
        if body.len() < 40 {
            return Err(bad("WAV extensible fmt chunk is too short"));
        }
        tag = u16_at(body, 24);
    }
    if !matches!(tag, PCM | FLOAT | ALAW | MULAW) {
        return Err(unsupported(
            path,
            &format!("WAV format 0x{tag:04x} is not PCM, float, A-law or µ-law"),
        ));
    }
    let f = Fmt {
        channels: u16_at(body, 2),
        sample_rate: u32_at(body, 4),
        block_align: u16_at(body, 12),
    };
    if f.channels == 0 || f.sample_rate == 0 || f.block_align == 0 {
        return Err(bad("WAV fmt has no channels, sample rate or frame size"));
    }
    Ok(f)
}

pub(crate) fn probe<R: Read + Seek>(
    file: &mut R,
    size: u64,
    path: &Path,
) -> Result<Probe, CoreError> {
    let io = |e: std::io::Error| match e.kind() {
        std::io::ErrorKind::UnexpectedEof => unsupported(path, "truncated WAV"),
        _ => io_error(path, &e),
    };
    let mut at: u64 = 12; // after `RIFF`, the RIFF size and `WAVE`
    let mut format: Option<Fmt> = None;
    let mut data: Option<u64> = None;
    while format.is_none() || data.is_none() {
        if at + 8 > size {
            return Err(unsupported(path, "WAV has no fmt or data chunk"));
        }
        let mut header = [0u8; 8];
        file.seek(SeekFrom::Start(at)).map_err(io)?;
        file.read_exact(&mut header).map_err(io)?;
        let mut len = u64::from(u32_at(&header, 4));
        let body_at = at + 8;
        if &header[..4] == b"data" && len == u64::from(u32::MAX) {
            len = size - body_at;
        }
        if body_at + len > size {
            return Err(unsupported(path, "truncated WAV chunk"));
        }
        match &header[..4] {
            b"fmt " if format.is_none() => {
                let mut body = vec![0u8; len.min(64) as usize];
                file.read_exact(&mut body).map_err(io)?;
                format = Some(fmt(&body, path)?);
            }
            b"data" if data.is_none() => data = Some(len),
            _ => {}
        }
        at = body_at + len + (len & 1);
    }
    let (Some(f), Some(bytes)) = (format, data) else {
        unreachable!("the loop ends once both are found")
    };
    let frames = bytes / u64::from(f.block_align);
    let duration = i64::try_from(frames)
        .ok()
        .filter(|n| *n > 0)
        .and_then(|n| Rational::new(n, i64::from(f.sample_rate)).ok())
        .ok_or_else(|| unsupported(path, "WAV has no audio frames"))?;
    Ok(Probe {
        duration: Some(duration),
        audio: Some(AudioInfo {
            sample_rate: f.sample_rate,
            channels: f.channels,
        }),
        ..Probe::of(AssetKind::Media)
    })
}
