//! Where `media.thumbs` reads its frames (plan 6B-1 Task 15): the `1080` proxy entry the host
//! found, else the original. Either way the caller sees the *original's* sample table, media
//! information and decoder class (a proxy's `table.bin` and `meta.json` carry them), so the
//! frame `frame_at` picks on that table is proxy frame `i` of the proxy (the one-frame-per-frame
//! contract, Decision 7) and the cache key does not depend on which source made a thumbnail.
//!
//! The proxy entry is opened below its directory with `fin-shm`'s no-follow opener, its
//! `meta.json` checked against the request (key, recipe, content, versions) and its file sizes
//! against the sizes it lists. Errors name no path: the caller only keeps their code (a proxy
//! that cannot be read is replaced by the original).

use std::fs::File;
use std::io::Read;
use std::path::Path;

use fin_codecs::{DecodeThreads, FrameReader};
use fin_container::Limits;
use fin_core::CoreError;
use fin_media::RENDER_VERSION;
use fin_media::backend::{VendorClass, backend};
use fin_media::cache::{EntryMeta, META_FILE, PROXY_1080};
use fin_media::jobs::{ThumbsProxy, ToneMapped};
use fin_media::table_bin::{self, TABLE_FILE};
use fin_media::{MediaInfo, SampleTable, VideoTrackInfo};
use serde_json::json;

use super::Warm;
use super::open_cache::WORKER_MAX_PIXELS;
use super::proxy::PROXY_FILE;

/// The largest `meta.json` of a proxy entry read.
const META_CAP: u64 = 16 << 20;
/// The largest `table.bin` read.
const TABLE_CAP: u64 = 512 << 20;

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
    /// An open original.
    pub fn original(file: File, warm: &Warm) -> Result<Source, CoreError> {
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

    /// The proxy entry `proxy` of the content `fingerprint`.
    pub fn proxy(proxy: &ThumbsProxy, fingerprint: &str, warm: &Warm) -> Result<Source, CoreError> {
        let dir = Path::new(&proxy.dir);
        let meta: EntryMeta = {
            let bytes = read(dir, META_FILE, META_CAP, META_CAP)?;
            serde_json::from_slice(&bytes).map_err(|_| refuse("meta.json does not parse"))?
        };
        meta.check()?;
        if meta.key != proxy.key
            || meta.fingerprint != fingerprint
            || meta.recipe != PROXY_1080.name
            || meta.recipe_version != PROXY_1080.version
            || meta.render_version != RENDER_VERSION
        {
            return Err(refuse("the entry is not this content's current 1080 proxy"));
        }
        let size_of = |name: &str| {
            meta.files
                .iter()
                .find(|f| f.name == name)
                .map(|f| f.bytes)
                .ok_or_else(|| refuse("the entry lists no such file"))
        };
        let table = table_bin::decode(&read(dir, TABLE_FILE, size_of(TABLE_FILE)?, TABLE_CAP)?)?;
        let info = meta
            .media
            .clone()
            .ok_or_else(|| refuse("the entry keeps no media information"))?;
        let video = info
            .video
            .clone()
            .ok_or_else(|| refuse("the entry's media has no video track"))?;
        let file = open(dir, PROXY_FILE, size_of(PROXY_FILE)?)?;
        let reader = FrameReader::open_with_limit(
            file,
            &Limits::default(),
            &warm.probe,
            DecodeThreads::default(),
            WORKER_MAX_PIXELS,
        )?;
        if reader.table().len() != table.len() {
            return Err(CoreError::new(
                "MEDIA_CORRUPT",
                "the proxy does not hold one frame per original frame",
            )
            .with_details(json!({ "reason": "proxyFrames" })));
        }
        let ingest = reader
            .info()
            .video
            .clone()
            .ok_or_else(|| refuse("the proxy has no video track"))?;
        Ok(Source {
            reader,
            table,
            info,
            video,
            ingest,
            decoder_class: meta.decoder_class,
            origin: Origin::Proxy,
            tone_mapped: meta.tone_mapped,
        })
    }
}

fn refuse(why: &str) -> CoreError {
    CoreError::new("PLAN_MISMATCH", why).with_details(json!({ "reason": "cacheEntry" }))
}

/// `name` below `dir`, opened no-follow with at most `cap` bytes.
fn open(dir: &Path, name: &str, cap: u64) -> Result<File, CoreError> {
    fin_shm::checked_open(&dir.join(name), cap).map_err(|e| {
        // The error names a path of this host: keep its reason, not the path.
        let reason = e.details.get("reason").cloned().unwrap_or_default();
        CoreError {
            code: e.code,
            message: format!("{name} cannot be opened"),
            details: json!({ "file": name, "reason": reason }),
            fix: None,
        }
    })
}

/// All of `name` below `dir` (at most `cap` bytes; `listed` is the size the entry gives it).
fn read(dir: &Path, name: &str, listed: u64, cap: u64) -> Result<Vec<u8>, CoreError> {
    let file = open(dir, name, listed.min(cap))?;
    let mut bytes = Vec::new();
    file.take(listed.min(cap))
        .read_to_end(&mut bytes)
        .map_err(|_| CoreError::new("IO_ERROR", format!("{name} cannot be read")))?;
    Ok(bytes)
}
