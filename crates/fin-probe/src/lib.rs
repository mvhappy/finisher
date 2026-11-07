//! File probe and fast file fingerprint used to register assets. The kind comes from the
//! file's first bytes, never its name: MP4/MOV/M4A and WAV (`media`), PNG/JPEG/WebP
//! (`image`), SVG (`svg`), dotLottie (`lottieTemplate`), TTF/OTF/TTC (`font`) and `.cube`
//! (`lut`).

use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

#[cfg(feature = "fixtures")]
pub mod fixtures;
mod font;
mod image;
mod lottie;
mod lut;
mod svg;
mod wav;

use fin_core::{
    AssetKind, AudioInfo, CoreError, FontFile, ImageInfo, LottieInfo, LutInfo, Rational, VideoInfo,
};
use mp4::{Mp4Reader, Mp4Track, TrackType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

const SAMPLE: u64 = 64 * 1024;
/// Files smaller than this are hashed whole instead of as head/middle/tail samples.
const WHOLE_FILE_LIMIT: u64 = 3 * SAMPLE;

/// What the server needs to register an asset: its kind and that kind's fields (the
/// `asset.add` fields, plan 4C Task 7). Its serde form is what `asset.probe` answers
/// (`@finisher/core-types` exports it as `Probe`); absent fields are omitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub kind: AssetKind,
    /// Seconds: `media` and `lottieTemplate` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<Rational>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioInfo>,
    /// `image` and `svg`: the pixel size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lottie: Option<LottieInfo>,
    /// A font's whole-file SHA-256, format and faces (Decision 37).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<FontFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lut: Option<LutInfo>,
    pub fingerprint: String,
    pub size: u64,
}

impl Probe {
    /// A `kind` probe with no fields yet (no fingerprint, size 0).
    pub(crate) fn of(kind: AssetKind) -> Probe {
        Probe {
            kind,
            duration: None,
            video: None,
            audio: None,
            image: None,
            lottie: None,
            font: None,
            lut: None,
            fingerprint: String::new(),
            size: 0,
        }
    }
}

/// Files read whole (SVG, `.cube`, fonts) are refused above this.
pub(crate) const READ_LIMIT: u64 = 256 * 1024 * 1024;

/// The widest and tallest image or SVG a probe accepts (`asset.add` checks the same bound).
pub(crate) const MAX_DIMENSION: u32 = fin_core::MAX_IMAGE_DIMENSION;

/// The reason for an image over [`MAX_DIMENSION`].
pub(crate) fn too_large(width: u32, height: u32) -> String {
    format!("the image is {width}×{height} px; at most {MAX_DIMENSION} px a side")
}

/// Absolute form of `path`: canonical when the file exists, otherwise just absolutised.
fn absolute(path: &Path) -> PathBuf {
    dunce::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn unsupported(path: &Path, reason: &str) -> CoreError {
    CoreError::new(
        "UNSUPPORTED_CONTAINER",
        format!("cannot read the file: {reason}"),
    )
    .with_details(json!({ "path": absolute(path).to_string_lossy(), "reason": reason }))
    .with_fix(
        "use an MP4, MOV, M4A or WAV file, a PNG, JPEG, WebP or SVG image, a dotLottie \
         template, a TTF, OTF or TTC font, or a 3D .cube LUT",
    )
}

/// What a file's first bytes say it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sniffed {
    Mp4,
    Image,
    Wav,
    Zip,
    Font(fin_core::FontFormat),
    Woff,
    /// Not binary: SVG or `.cube`, judged from the text.
    Text,
    Unknown,
}

fn sniff(head: &[u8]) -> Sniffed {
    use fin_core::FontFormat::{Otf, Ttc, Ttf};
    let at = |i: usize, magic: &[u8]| head.get(i..i + magic.len()) == Some(magic);
    let mp4_box = |b: &[u8]| matches!(b, b"ftyp" | b"moov" | b"mdat" | b"wide" | b"free" | b"skip");
    if head.get(4..8).is_some_and(mp4_box) {
        Sniffed::Mp4
    } else if at(0, b"\x89PNG\r\n\x1a\n")
        || at(0, &[0xFF, 0xD8, 0xFF])
        || at(0, b"RIFF") && at(8, b"WEBP")
    {
        Sniffed::Image
    } else if at(0, b"RIFF") && at(8, b"WAVE") {
        Sniffed::Wav
    } else if at(0, b"PK\x03\x04") {
        Sniffed::Zip
    } else if at(0, b"ttcf") {
        Sniffed::Font(Ttc)
    } else if at(0, b"OTTO") {
        Sniffed::Font(Otf)
    } else if at(0, &[0, 1, 0, 0]) || at(0, b"true") {
        Sniffed::Font(Ttf)
    } else if at(0, b"wOFF") || at(0, b"wOF2") {
        Sniffed::Woff
    } else if !head.is_empty()
        && !head
            .iter()
            .any(|&b| b == 0 || (b < 0x20 && !b" \t\r\n\x0c".contains(&b)))
    {
        Sniffed::Text
    } else {
        Sniffed::Unknown
    }
}

