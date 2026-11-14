//! Where `media.thumbs` reads its frames (plan 6B-1 Task 15): the `1080` proxy entry the host
//! found, else the original. Either way the caller sees the *original's* sample table, media
//! information and decoder class (a proxy's `table.bin` and `meta.json` carry them), so the
//! frame `frame_at` picks on that table is proxy frame `i` of the proxy (the one-frame-per-frame
//! contract, Decision 7) and the cache key does not depend on which source made a thumbnail.
//!
//! The proxy is the `1080` entry the job's manifest lists for the asset (the host pinned it for
//! the job, Decision 12), opened and verified by [`proxy_source::open`] as a look's is: no-follow,
//! its key, content, tier and listed sizes, and the BLAKE3 digests of `table.bin` and
//! `proxy.mp4` through the handles read, then one proxy frame per original frame. It must also
//! be of this build's `RENDER_VERSION` and keep the original's media information. Anything else
//! is a [`Refusal`]: the caller reads the original instead and reports its reason.
//!
//! An original with Dolby Vision profile 5 is refused (`CODEC_UNAVAILABLE`, Decision 9), as
//! `media.proxy` refuses it.

use std::fs::File;

use fin_codecs::{DecodeThreads, FrameReader};
use fin_container::Limits;
use fin_core::CoreError;
use fin_media::RENDER_VERSION;
use fin_media::backend::{VendorClass, backend};
use fin_media::jobs::{ManifestProxy, ToneMapped};
use fin_media::{MediaInfo, SampleTable, VideoTrackInfo};

use super::Warm;
use super::open_cache::WORKER_MAX_PIXELS;
use super::proxy::{Fail, refuse_dolby_vision_5};
use super::proxy_source::{self, Refusal};

/// Where the frames come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Proxy,
    Original,
}

impl Origin {
    pub fn name(self) -> &'static str {
        match self {
            Origin::Proxy => "proxy",
            Origin::Original => "original",
        }
    }
}

/// An open source of frames, indexed by the original's presented frame index.
pub struct Source {
    pub reader: FrameReader<File>,
    /// The original's sample table: `frame_at` runs on it.
    pub table: SampleTable,
    /// The original's media information.
    pub info: MediaInfo,
    /// The original's video track: display size, rotation, HDR.
    pub video: VideoTrackInfo,
    /// What describes the colours of the frames `reader` yields: the original's track, or the
    /// proxy's own (BT.709 SDR).
    pub ingest: VideoTrackInfo,
    /// The vendor class of the backend that decodes the original.
    pub decoder_class: VendorClass,
    pub origin: Origin,
    /// How the original was brought to SDR (a proxy's entry records it).
    pub tone_mapped: Option<ToneMapped>,
}

impl Source {
    /// An open original (Dolby Vision profile 5 refused).
    pub fn original(file: File, warm: &Warm) -> Result<Source, Fail> {
        let reader = FrameReader::open_with_limit(
            file,
            &Limits::default(),
            &warm.probe,
            DecodeThreads::default(),
            WORKER_MAX_PIXELS,
        )?;
        let decoder = reader.backend().0;
        let decoder_class = backend(decoder).map(|b| b.class).ok_or_else(|| {
            CoreError::new("INTERNAL", format!("{decoder} is not a known backend"))
        })?;
        let info = reader.info().clone();
        let video = info
            .video
            .clone()
            .ok_or_else(|| CoreError::new("NOT_FOUND", "the file has no video track"))?;
        refuse_dolby_vision_5(&video)?;
        Ok(Source {
            table: reader.table().clone(),
            reader,
            info,
            ingest: video.clone(),
            video,
            decoder_class,
            origin: Origin::Original,
            tone_mapped: None,
        })
    }

    /// The listed proxy entry `proxy` of the content `fingerprint` (module docs).
    pub fn proxy(proxy: &ManifestProxy, fingerprint: &str, warm: &Warm) -> Result<Source, Refusal> {
        let o = proxy_source::open(proxy, fingerprint, &warm.probe)?;
        if o.meta.render_version != RENDER_VERSION {
            return Err(Refusal::Invalid);
        }
        let info = o.meta.media.ok_or(Refusal::Invalid)?;
        let video = info.video.clone().ok_or(Refusal::Invalid)?;
        let ingest = o.reader.info().video.clone().ok_or(Refusal::Invalid)?;
        Ok(Source {
            reader: o.reader,
            table: o.side.table,
            info,
            video,
            ingest,
            decoder_class: o.meta.decoder_class,
            origin: Origin::Proxy,
            tone_mapped: o.meta.tone_mapped,
        })
    }
}
