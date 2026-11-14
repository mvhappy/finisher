//! 6C Task 20 re-check: `fin_media::jobs::audio_decoder_for_proxy` (the proxy key's audio half,
//! planned by the host without `fin-codecs`) names exactly the backend `open_audio_decoder`
//! opens for an MP4 audio track of each codec, and `None` exactly where it refuses with
//! `CODEC_UNAVAILABLE`. Tasks 15–16 change the routing; this pins the table to it.

use fin_codecs::audio::{AudioConfig, AudioEncoderConfig, open_audio_decoder, open_audio_encoder};
use fin_container::CodecConfig;
use fin_media::backend::EncodePurpose;
use fin_media::jobs::audio_decoder_for_proxy;
use fin_media::{AudioCodec, AudioTrackInfo, ContainerKind};

/// Every codec (a new variant fails to compile here until it is given a sample entry).
const CODECS: [AudioCodec; 10] = [
    AudioCodec::Aac,
    AudioCodec::Opus,
    AudioCodec::Mp3,
    AudioCodec::Flac,
    AudioCodec::Alac,
    AudioCodec::Pcm,
    AudioCodec::Ac3,
    AudioCodec::Eac3,
    AudioCodec::Vorbis,
    AudioCodec::Other,
];

/// The configuration record an encoder of this build gives for `codec`, when it has one.
fn encoded_config(codec: AudioCodec) -> Option<CodecConfig> {
    let cfg = AudioEncoderConfig {
        codec,
        rate: 48_000,
        channels: 2,
        bitrate: 128_000,
    };
    open_audio_encoder(&cfg, EncodePurpose::TestFixture)
        .ok()
        .map(|e| e.config())
}

/// The sample entry the MP4 demuxer gives for an audio track of `codec`: the four-character code
/// and the configuration record.
fn sample_entry(codec: AudioCodec) -> (&'static str, CodecConfig) {
    match codec {
        // A dOps from libopus.
        AudioCodec::Opus => ("Opus", encoded_config(codec).expect("libopus encodes")),
        // An AAC-LC esds: vo-aacenc's where it exists (Linux), else a hand-made one (LC, 48 kHz,
        // stereo: AudioSpecificConfig 0x11 0x90).
        AudioCodec::Aac => (
            "mp4a",
            encoded_config(codec).unwrap_or_else(|| CodecConfig::Aac {
                esds: esds_lc(&[0x11, 0x90]),
            }),
        ),
        AudioCodec::Mp3 => (".mp3", CodecConfig::Other { fourcc: *b".mp3" }),
        AudioCodec::Flac => ("fLaC", CodecConfig::Other { fourcc: *b"fLaC" }),
        AudioCodec::Alac => ("alac", CodecConfig::Other { fourcc: *b"alac" }),
        AudioCodec::Pcm => ("sowt", CodecConfig::Other { fourcc: *b"sowt" }),
        AudioCodec::Ac3 => ("ac-3", CodecConfig::Other { fourcc: *b"ac-3" }),
        AudioCodec::Eac3 => ("ec-3", CodecConfig::Other { fourcc: *b"ec-3" }),
        AudioCodec::Vorbis => ("vorb", CodecConfig::Other { fourcc: *b"vorb" }),
        AudioCodec::Other => ("xxxx", CodecConfig::Other { fourcc: *b"xxxx" }),
    }
}

/// An `esds` box body (version and flags, then ES_Descriptor → DecoderConfigDescriptor →
/// DecoderSpecificInfo `asc`, then SLConfigDescriptor).
fn esds_lc(asc: &[u8]) -> Vec<u8> {
    let dsi = [&[0x05, asc.len() as u8][..], asc].concat();
    let dcd_body = [
        &[
            0x40, 0x15, 0x00, 0x00, 0x00, 0x00, 0x01, 0xF4, 0x00, 0x00, 0x01, 0xF4, 0x00,
        ][..],
        &dsi,
    ]
    .concat();
    let dcd = [&[0x04, dcd_body.len() as u8][..], &dcd_body].concat();
    let es_body = [&[0x00, 0x01, 0x00][..], &dcd, &[0x06, 0x01, 0x02]].concat();
    [&[0, 0, 0, 0][..], &[0x03, es_body.len() as u8], &es_body].concat()
}

#[test]
fn the_proxy_key_s_audio_decoder_is_the_one_mp4_audio_opens() {
    for codec in CODECS {
        let (fourcc, config) = sample_entry(codec);
        let info = AudioTrackInfo {
            codec,
            fourcc: fourcc.into(),
            sample_rate: 48_000,
            channels: 2,
            priming: 0,
            padding: 0,
        };
        let predicted = audio_decoder_for_proxy(ContainerKind::Mp4, codec).map(|b| b.name);
        let opened = open_audio_decoder(&info, &AudioConfig::Record(config));
        match (&opened, predicted) {
            (Ok(d), _) => assert_eq!(Some(d.backend().0), predicted, "{codec:?}"),
            (Err(e), None) => assert_eq!(e.code, "CODEC_UNAVAILABLE", "{codec:?}: {e:?}"),
            (Err(e), Some(p)) => panic!("{codec:?}: predicted {p}, the decoder refused: {e:?}"),
        }
        // MOV routes the same way.
        assert_eq!(
            audio_decoder_for_proxy(ContainerKind::Mov, codec).map(|b| b.name),
            predicted,
            "{codec:?}"
        );
    }
}
