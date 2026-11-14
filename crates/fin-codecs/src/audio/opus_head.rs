//! The Opus stream header (6C plan Decision 3): read from an MP4 `dOps` body (ISO/IEC 23003-5
//! §4.3.2, big-endian) or an Ogg `OpusHead` packet (RFC 7845 §5.1, little-endian), written as
//! `dOps`, and checked as RFC 7845 §5.1.1 asks. Mapping family 0 is mono or stereo in one
//! stream; family 1 is 1 to 8 channels in Vorbis order; other families (ambisonics, 255) are
//! not decoded here (`CODEC_UNAVAILABLE`).

use fin_core::CoreError;
use fin_media::audio_layout::{ChannelLayout, ChannelPos};
use serde_json::json;

pub(crate) const BACKEND: &str = "libopus";

/// What a decoder needs from the header, and what an encoder's `dOps` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpusHeader {
    pub channels: u8,
    /// Samples at 48 kHz to drop from the start.
    pub pre_skip: u16,
    /// Output gain, Q7.8 dB.
    pub gain: i16,
    pub family: u8,
    pub streams: u8,
    pub coupled: u8,
    /// The decoded channel feeding each output channel (255: silence).
    pub mapping: Vec<u8>,
}

fn corrupt(why: &str) -> CoreError {
    CoreError::new(
        "MEDIA_CORRUPT",
        format!("the Opus stream header is malformed: {why}"),
    )
    .with_details(json!({ "at": "header", "backend": BACKEND, "reason": why }))
}

fn unavailable(message: String, details: serde_json::Value) -> CoreError {
    CoreError::new("CODEC_UNAVAILABLE", message)
        .with_details(details)
        .with_fix("convert the audio to mono, stereo or a standard 5.1 or 7.1 layout")
}

impl OpusHeader {
    /// From the body of an MP4 `dOps` box.
    pub fn from_dops(b: &[u8]) -> Result<Self, CoreError> {
        if b.len() < 11 {
            return Err(corrupt("dOps is shorter than 11 bytes"));
        }
        if b[0] != 0 {
            return Err(corrupt("dOps version is not 0"));
        }
        Self::checked(
            b[1],
            u16::from_be_bytes([b[2], b[3]]),
            i16::from_be_bytes([b[8], b[9]]),
            b[10],
            &b[11..],
        )
    }

    /// From an Ogg `OpusHead` identification packet.
    pub fn from_opus_head(b: &[u8]) -> Result<Self, CoreError> {
        if b.len() < 19 || &b[..8] != b"OpusHead" {
            return Err(corrupt("OpusHead is missing or shorter than 19 bytes"));
        }
        // Versions 0 to 15 share this layout (RFC 7845 §5.1: the major version is the top
        // four bits).
        if b[8] >> 4 != 0 {
            return Err(corrupt("OpusHead's major version is not 0"));
        }
        Self::checked(
            b[9],
            u16::from_le_bytes([b[10], b[11]]),
            i16::from_le_bytes([b[16], b[17]]),
            b[18],
            &b[19..],
        )
    }

    /// The fields and the mapping table after the family byte, checked.
    fn checked(
        channels: u8,
        pre_skip: u16,
        gain: i16,
        family: u8,
        table: &[u8],
    ) -> Result<Self, CoreError> {
        if channels == 0 {
            return Err(corrupt("no output channels"));
        }
        let (streams, coupled, mapping) = match family {
            0 => {
                if channels > 2 {
                    return Err(corrupt("mapping family 0 has more than two channels"));
                }
                (1, channels - 1, (0..channels).collect())
            }
            1 => {
                if channels > 8 {
                    return Err(unavailable(
                        format!("Opus with {channels} channels is not decoded here"),
                        json!({ "backend": BACKEND, "reason": "channels", "channels": channels }),
                    ));
                }
                let need = 2 + channels as usize;
                if table.len() < need {
                    return Err(corrupt("the channel mapping table is cut"));
                }
                let (streams, coupled) = (table[0], table[1]);
                let mapping = table[2..need].to_vec();
                let decoded = streams as u16 + coupled as u16;
                if streams == 0 || coupled > streams || decoded > 255 {
                    return Err(corrupt("impossible stream counts"));
                }
                if mapping.iter().any(|&m| m != 255 && m as u16 >= decoded) {
                    return Err(corrupt("the mapping names a channel past the decoded ones"));
                }
                (streams, coupled, mapping)
            }
            f => {
                return Err(unavailable(
                    format!("Opus channel mapping family {f} is not decoded here"),
                    json!({ "backend": BACKEND, "reason": "channelMapping", "family": f }),
                ));
            }
        };
        Ok(OpusHeader {
            channels,
            pre_skip,
            gain,
            family,
            streams,
            coupled,
            mapping,
        })
    }

    /// The body of a `dOps` box (input rate 48 kHz).
    pub fn to_dops(&self) -> Vec<u8> {
        let mut b = vec![0, self.channels];
        b.extend_from_slice(&self.pre_skip.to_be_bytes());
        b.extend_from_slice(&48_000u32.to_be_bytes());
        b.extend_from_slice(&self.gain.to_be_bytes());
        b.push(self.family);
        if self.family != 0 {
            b.push(self.streams);
            b.push(self.coupled);
            b.extend_from_slice(&self.mapping);
        }
        b
    }

    /// The decoder's channel order: family 0 is centre (mono) or left, right; family 1 is
    /// Vorbis order (Decision 3).
    pub fn layout(&self) -> ChannelLayout {
        match (self.family, self.channels) {
            (0, 1) => ChannelLayout(vec![ChannelPos::C]),
            (0, _) => ChannelLayout(vec![ChannelPos::L, ChannelPos::R]),
            (_, n) => ChannelLayout::vorbis_order(n as usize),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dops_and_opus_head_read_the_same_header() {
        let h = OpusHeader {
            channels: 6,
            pre_skip: 312,
            gain: -256,
            family: 1,
            streams: 4,
            coupled: 2,
            mapping: vec![0, 4, 1, 2, 3, 5],
        };
        let dops = h.to_dops();
        assert_eq!(OpusHeader::from_dops(&dops).unwrap(), h);
        let mut head = b"OpusHead\x01\x06".to_vec();
        head.extend_from_slice(&312u16.to_le_bytes());
        head.extend_from_slice(&48_000u32.to_le_bytes());
        head.extend_from_slice(&(-256i16).to_le_bytes());
        head.extend_from_slice(&dops[10..]);
        assert_eq!(OpusHeader::from_opus_head(&head).unwrap(), h);
        assert_eq!(h.layout(), ChannelLayout::vorbis_order(6));
    }

    #[test]
    fn family_zero_is_one_stream() {
        let h = OpusHeader::from_dops(&[0, 2, 1, 56, 0, 0, 187, 128, 0, 0, 0]).unwrap();
        assert_eq!(
            (h.streams, h.coupled, h.mapping.as_slice()),
            (1, 1, &[0, 1][..])
        );
        let e = OpusHeader::from_dops(&[0, 3, 1, 56, 0, 0, 187, 128, 0, 0, 0]).unwrap_err();
        assert_eq!(e.code, "MEDIA_CORRUPT");
    }
}
