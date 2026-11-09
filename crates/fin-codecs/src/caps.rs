//! What this machine can decode and encode (spec §6.2), as `media.capabilities` v1 reports it
//! (plan 6A Task 13); backend choice is `registry`'s.

use fin_media::VideoCodec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Support {
    Hardware,
    Software,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodecSupport {
    pub codec: VideoCodec,
    pub decode: Support,
    pub encode: Support,
    /// The backends that would be used, decode first.
    pub backends: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub video: Vec<CodecSupport>,
}

impl Capabilities {
    pub fn get(&self, codec: VideoCodec) -> Option<&CodecSupport> {
        self.video.iter().find(|c| c.codec == codec)
    }
}

/// The v1 table of 6A's software backends (dav1d decode, SVT-AV1 encode): what
/// `ProbeResult::software_only().capabilities()` answers. The capability probe replaces it
/// in 6B-1 Task 8.
pub fn capabilities() -> Capabilities {
    crate::registry::ProbeResult::software_only().capabilities()
}
