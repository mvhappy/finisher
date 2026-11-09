//! Codec backends, their vendor classes and the encoder policy (sub-project 6 spec §6.1–6.2,
//! §9.2; plan 6B-1 Decisions 2 and 3; owner decision 9 and follow-up F3).
//!
//! Every backend has a [`VendorClass`]: `os` (VideoToolbox, Microsoft's own MFTs), `gpu` (a
//! GPU vendor's MFT; VA-API in 6B-2, NVDEC/NVENC in 6G), `software` (dav1d, SVT-AV1) or
//! `plugin` (the codec plugin). [`encoder_allowed`] is **the one function** that decides which
//! class may encode a codec for a purpose; `fin_codecs::open_encoder` filters its candidates
//! through it and no other code opens an encoder (`scripts/check-encoder-call-sites.mjs`).

use fin_core::CoreError;
use fin_core::events::names_media::BackendClass;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::format::VideoCodec;

/// Who provides a backend (spec §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum VendorClass {
    /// Part of the OS, from the OS vendor.
    Os,
    /// A GPU vendor's codec (fail closed: a hardware MFT that reports no vendor id is `gpu`).
    Gpu,
    /// Built from source in Finisher.
    Software,
    /// The codec plugin (spec §6.5).
    Plugin,
}

impl VendorClass {
    pub const ALL: [VendorClass; 4] = [
        VendorClass::Os,
        VendorClass::Gpu,
        VendorClass::Software,
        VendorClass::Plugin,
    ];
}

impl From<VendorClass> for BackendClass {
    fn from(c: VendorClass) -> BackendClass {
        match c {
            VendorClass::Os => BackendClass::Os,
            VendorClass::Gpu => BackendClass::Gpu,
            VendorClass::Software => BackendClass::Software,
            VendorClass::Plugin => BackendClass::Plugin,
        }
    }
}

/// What an encode is for. Exports, QA renders and mattes leave the machine or stand for what
/// would (`distributed`); proxies never leave the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EncodePurpose {
    Export,
    QaRender,
    Matte,
    /// An internal preview proxy (F3): any hardware encoder, `gpu` included.
    Proxy,
    /// Test clips made at test time (spec §13.3): the only purpose besides the codec plugin
    /// that may encode HEVC. Behind the `testing` feature, never in a shipped build.
    #[cfg(feature = "testing")]
    TestFixture,
}

impl EncodePurpose {
    #[cfg(not(feature = "testing"))]
    pub const ALL: [EncodePurpose; 4] = [
        EncodePurpose::Export,
        EncodePurpose::QaRender,
        EncodePurpose::Matte,
        EncodePurpose::Proxy,
    ];
    #[cfg(feature = "testing")]
    pub const ALL: [EncodePurpose; 5] = [
        EncodePurpose::Export,
        EncodePurpose::QaRender,
        EncodePurpose::Matte,
        EncodePurpose::Proxy,
        EncodePurpose::TestFixture,
    ];

    /// Output that leaves the machine, or stands for output that would (owner decision 9).
    pub fn distributed(self) -> bool {
        matches!(
            self,
            EncodePurpose::Export | EncodePurpose::QaRender | EncodePurpose::Matte
        )
    }

    fn test_fixture(self) -> bool {
        #[cfg(feature = "testing")]
        {
            self == EncodePurpose::TestFixture
        }
        #[cfg(not(feature = "testing"))]
        {
            false
        }
    }
}

/// Why [`encoder_allowed`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PolicyReason {
    /// A GPU vendor's encoder for an export, QA render or matte (owner decision 9).
    GpuForDistributed,
    /// HEVC from anything but the codec plugin (no built-in HEVC encoding, owner decision 9).
    HevcBuiltIn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyRefusal {
    pub reason: PolicyReason,
    pub class: VendorClass,
    pub codec: VideoCodec,
    pub purpose: EncodePurpose,
}

