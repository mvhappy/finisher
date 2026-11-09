//! Backend choice (plan 6B-1 Task 3, spec §6.1–6.2): which decoder serves a stream and which
//! encoders may serve an encode. A [`ProbeResult`] lists the backends this machine has (6A's
//! software pair until the capability probe of Task 8); [`open_decoder`] takes the first that
//! decodes the stream's codec, profile and bit depth, OS before GPU before software, and
//! [`open_encoder`] filters its candidates through `fin_media::backend::encoder_allowed`, the
//! one policy function. **No other code opens an encoder** (`check-encoder-call-sites`).

use std::sync::Arc;

use fin_container::CodecConfig;
use fin_core::CoreError;
use fin_media::VideoCodec;
use fin_media::backend::{BackendEntry, VendorClass, backend, encoder_allowed};
use fin_media::jobs::Profile;
use serde_json::json;

use crate::caps::{Capabilities, CodecSupport, Support};
use crate::config::{AdapterBinding, DecoderConfig, EncoderConfig, StreamDesc};
use crate::hung::EncoderSession;
use crate::traits::{VideoDecoder, VideoEncoder};

/// What only this crate can make: an encoder backend's `open` takes one, so nothing outside
/// `fin-codecs` opens an encoder except through [`open_encoder`] and the policy.
pub struct OpenToken(pub(crate) ());

/// A decoder this machine has.
pub trait DecoderBackend: Send + Sync {
    fn entry(&self) -> &'static BackendEntry;
    /// Decodes on hardware.
    fn hw(&self) -> bool;
    /// The GPU a hardware decoder runs on (`None`: any, or none).
    fn adapter(&self) -> Option<AdapterBinding> {
        None
    }
    /// Decodes `s` (codec, profile and bit depth) on this machine.
    fn decodes(&self, s: &StreamDesc) -> bool;
    fn open(&self, cfg: &DecoderConfig) -> Result<Box<dyn VideoDecoder>, CoreError>;
}

/// An encoder this machine has.
pub trait EncoderBackend: Send + Sync {
    fn entry(&self) -> &'static BackendEntry;
    /// Encodes on hardware.
    fn hw(&self) -> bool;
    /// The GPU a hardware encoder runs on (`None`: any, or none).
    fn adapter(&self) -> Option<AdapterBinding> {
        None
    }
    /// Encodes `codec` (at `profile`, when one is asked for) on this machine.
    fn encodes(&self, codec: VideoCodec, profile: Option<Profile>) -> bool;
    /// Opens a session, on the session's own thread (set per-thread state up here). Only
    /// [`open_encoder`]'s session can call it (`OpenToken`).
    fn open(
        &self,
        cfg: &EncoderConfig,
        token: OpenToken,
    ) -> Result<Box<dyn VideoEncoder>, CoreError>;
}

/// The backends this machine has, in the order the probe found them.
#[derive(Clone)]
pub struct ProbeResult {
    decoders: Vec<Arc<dyn DecoderBackend>>,
    encoders: Vec<Arc<dyn EncoderBackend>>,
}

impl std::fmt::Debug for ProbeResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names = |v: Vec<&str>| v.join(", ");
        f.debug_struct("ProbeResult")
            .field(
                "decoders",
                &names(self.decoders.iter().map(|d| d.entry().name).collect()),
            )
            .field(
                "encoders",
                &names(self.encoders.iter().map(|e| e.entry().name).collect()),
            )
            .finish()
    }
}

/// Preference by class: the OS's own, then a GPU vendor's, then ours, then the plugin.
fn rank(class: VendorClass) -> u8 {
    match class {
        VendorClass::Os => 0,
        VendorClass::Gpu => 1,
        VendorClass::Software => 2,
        VendorClass::Plugin => 3,
    }
}

impl ProbeResult {
    pub fn new(
        decoders: Vec<Arc<dyn DecoderBackend>>,
        encoders: Vec<Arc<dyn EncoderBackend>>,
    ) -> ProbeResult {
        ProbeResult { decoders, encoders }
    }

    /// 6A's backends: dav1d (AV1 decode) and SVT-AV1 (AV1 encode), in software.
    pub fn software_only() -> ProbeResult {
        ProbeResult::new(
            vec![Arc::new(crate::av1::Dav1dBackend)],
            vec![Arc::new(crate::av1::SvtBackend)],
        )
    }

    pub fn decoders(&self) -> &[Arc<dyn DecoderBackend>] {
        &self.decoders
    }

    pub fn encoders(&self) -> &[Arc<dyn EncoderBackend>] {
        &self.encoders
    }

    /// The decoders that decode `s` (on `adapter`, when one is named and the decoder is bound
    /// to a GPU), in the order they are tried: hardware first (the compositor's adapter's
    /// hardware MFT before Microsoft's software decoder, arch §27.3), then by class (OS, GPU,
    /// software, plugin), then as probed.
    pub fn decoder_candidates(
        &self,
        s: &StreamDesc,
        adapter: Option<AdapterBinding>,
    ) -> Vec<Arc<dyn DecoderBackend>> {
        let mut v: Vec<_> = (self.decoders.iter())
            .filter(|d| d.decodes(s) && on(d.adapter(), adapter))
            .cloned()
            .collect();
        v.sort_by_key(|d| (!d.hw(), rank(d.entry().class)));
        v
    }

