//! What the first and last bytes of a file say before any reader runs: the container family
//! (so a reader's failure on a known audio container is `MEDIA_CORRUPT`, and anything else is
//! `UNSUPPORTED_CONTAINER`), a WAVE file's channel layout per Decision 3, and whether an Ogg
//! stream ends on a complete end-of-stream page (a cut file is corrupt, not short).
//!
//! **The Ogg end-of-stream rule is a deliberate restriction.** RFC 3533 requires a logical
//! stream's last page to carry the EOS flag, and a file cut at a page boundary is otherwise
//! indistinguishable from a whole one; so a file that does not end on a complete EOS page (a
//! cut download, an interrupted recording, trailing bytes after the last page) is
//! `MEDIA_CORRUPT`, never decoded as far as it goes. The fix for such a file is to remux it.

use std::io::{Read, Seek, SeekFrom};

use fin_core::CoreError;
use fin_media::audio_layout::ChannelLayout;
use serde_json::json;

/// The container family of a file's first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// ISOBMFF (`ftyp` first): `fin-container`'s demuxer.
    Mp4,
    /// A container symphonia reads.
    Wave,
    Aiff,
    Caf,
    Flac,
    Ogg,
    Mp3,
    Unknown,
}

impl Kind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::Mp4 => "mp4",
            Kind::Wave => "wav",
            Kind::Aiff => "aiff",
            Kind::Caf => "caf",
            Kind::Flac => "flac",
            Kind::Ogg => "ogg",
            Kind::Mp3 => "mp3",
            Kind::Unknown => "unknown",
        }
    }

    /// Whether the container states its length in frames, so fewer decoded frames is a cut file.
    pub(crate) fn exact_length(self) -> bool {
        matches!(
            self,
            Kind::Wave | Kind::Aiff | Kind::Caf | Kind::Flac | Kind::Ogg
        )
    }
}

pub(crate) fn sniff(head: &[u8]) -> Kind {
    let at = |i: usize, m: &[u8]| head.get(i..i + m.len()) == Some(m);
    if at(4, b"ftyp") {
        Kind::Mp4
    } else if at(0, b"RIFF") || at(0, b"RF64") {
        Kind::Wave
    } else if at(0, b"FORM") {
        Kind::Aiff
    } else if at(0, b"caff") {
        Kind::Caf
    } else if at(0, b"fLaC") {
        Kind::Flac
    } else if at(0, b"OggS") {
        Kind::Ogg
    } else if at(0, b"ID3") || (head.len() >= 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0) {
        Kind::Mp3
    } else {
        Kind::Unknown
    }
}

/// Chunks walked before `fmt ` at most.
const MAX_CHUNKS: usize = 1024;

fn read_exact_at<R: Read + Seek>(r: &mut R, at: u64, buf: &mut [u8]) -> Result<bool, CoreError> {
    let io = |e: std::io::Error| fin_container::error::io(&e);
    r.seek(SeekFrom::Start(at)).map_err(io)?;
    let mut got = 0;
    while got < buf.len() {
        match r.read(&mut buf[got..]).map_err(io)? {
            0 => return Ok(false),
            n => got += n,
        }
    }
    Ok(true)
}

