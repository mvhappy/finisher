//! The symphonia backend (MPL-2.0, unmodified; 6C plan Decisions 3, 8 and 24): PCM, FLAC, ALAC,
//! MP3 and Vorbis decode at the source's rate, with the layout read from symphonia's channel
//! set in the buffer's own order. Every call into symphonia runs inside [`guarded`], so a panic
//! in a decoder or format reader is `DECODE_FAILED {backend: symphonia}`, never an abort.

use std::panic::{AssertUnwindSafe, catch_unwind};

use fin_core::CoreError;
use fin_media::AudioCodec;
use fin_media::audio_layout::{ChannelLayout, ChannelPos};
use serde_json::json;
use symphonia::core::audio::{ChannelLabel, Channels, Position};
use symphonia::core::codecs::audio::well_known as ids;
use symphonia::core::codecs::audio::{
    AudioCodecId, AudioCodecParameters, AudioDecoder as SymDecoder, AudioDecoderOptions,
};
use symphonia::core::errors::Error as SymError;
use symphonia::core::packet::{Packet, PacketRef};
use symphonia::core::units::{Duration, Timestamp};

use super::traits::{AudioDecoder, DecodedAudio};
use crate::traits::BackendId;

pub(crate) const BACKEND: BackendId = BackendId("symphonia");

/// Runs `f`; a panic inside is `DECODE_FAILED {backend: symphonia, during}`.
pub(crate) fn guarded<T>(
    during: &str,
    f: impl FnOnce() -> Result<T, CoreError>,
) -> Result<T, CoreError> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        Err(CoreError::new(
            "DECODE_FAILED",
            format!("the audio decoder failed while {during}"),
        )
        .with_details(json!({ "backend": BACKEND.0, "during": during, "reason": "panic" })))
    })
}

/// A symphonia error as Finisher's (spec §17). `demux` says whether the format reader (the
/// container) or a decoder (a packet) failed.
pub(crate) fn error(e: SymError, demux: bool) -> CoreError {
    let during = if demux { "demux" } else { "decode" };
    match e {
        // A decoder's I/O error is its bitstream reader running out inside a packet: the packet
        // is bad, not the file system.
        SymError::IoError(io) if !demux => {
            CoreError::new("DECODE_FAILED", format!("audio decode failed: {io}")).with_details(
                json!({ "backend": BACKEND.0, "during": during, "reason": "bitstream" }),
            )
        }
        SymError::IoError(io) if io.kind() == std::io::ErrorKind::UnexpectedEof => {
            CoreError::new("MEDIA_CORRUPT", "the audio file ends early (truncated)")
                .with_details(json!({ "at": "end", "backend": BACKEND.0, "during": during }))
        }
        SymError::IoError(io) => fin_container::error::io(&io),
        SymError::LimitError(what) => CoreError::new(
            "MEDIA_LIMIT",
            format!("the audio file exceeds a reader limit: {what}"),
        )
        .with_details(json!({ "limit": what, "backend": BACKEND.0 })),
        // The probe found no reader for a file whose magic named one: a broken header.
        SymError::Unsupported(what) if demux && what.starts_with("core (probe)") => CoreError::new(
            "MEDIA_CORRUPT",
            format!("the audio container is malformed: {what}"),
        )
        .with_details(json!({ "at": "header", "backend": BACKEND.0, "reason": what })),
        SymError::Unsupported(what) => CoreError::new(
            "CODEC_UNAVAILABLE",
            format!("symphonia cannot read this audio: {what}"),
        )
        .with_details(json!({ "backend": BACKEND.0, "reason": what, "direction": "decode" }))
        .with_fix("see `fin media caps` for what this machine decodes; the codec plugin adds more"),
        SymError::DecodeError(what) if demux => CoreError::new(
            "MEDIA_CORRUPT",
            format!("the audio container is malformed: {what}"),
        )
        .with_details(json!({ "at": "container", "backend": BACKEND.0, "reason": what })),
        other => CoreError::new("DECODE_FAILED", format!("audio decode failed: {other}"))
            .with_details(json!({ "backend": BACKEND.0, "during": during })),
    }
}

/// The model's codec for a symphonia codec id.
pub(crate) fn codec_of(id: AudioCodecId) -> AudioCodec {
    match id {
        ids::CODEC_ID_FLAC => AudioCodec::Flac,
        ids::CODEC_ID_ALAC => AudioCodec::Alac,
        ids::CODEC_ID_MP3 => AudioCodec::Mp3,
        ids::CODEC_ID_VORBIS => AudioCodec::Vorbis,
        ids::CODEC_ID_OPUS => AudioCodec::Opus,
        ids::CODEC_ID_AAC => AudioCodec::Aac,
        ids::CODEC_ID_AC3 => AudioCodec::Ac3,
        ids::CODEC_ID_EAC3 => AudioCodec::Eac3,
        // The PCM block: CODEC_ID_PCM_S32LE (0x100) to CODEC_ID_PCM_MULAW (0x125).
        _ if (ids::CODEC_ID_PCM_S32LE..=ids::CODEC_ID_PCM_MULAW).contains(&id) => AudioCodec::Pcm,
        _ => AudioCodec::Other,
    }
}

