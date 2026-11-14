//! The audio encoder policy (6C plan Decision 10) and the audio backend table (Decision 24).

use fin_media::AudioCodec;
use fin_media::backend::{EncodePurpose, PolicyReason, VendorClass};
use fin_media::backend_audio::{AUDIO_BACKENDS, audio_backend, audio_encoder_allowed};

#[test]
fn audio_policy_matrix() {
    use EncodePurpose::*;
    for purpose in EncodePurpose::ALL {
        for class in VendorClass::ALL {
            for codec in [AudioCodec::Aac, AudioCodec::Opus] {
                let got = audio_encoder_allowed(class, codec, purpose);
                let refused =
                    class == VendorClass::Gpu && matches!(purpose, Export | QaRender | Matte);
                assert_eq!(got.is_err(), refused, "{class:?} {codec:?} {purpose:?}");
                if let Err(r) = got {
                    assert_eq!(r.reason, PolicyReason::GpuForDistributed);
                    let e = r.to_error("hypothetical");
                    assert_eq!(e.code, "CODEC_UNAVAILABLE");
                    assert_eq!(e.details["direction"], "encode");
                    assert_eq!(e.details["class"], "gpu");
                }
            }
        }
    }
    // A hypothetical GPU AAC encoder: refused for an export, allowed for a proxy.
    assert!(audio_encoder_allowed(VendorClass::Gpu, AudioCodec::Aac, Export).is_err());
    assert!(audio_encoder_allowed(VendorClass::Gpu, AudioCodec::Aac, Proxy).is_ok());
}

#[test]
fn symphonia_row() {
    let s = audio_backend("symphonia").expect("symphonia is listed");
    assert_eq!(s.class, VendorClass::Software);
    for c in [
        AudioCodec::Pcm,
        AudioCodec::Flac,
        AudioCodec::Alac,
        AudioCodec::Mp3,
        AudioCodec::Vorbis,
    ] {
        assert!(s.decode.contains(&c), "{c:?}");
    }
    // AAC-LC is symphonia's on Linux only (Decision 9); symphonia encodes nothing.
    assert_eq!(
        s.decode.contains(&AudioCodec::Aac),
        cfg!(target_os = "linux")
    );
    assert!(s.encode.is_empty());
    // Every name is one of the diagnostics' closed table (Decision 24).
    for b in AUDIO_BACKENDS {
        assert!(
            fin_core::events::names_media::MEDIA_BACKENDS
                .get(b.name)
                .is_some(),
            "{}",
            b.name
        );
    }
}

#[test]
fn vo_aacenc_row_on_linux_only() {
    let row = audio_backend("vo-aacenc");
    assert_eq!(row.is_some(), cfg!(target_os = "linux"), "{row:?}");
    if let Some(b) = row {
        assert_eq!(b.class, VendorClass::Software);
        assert_eq!((b.decode, b.encode), (&[][..], &[AudioCodec::Aac][..]));
    }
}

#[test]
fn vorbis_from_fourcc() {
    assert_eq!(AudioCodec::from_fourcc(b"vorb"), AudioCodec::Vorbis);
}