/// A WAVE file's layout from its own `fmt ` chunk (Decision 3): `WAVEFORMATEXTENSIBLE`'s
/// `dwChannelMask`, or the WAVE default for the count when there is no mask (or it is 0). The
/// chunk headers before it are walked with seeks (at most [`MAX_CHUNKS`]); a chunk that runs
/// past the end of the file before `fmt ` is `MEDIA_CORRUPT`. `None` when the file ends first.
pub(crate) fn wave_layout<R: Read + Seek>(
    r: &mut R,
    len: u64,
) -> Result<Option<ChannelLayout>, CoreError> {
    let mut at = 12u64;
    for _ in 0..MAX_CHUNKS {
        let mut h = [0u8; 8];
        if !read_exact_at(r, at, &mut h)? {
            return Ok(None);
        }
        let size = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as u64;
        if &h[..4] == b"fmt " {
            let mut body = vec![0u8; size.min(40) as usize];
            if size < 16 || !read_exact_at(r, at + 8, &mut body)? {
                return Ok(None);
            }
            let tag = u16::from_le_bytes([body[0], body[1]]);
            let channels = u16::from_le_bytes([body[2], body[3]]) as usize;
            let mask = if tag == 0xFFFE && body.len() >= 24 {
                u32::from_le_bytes([body[20], body[21], body[22], body[23]])
            } else {
                0
            };
            return Ok(Some(ChannelLayout::wave_mask(mask, channels)));
        }
        let next = at + 8 + size + size % 2;
        if next > len {
            return Err(CoreError::new(
                "MEDIA_CORRUPT",
                "a WAVE chunk before `fmt ` runs past the end of the file",
            )
            .with_details(
                json!({ "at": at, "format": "wav", "chunk": String::from_utf8_lossy(&h[..4]) }),
            ));
        }
        at = next;
    }
    Err(CoreError::new(
        "MEDIA_CORRUPT",
        "no `fmt ` chunk among the WAVE file's first chunks",
    )
    .with_details(json!({ "at": at, "format": "wav", "maxChunks": MAX_CHUNKS })))
}

/// Whether an MP3 states its length: its first frame (after an ID3v2 tag) is a Xing/Info frame
/// with a frame count, or a VBRI frame; otherwise symphonia estimates it from the bitrate.
pub(crate) fn mp3_exact<R: Read + Seek>(r: &mut R) -> Result<bool, CoreError> {
    let mut id3 = [0u8; 10];
    let mut at = 0u64;
    if read_exact_at(r, 0, &mut id3)? && &id3[..3] == b"ID3" {
        let size = id3[6..10]
            .iter()
            .fold(0u64, |a, &b| (a << 7) | (b & 0x7F) as u64);
        at = 10 + size + if id3[5] & 0x10 != 0 { 10 } else { 0 };
    }
    let mut f = [0u8; 64];
    if !read_exact_at(r, at, &mut f)? || f[0] != 0xFF || f[1] & 0xE0 != 0xE0 {
        return Ok(false);
    }
    let mpeg1 = f[1] & 0x18 == 0x18;
    let mono = f[3] >> 6 == 3;
    let side = match (mpeg1, mono) {
        (true, true) => 17,
        (true, false) => 32,
        (false, true) => 9,
        (false, false) => 17,
    };
    let x = 4 + side;
    let xing = (&f[x..x + 4] == b"Xing" || &f[x..x + 4] == b"Info") && f[x + 7] & 1 != 0;
    Ok(xing || &f[36..40] == b"VBRI")
}

/// Whether `tail` (the file's last bytes) ends with a complete Ogg page carrying the
/// end-of-stream flag: `Err` names what is wrong.
pub(crate) fn ogg_end(tail: &[u8]) -> Result<(), &'static str> {
    for i in (0..tail.len().saturating_sub(3)).rev() {
        if &tail[i..i + 4] != b"OggS" {
            continue;
        }
        let Some(head) = tail.get(i..i + 27) else {
            continue;
        };
        let n = head[26] as usize;
        let Some(lacing) = tail.get(i + 27..i + 27 + n) else {
            continue;
        };
        let body: usize = lacing.iter().map(|&l| l as usize).sum();
        if i + 27 + n + body != tail.len() {
            continue;
        }
        return if head[5] & 4 != 0 {
            Ok(())
        } else {
            Err("the last page is not end-of-stream")
        };
    }
    Err("the file does not end on a complete page")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(sniff(b"\0\0\0\x18ftypisom"), Kind::Mp4);
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVE"), Kind::Wave);
        assert_eq!(sniff(b"ID3\x04"), Kind::Mp3);
        assert_eq!(sniff(&[0xFF, 0xFB, 0x90]), Kind::Mp3);
        assert_eq!(sniff(b"hello"), Kind::Unknown);
        assert_eq!(sniff(b""), Kind::Unknown);
    }
}