    /// The encoders that encode `codec` at `profile`, in the order they are tried: hardware
    /// first (a proxy takes the fastest; spec §7.1), then by class, then as probed. The policy
    /// is applied by [`open_encoder`], not here.
    pub fn encoder_candidates(
        &self,
        codec: VideoCodec,
        profile: Option<Profile>,
        adapter: Option<AdapterBinding>,
    ) -> Vec<Arc<dyn EncoderBackend>> {
        let mut v: Vec<_> = (self.encoders.iter())
            .filter(|e| e.entry().encode.contains(&codec) && e.encodes(codec, profile))
            .filter(|e| on(e.adapter(), adapter))
            .cloned()
            .collect();
        v.sort_by_key(|e| (!e.hw(), rank(e.entry().class)));
        v
    }

    /// The v1 report (`media.capabilities` v1, plan 6A Task 13): per codec whether it is
    /// decoded and encoded, in hardware or software, and by which backends (decode first).
    pub fn capabilities(&self) -> Capabilities {
        let codecs = [
            VideoCodec::Av1,
            VideoCodec::H264,
            VideoCodec::Hevc,
            VideoCodec::Vp9,
            VideoCodec::ProRes,
        ];
        let level = |hw: Option<bool>| match hw {
            Some(true) => Support::Hardware,
            Some(false) => Support::Software,
            None => Support::None,
        };
        let video = codecs
            .into_iter()
            .map(|codec| {
                let s = StreamDesc {
                    codec,
                    profile: None,
                    bit_depth: 8,
                    chroma: fin_media::Chroma::Yuv420,
                };
                let dec = self.decoder_candidates(&s, None);
                let enc = self.encoder_candidates(codec, None, None);
                let mut backends: Vec<String> = Vec::new();
                for name in dec
                    .iter()
                    .map(|d| d.entry().name)
                    .chain(enc.iter().map(|e| e.entry().name))
                {
                    if !backends.iter().any(|b| b == name) {
                        backends.push(name.to_string());
                    }
                }
                CodecSupport {
                    codec,
                    decode: level(dec.iter().map(|d| d.hw()).max()),
                    encode: level(enc.iter().map(|e| e.hw()).max()),
                    backends,
                }
            })
            .collect();
        Capabilities { video }
    }
}

/// A backend bound to `bound` may serve a session asked for `want`: when both name a GPU, the
/// same one (by LUID when both have one, else by ids).
fn on(bound: Option<AdapterBinding>, want: Option<AdapterBinding>) -> bool {
    match (bound, want) {
        (Some(b), Some(w)) => match (b.luid, w.luid) {
            (Some(x), Some(y)) => x == y,
            _ => b.id.vendor_id == w.id.vendor_id && b.id.device_id == w.id.device_id,
        },
        _ => true,
    }
}

/// `CODEC_UNAVAILABLE` for a codec nothing here serves.
pub(crate) fn unavailable(codec: &str, direction: &str) -> CoreError {
    CoreError::new(
        "CODEC_UNAVAILABLE",
        format!("no {codec} {direction}r on this machine"),
    )
    .with_details(json!({ "codec": codec, "direction": direction, "os": std::env::consts::OS }))
    .with_fix("see `fin media caps` for what this machine decodes; the codec plugin adds more")
}

pub(crate) fn codec_name(codec: VideoCodec) -> String {
    serde_json::to_value(codec)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// A decoder for `cfg.track`: the first candidate that decodes its codec, profile and bit
/// depth and opens (one that fails to open gives way to the next); `CODEC_UNAVAILABLE` when
/// none decodes it, else the last candidate's error.
pub fn open_decoder(
    cfg: &DecoderConfig,
    caps: &ProbeResult,
) -> Result<Box<dyn VideoDecoder>, CoreError> {
    if matches!(
        cfg.track.config,
        CodecConfig::Aac { .. } | CodecConfig::Opus { .. }
    ) {
        return Err(unavailable("audio", "decode"));
    }
    let s = cfg.stream();
    let mut last = None;
    for d in caps.decoder_candidates(&s, cfg.adapter) {
        match d.open(cfg) {
            Ok(dec) => return Ok(dec),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| unavailable(&codec_name(s.codec), "decode")))
}

/// An encoder session for `cfg`: the candidates that encode its codec, filtered by
/// [`encoder_allowed`] for `cfg.purpose`, tried in order (one that fails to open gives way to
/// the next, and the rest stay as fallbacks for `ENCODER_HUNG`, plan 6B-1 Decision 20).
/// `CODEC_UNAVAILABLE` when none encodes the codec, or (with the policy's reason) when the
/// policy refuses every one.
pub fn open_encoder(cfg: &EncoderConfig, caps: &ProbeResult) -> Result<EncoderSession, CoreError> {
    let candidates = caps.encoder_candidates(cfg.codec, cfg.profile, cfg.adapter);
    let Some(first) = candidates.first() else {
        return Err(unavailable(&codec_name(cfg.codec), "encode"));
    };
    let refusal = encoder_allowed(first.entry().class, cfg.codec, cfg.purpose)
        .err()
        .map(|r| r.to_error(first.entry().name));
    let allowed: Vec<_> = candidates
        .iter()
        .filter(|e| encoder_allowed(e.entry().class, cfg.codec, cfg.purpose).is_ok())
        .cloned()
        .collect();
    if allowed.is_empty() {
        return Err(refusal.expect("the first candidate was refused"));
    }
    EncoderSession::start(cfg.clone(), allowed.into())
}

/// The policy table's entry for `name` (a backend's own `entry()`).
pub(crate) fn entry(name: &str) -> &'static BackendEntry {
    backend(name).unwrap_or_else(|| panic!("{name} is not in fin_media::backend::BACKENDS"))
}