/// The whole file, refused above [`READ_LIMIT`].
pub(crate) fn read_all(file: &mut File, size: u64, path: &Path) -> Result<Vec<u8>, CoreError> {
    if size > READ_LIMIT {
        return Err(unsupported(path, "file too large"));
    }
    let mut all = Vec::with_capacity(size as usize);
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.take(READ_LIMIT).read_to_end(&mut all))
        .map_err(|e| io_error(path, &e))?;
    Ok(all)
}

/// SVG when the text's first markup is an element, else `.cube` when its first keyword is
/// one; anything else is not a kind.
fn probe_text(file: &mut File, size: u64, path: &Path) -> Result<Probe, CoreError> {
    let bytes = read_all(file, size, path)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| unsupported(path, "text is not UTF-8"))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.trim_start().starts_with('<') {
        svg::probe(text, path)
    } else if lut::is_cube(text) {
        lut::probe(text, path)
    } else {
        Err(unsupported(path, "unknown file type"))
    }
}

/// Probes a file by its first bytes (errors: `ASSET_NOT_FOUND`, `ASSET_UNREADABLE`,
/// `UNSUPPORTED_CONTAINER` with `details.reason`, also for a truncated or malformed file).
///
/// `path` is used as an OS path exactly as given: `~` is not expanded (callers such as `fin`
/// do that), and relative paths resolve against the current directory. Error `details.path`
/// is absolute (canonical when the file exists; on Windows without a `\\?\` verbatim prefix).
///
/// Fragmented MP4 is not supported in sub-project 2: a file with an `mvex` box, or with no
/// samples and zero duration, fails with `UNSUPPORTED_CONTAINER` and
/// `details.reason = "fragmented mp4"`.
pub fn probe(path: &Path) -> Result<Probe, CoreError> {
    let mut file = File::open(path).map_err(|e| io_error(path, &e))?;
    let meta = file.metadata().map_err(|e| io_error(path, &e))?;
    let size = meta.len();
    let mut head = Vec::with_capacity(16);
    (&mut file)
        .take(16)
        .read_to_end(&mut head)
        .map_err(|e| io_error(path, &e))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| io_error(path, &e))?;
    let parsed = match sniff(&head) {
        // The mp4 crate can panic on malformed boxes; treat that as an unreadable container.
        Sniffed::Mp4 => catch_unwind(AssertUnwindSafe(|| parse(&file, size, path)))
            .unwrap_or_else(|_| Err(unsupported(path, "malformed container"))),
        Sniffed::Image => image::probe(&mut file, path),
        Sniffed::Wav => wav::probe(&mut file, size, path),
        Sniffed::Zip => lottie::probe(&mut file, size, path),
        Sniffed::Font(format) => font::probe(&mut file, size, format, path),
        Sniffed::Woff => Err(unsupported(path, "WOFF and WOFF2 fonts are not supported")),
        Sniffed::Text => probe_text(&mut file, size, path),
        Sniffed::Unknown => Err(unsupported(path, "unknown file type")),
    }?;
    let fingerprint = fingerprint_file(&mut file, &meta).map_err(|e| io_error(path, &e))?;
    Ok(Probe {
        fingerprint,
        size,
        ..parsed
    })
}

/// True when some ancestor of `path` exists but is not a directory. Unix reports such a path
/// as `ENOTDIR`; Windows reports it as not found (`ERROR_PATH_NOT_FOUND`).
fn through_a_file(path: &Path) -> bool {
    path.ancestors()
        .skip(1)
        .any(|a| !a.as_os_str().is_empty() && a.metadata().is_ok_and(|m| !m.is_dir()))
}

pub(crate) fn io_error(path: &Path, e: &io::Error) -> CoreError {
    let through_file = e.kind() == io::ErrorKind::NotFound && through_a_file(path);
    let path = absolute(path).to_string_lossy().into_owned();
    if through_file {
        return CoreError::new(
            "ASSET_UNREADABLE",
            "a parent of the media path is a file, not a folder",
        )
        .with_details(json!({ "path": path, "reason": "NotADirectory" }))
        .with_fix("check the file and its parent folders exist and are readable");
    }
    if e.kind() == io::ErrorKind::NotFound {
        return CoreError::new("ASSET_NOT_FOUND", "media file does not exist")
            .with_details(json!({ "path": path }))
            .with_fix("check the path or relink the asset");
    }
    CoreError::new("ASSET_UNREADABLE", format!("cannot read media file: {e}"))
        .with_details(json!({ "path": path, "reason": format!("{:?}", e.kind()) }))
        .with_fix("check the file and its parent folders exist and are readable")
}