/// The symphonia codec id the MP4 path can decode without a configuration record.
pub(crate) fn id_of(codec: AudioCodec) -> Option<AudioCodecId> {
    match codec {
        AudioCodec::Mp3 => Some(ids::CODEC_ID_MP3),
        _ => None,
    }
}

/// Positions for a symphonia channel set, in its buffer order (lowest position bit first; bits
/// 0–10 mean what the WAVE mask's do, later ones are `Unknown`). Discrete channels have no
/// positions: one is centre, two are left and right, more are `Unknown` (refused when mixed,
/// Decision 3).
pub(crate) fn layout_of(ch: &Channels) -> ChannelLayout {
    let n = ch.count();
    let bits: Vec<Option<u64>> = match ch {
        Channels::Positioned(p) => (0..64)
            .map(|b| 1u64 << b)
            .filter(|b| p.bits() & b != 0)
            .map(Some)
            .collect(),
        Channels::Custom(labels) => labels
            .iter()
            .map(|l| match l {
                ChannelLabel::Positioned(p) if p.bits().count_ones() == 1 => Some(p.bits()),
                _ => None,
            })
            .collect(),
        _ => {
            return match n {
                1 => ChannelLayout(vec![ChannelPos::C]),
                2 => ChannelLayout(vec![ChannelPos::L, ChannelPos::R]),
                _ => ChannelLayout::unknown(n),
            };
        }
    };
    let all = bits.iter().flatten().fold(0u64, |a, b| a | b);
    // Back left/right are `Lb`/`Rb` only beside side left/right (as in `wave_mask`).
    let sides = all & (1 << 9) != 0 && all & (1 << 10) != 0;
    ChannelLayout(
        bits.into_iter()
            .map(|b| match b.map(u64::trailing_zeros) {
                Some(4) if sides => ChannelPos::Lb,
                Some(5) if sides => ChannelPos::Rb,
                Some(bit @ 0..=10) => ChannelLayout::wave_mask(1 << bit, 1).0[0],
                _ => ChannelPos::Unknown,
            })
            .collect(),
    )
}

/// A symphonia decoder behind the [`AudioDecoder`] trait.
pub(crate) struct SymphoniaDecoder {
    dec: Box<dyn SymDecoder>,
    out: Option<DecodedAudio>,
}

impl SymphoniaDecoder {
    pub(crate) fn open(params: &AudioCodecParameters) -> Result<Self, CoreError> {
        let opts = AudioDecoderOptions::default().gapless(true);
        let dec = guarded("opening the decoder", || {
            symphonia::default::get_codecs()
                .make_audio_decoder(params, &opts)
                .map_err(|e| error(e, false))
        })?;
        Ok(SymphoniaDecoder { dec, out: None })
    }

    /// Decodes a packet from one of symphonia's format readers (its trims applied: gapless).
    pub(crate) fn decode(&mut self, packet: &Packet) -> Result<(), CoreError> {
        self.decode_ref(&packet.as_packet_ref(), packet.pts.get())
    }

    fn decode_ref(&mut self, packet: &PacketRef<'_>, pts: i64) -> Result<(), CoreError> {
        let dec = &mut self.dec;
        let out = guarded("decoding a packet", || {
            let buf = dec.decode_ref(packet).map_err(|e| error(e, false))?;
            let layout = layout_of(buf.spec().channels());
            let rate = buf.spec().rate();
            let mut interleaved = Vec::new();
            buf.copy_to_vec_interleaved::<f32>(&mut interleaved);
            Ok(DecodedAudio {
                interleaved,
                layout,
                rate,
                pts,
            })
        })?;
        self.out = (!out.interleaved.is_empty()).then_some(out);
        Ok(())
    }
}

impl AudioDecoder for SymphoniaDecoder {
    fn send(&mut self, packet: &[u8], pts: i64) -> Result<(), CoreError> {
        if self.out.is_some() {
            return Err(CoreError::new(
                "DECODE_FAILED",
                "the previous packet's audio was not received",
            )
            .with_details(json!({ "backend": BACKEND.0 })));
        }
        let p = PacketRef::new(0, Timestamp::new(pts), Duration::new(0), packet);
        self.decode_ref(&p, pts)
    }

    fn receive(&mut self) -> Result<Option<DecodedAudio>, CoreError> {
        Ok(self.out.take())
    }

    fn finish(&mut self) -> Result<(), CoreError> {
        Ok(())
    }

    fn flush(&mut self) -> Result<(), CoreError> {
        self.out = None;
        let dec = &mut self.dec;
        guarded("resetting the decoder", || {
            dec.reset();
            Ok(())
        })
    }

    fn backend(&self) -> crate::traits::BackendId {
        BACKEND
    }
}

/// A channel set of `n` positioned channels in the WAVE default order (the MP4 path's MP3).
pub(crate) fn channels_for(n: u16) -> Option<Channels> {
    Position::from_wave_channel_count(n as u32).map(Channels::Positioned)
}
