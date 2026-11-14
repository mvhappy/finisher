//! What the first and last bytes of a file say before any reader runs: the container family
//! (so a reader's failure on a known audio container is `MEDIA_CORRUPT`, and anything else is
//! `UNSUPPORTED_CONTAINER`), a WAVE file's channel layout per Decision 3, and whether an Ogg
//! stream ends on a complete end-of-stream page (a cut file is corrupt, not short).

use fin_media::audio_layout::ChannelLayout;

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

/// A WAVE file's layout from its own `fmt ` chunk (Decision 3): `WAVEFORMATEXTENSIBLE`'s
/// `dwChannelMask`, or the WAVE default for the count when there is no mask (or it is 0).
/// `None` when the chunk is not in `head`.
pub(crate) fn wave_layout(head: &[u8]) -> Option<ChannelLayout> {
    let mut at = 12usize;
    while at + 8 <= head.len() {
        let id = &head[at..at + 4];
        let size = u32::from_le_bytes(head[at + 4..at + 8].try_into().ok()?) as usize;
        let body = head.get(at + 8..(at + 8).checked_add(size)?)?;
        if id == b"fmt " {
            let u16_at = |i: usize| Some(u16::from_le_bytes(body.get(i..i + 2)?.try_into().ok()?));
            let tag = u16_at(0)?;
            let channels = u16_at(2)? as usize;
            let mask = if tag == 0xFFFE && body.len() >= 24 {
                u32::from_le_bytes(body[20..24].try_into().ok()?)
            } else {
                0
            };
            return Some(ChannelLayout::wave_mask(mask, channels));
        }
        at = at.checked_add(8 + size + size % 2)?;
    }
    None
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