fn parse(file: &File, size: u64, path: &Path) -> Result<Probe, CoreError> {
    let reader = Mp4Reader::read_header(io::BufReader::new(file), size)
        .map_err(|e| unsupported(path, &e.to_string()))?;
    let mvhd = &reader.moov.mvhd;
    let no_samples = reader.tracks().values().all(|t| t.sample_count() == 0);
    if reader.moov.mvex.is_some() || (mvhd.duration == 0 && no_samples) {
        return Err(unsupported(path, "fragmented mp4"));
    }
    let duration = i64::try_from(mvhd.duration)
        .ok()
        .and_then(|d| Rational::new(d, i64::from(mvhd.timescale)).ok())
        .ok_or_else(|| unsupported(path, "invalid movie duration"))?;

    let mut ids: Vec<u32> = reader.tracks().keys().copied().collect();
    ids.sort_unstable();
    let first = |kind: TrackType| {
        ids.iter()
            .filter_map(|id| reader.tracks().get(id))
            .find(|t| t.track_type().is_ok_and(|k| k == kind))
    };
    let video = first(TrackType::Video)
        .map(|t| video_info(t, path))
        .transpose()?;
    let audio = first(TrackType::Audio).and_then(audio_info);
    if video.is_none() && audio.is_none() {
        return Err(unsupported(path, "no video or audio track"));
    }
    Ok(Probe {
        duration: Some(duration),
        video,
        audio,
        ..Probe::of(AssetKind::Media)
    })
}

/// Exact rate: `sample_count * timescale / track duration (in timescale units)`.
fn video_info(track: &Mp4Track, path: &Path) -> Result<VideoInfo, CoreError> {
    let mdhd = &track.trak.mdia.mdhd;
    let rate = i64::try_from(u64::from(track.sample_count()) * u64::from(mdhd.timescale))
        .ok()
        .zip(i64::try_from(mdhd.duration).ok())
        .filter(|(n, d)| *n > 0 && *d > 0)
        .and_then(|(n, d)| Rational::new(n, d).ok())
        .ok_or_else(|| unsupported(path, "unknown frame rate"))?;
    Ok(VideoInfo {
        width: track.width() as u32,
        height: track.height() as u32,
        rate,
    })
}

/// Sample rate is the audio track's `mdhd` timescale; `None` when channels are unknown.
fn audio_info(track: &Mp4Track) -> Option<AudioInfo> {
    let mp4a = track.trak.mdia.minf.stbl.stsd.mp4a.as_ref()?;
    (mp4a.channelcount > 0).then_some(AudioInfo {
        sample_rate: track.timescale(),
        channels: mp4a.channelcount,
    })
}

/// `"fp1:" + hex(blake3(size || mtime_ns || head || middle || tail))`, byte layout:
/// - `size`: file length, `u64` little-endian (8 bytes);
/// - `mtime_ns`: modification time as nanoseconds since the Unix epoch, `i64` little-endian
///   (8 bytes), saturating to `i64::MIN/MAX`, 0 if unavailable;
/// - `head`: bytes `[0, 64 KiB)`; `middle`: 64 KiB starting at `size/2 - 32 KiB`; `tail`: the
///   last 64 KiB.
///
/// Files under 192 KiB are hashed whole (once) in place of head/middle/tail.
pub fn fingerprint(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let meta = file.metadata()?;
    fingerprint_file(&mut file, &meta)
}

fn mtime_ns(meta: &Metadata) -> i64 {
    let ns = |d: std::time::Duration| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX);
    match meta
        .modified()
        .map(|t| t.duration_since(std::time::UNIX_EPOCH))
    {
        Ok(Ok(d)) => ns(d),
        Ok(Err(e)) => ns(e.duration()).saturating_neg(),
        Err(_) => 0,
    }
}

fn fingerprint_file(file: &mut File, meta: &Metadata) -> io::Result<String> {
    let size = meta.len();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());
    hasher.update(&mtime_ns(meta).to_le_bytes());
    file.seek(SeekFrom::Start(0))?;
    if size < WHOLE_FILE_LIMIT {
        let mut all = Vec::with_capacity(size as usize);
        file.take(size).read_to_end(&mut all)?;
        hasher.update(&all);
    } else {
        let mut buf = vec![0u8; SAMPLE as usize];
        for offset in [0, size / 2 - SAMPLE / 2, size - SAMPLE] {
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut buf)?;
            hasher.update(&buf);
        }
    }
    Ok(format!("fp1:{}", hasher.finalize().to_hex()))
}