impl PolicyRefusal {
    /// `CODEC_UNAVAILABLE` for an encode the policy refuses on `backend`.
    pub fn to_error(&self, backend: &str) -> CoreError {
        let name = |v: serde_json::Value| v.as_str().unwrap_or_default().to_string();
        let (codec, purpose, reason, class) = (
            name(json!(self.codec)),
            name(json!(self.purpose)),
            name(json!(self.reason)),
            name(json!(self.class)),
        );
        let fix = match self.reason {
            PolicyReason::GpuForDistributed => {
                "exports use the OS's own or a software encoder; the codec plugin adds more"
            }
            PolicyReason::HevcBuiltIn => "HEVC encoding comes only from the codec plugin",
        };
        CoreError::new(
            "CODEC_UNAVAILABLE",
            format!("{backend} may not encode {codec} for {purpose} ({reason})"),
        )
        .with_details(json!({
            "codec": codec, "direction": "encode", "purpose": purpose,
            "backend": backend, "class": class, "reason": reason,
        }))
        .with_fix(fix)
    }
}

/// Whether an encoder of `class` may encode `codec` for `purpose` (plan 6B-1 Decision 2):
/// - `Export`, `QaRender` and `Matte` take `os`, `software` and `plugin` encoders, never `gpu`;
/// - `Proxy` takes every class;
/// - HEVC only from the `plugin` class, or for a `TestFixture`;
/// - `TestFixture` takes everything.
///
/// A `gpu` encoder for a distributed purpose is refused as such whatever the codec.
pub fn encoder_allowed(
    class: VendorClass,
    codec: VideoCodec,
    purpose: EncodePurpose,
) -> Result<(), PolicyRefusal> {
    let refuse = |reason| {
        Err(PolicyRefusal {
            reason,
            class,
            codec,
            purpose,
        })
    };
    if purpose.test_fixture() {
        return Ok(());
    }
    if class == VendorClass::Gpu && purpose.distributed() {
        return refuse(PolicyReason::GpuForDistributed);
    }
    if codec == VideoCodec::Hevc && class != VendorClass::Plugin {
        return refuse(PolicyReason::HevcBuiltIn);
    }
    Ok(())
}

/// One backend: its name (a `MEDIA_BACKENDS` entry, the name diagnostics and the capability
/// report use), its class and what it can decode and encode at most (the capability probe
/// says what this machine actually has).
#[derive(Debug, PartialEq, Eq)]
pub struct BackendEntry {
    pub name: &'static str,
    pub class: VendorClass,
    pub decode: &'static [VideoCodec],
    pub encode: &'static [VideoCodec],
}

use VideoCodec::{Av1, H264, Hevc, ProRes, Vp9};

/// The backends 6B-1 builds. Reserved for later batches, not listed until built: `vaapi`
/// (`gpu`, 6B-2), `libvpx` (`software`, VP9 on Linux, 6B-2), `nvdec`/`nvenc` (`gpu`, 6G).
pub static BACKENDS: &[BackendEntry] = &[
    BackendEntry {
        name: "videotoolbox",
        class: VendorClass::Os,
        decode: &[H264, Hevc, Av1, Vp9, ProRes],
        // HEVC encoding is for test fixtures only (the policy refuses it otherwise).
        encode: &[H264, Hevc],
    },
    BackendEntry {
        name: "mf-ms",
        class: VendorClass::Os,
        decode: &[H264, Hevc, Av1, Vp9],
        encode: &[H264],
    },
    BackendEntry {
        name: "mf-hw",
        class: VendorClass::Gpu,
        decode: &[H264, Hevc, Av1, Vp9],
        encode: &[H264],
    },
    BackendEntry {
        name: "dav1d",
        class: VendorClass::Software,
        decode: &[Av1],
        encode: &[],
    },
    BackendEntry {
        name: "svt-av1",
        class: VendorClass::Software,
        decode: &[],
        encode: &[Av1],
    },
];

/// The backend called `name`.
pub fn backend(name: &str) -> Option<&'static BackendEntry> {
    BACKENDS.iter().find(|b| b.name == name)
}
