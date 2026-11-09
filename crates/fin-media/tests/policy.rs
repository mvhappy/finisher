//! The encoder policy (plan 6B-1 Decision 2, owner decision 9 and F3) and the backend table.

use fin_core::events::names_media::{BackendClass, MEDIA_BACKENDS, backend_class};
use fin_media::VideoCodec;
use fin_media::backend::{
    BACKENDS, EncodePurpose, PolicyReason, VendorClass, backend, encoder_allowed,
};

const CODECS: [VideoCodec; 7] = [
    VideoCodec::Av1,
    VideoCodec::H264,
    VideoCodec::Hevc,
    VideoCodec::Vp9,
    VideoCodec::Vp8,
    VideoCodec::ProRes,
    VideoCodec::Other,
];

/// Every class × purpose, one outcome per codec of [`CODECS`] in order: `.` allowed,
/// `G` refused `gpuForDistributed`, `H` refused `hevcBuiltIn`.
#[rustfmt::skip]
const TABLE: &[(VendorClass, EncodePurpose, &str)] = &[
    (VendorClass::Os,       EncodePurpose::Export,      "..H...."),
    (VendorClass::Os,       EncodePurpose::QaRender,    "..H...."),
    (VendorClass::Os,       EncodePurpose::Matte,       "..H...."),
    (VendorClass::Os,       EncodePurpose::Proxy,       "..H...."),
    (VendorClass::Os,       EncodePurpose::TestFixture, "......."),
    (VendorClass::Software, EncodePurpose::Export,      "..H...."),
    (VendorClass::Software, EncodePurpose::QaRender,    "..H...."),
    (VendorClass::Software, EncodePurpose::Matte,       "..H...."),
    (VendorClass::Software, EncodePurpose::Proxy,       "..H...."),
    (VendorClass::Software, EncodePurpose::TestFixture, "......."),
    (VendorClass::Plugin,   EncodePurpose::Export,      "......."),
    (VendorClass::Plugin,   EncodePurpose::QaRender,    "......."),
    (VendorClass::Plugin,   EncodePurpose::Matte,       "......."),
    (VendorClass::Plugin,   EncodePurpose::Proxy,       "......."),
    (VendorClass::Plugin,   EncodePurpose::TestFixture, "......."),
    (VendorClass::Gpu,      EncodePurpose::Export,      "GGGGGGG"),
    (VendorClass::Gpu,      EncodePurpose::QaRender,    "GGGGGGG"),
    (VendorClass::Gpu,      EncodePurpose::Matte,       "GGGGGGG"),
    (VendorClass::Gpu,      EncodePurpose::Proxy,       "..H...."),
    (VendorClass::Gpu,      EncodePurpose::TestFixture, "......."),
];

fn outcome(class: VendorClass, codec: VideoCodec, purpose: EncodePurpose) -> char {
    match encoder_allowed(class, codec, purpose) {
        Ok(()) => '.',
        Err(r) => match r.reason {
            PolicyReason::GpuForDistributed => 'G',
            PolicyReason::HevcBuiltIn => 'H',
        },
    }
}

#[test]
fn policy_matrix() {
    assert_eq!(
        TABLE.len(),
        VendorClass::ALL.len() * EncodePurpose::ALL.len()
    );
    for class in VendorClass::ALL {
        for purpose in EncodePurpose::ALL {
            let want = TABLE
                .iter()
                .find(|(c, p, _)| *c == class && *p == purpose)
                .map(|(_, _, row)| *row)
                .unwrap_or_else(|| panic!("no row for {class:?} {purpose:?}"));
            let got: String = CODECS.iter().map(|&c| outcome(class, c, purpose)).collect();
            assert_eq!(got, want, "{class:?} {purpose:?}");
        }
    }
}

#[test]
fn gpu_never_for_export_qa_matte() {
    for purpose in [
        EncodePurpose::Export,
        EncodePurpose::QaRender,
        EncodePurpose::Matte,
    ] {
        assert!(purpose.distributed());
        for codec in CODECS {
            let r = encoder_allowed(VendorClass::Gpu, codec, purpose).unwrap_err();
            assert_eq!(r.reason, PolicyReason::GpuForDistributed);
        }
    }
    assert!(!EncodePurpose::Proxy.distributed());
    assert!(encoder_allowed(VendorClass::Gpu, VideoCodec::H264, EncodePurpose::Proxy).is_ok());
}

#[test]
fn hevc_encode_only_plugin_or_test_fixture() {
    for class in VendorClass::ALL {
        for purpose in EncodePurpose::ALL {
            let ok = encoder_allowed(class, VideoCodec::Hevc, purpose).is_ok();
            let expected = class == VendorClass::Plugin || purpose == EncodePurpose::TestFixture;
            assert_eq!(ok, expected, "{class:?} {purpose:?}");
        }
    }
}

#[test]
fn a_refusal_is_a_typed_error() {
    let r = encoder_allowed(VendorClass::Gpu, VideoCodec::H264, EncodePurpose::Export).unwrap_err();
    let e = r.to_error("mf-hw");
    assert_eq!(e.code, "CODEC_UNAVAILABLE");
    assert_eq!(e.details["direction"], "encode");
    assert_eq!(e.details["reason"], "gpuForDistributed");
    assert_eq!(e.details["backend"], "mf-hw");
    assert_eq!(e.details["purpose"], "export");
    assert_eq!(e.details["codec"], "h264");
    assert_eq!(
        e.message,
        "mf-hw may not encode h264 for export (gpuForDistributed)"
    );
}

#[test]
fn backend_names_are_unique_and_in_the_journal_table() {
    let mut names: Vec<&str> = BACKENDS.iter().map(|b| b.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), BACKENDS.len(), "duplicate backend names");
    for b in BACKENDS {
        assert!(
            MEDIA_BACKENDS.get(b.name).is_some(),
            "{} is not in MEDIA_BACKENDS",
            b.name
        );
        assert_eq!(
            backend_class(b.name),
            BackendClass::from(b.class),
            "{}: the journal's class differs from the policy's",
            b.name
        );
        assert_eq!(backend(b.name).map(|x| x.name), Some(b.name));
    }
    assert!(backend("ffmpeg-lgpl").is_none());
}

#[test]
fn the_backend_table_as_planned() {
    let row = |n: &str| backend(n).unwrap_or_else(|| panic!("{n}"));
    assert_eq!(row("videotoolbox").class, VendorClass::Os);
    assert_eq!(row("mf-ms").class, VendorClass::Os);
    assert_eq!(row("mf-hw").class, VendorClass::Gpu);
    assert_eq!(row("dav1d").class, VendorClass::Software);
    assert_eq!(row("svt-av1").class, VendorClass::Software);
    assert_eq!(row("dav1d").decode, &[VideoCodec::Av1]);
    assert!(row("dav1d").encode.is_empty());
    assert_eq!(row("svt-av1").encode, &[VideoCodec::Av1]);
    // No H.264 or HEVC of our own (D13): only OS and GPU backends name them.
    for b in BACKENDS {
        if matches!(b.class, VendorClass::Software) {
            for c in b.decode.iter().chain(b.encode) {
                assert!(
                    !matches!(c, VideoCodec::H264 | VideoCodec::Hevc),
                    "{}",
                    b.name
                );
            }
        }
    }
}
