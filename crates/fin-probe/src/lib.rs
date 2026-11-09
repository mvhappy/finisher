//! File probe and fast file fingerprint used to register assets. The kind comes from the
//! file's first bytes, never its name: MP4/MOV/M4A, WebM/MKV and WAV (`media`), PNG/JPEG/WebP
//! (`image`), SVG (`svg`), dotLottie (`lottieTemplate`), TTF/OTF/TTC (`font`) and `.cube`
//! (`lut`).
//!
//! Every entry reads through one handle: [`probe_reader`] over any `Read + Seek` (the
//! fingerprint first, so a file that fails to probe keeps it), [`probe_with`] over a file the
//! caller opens, and [`probe`] for a plain path. **A library file is opened only by
//! `fin_library::open_for_read`** (plan 5D Decision 20): its callers use [`probe_with`] or
//! [`probe_reader`], never [`probe`] or [`fingerprint`].

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
mod open;
mod svg;
mod wav;

use fin_container::{Demuxer, Limits};
use fin_core::{
    AssetKind, AudioInfo, CoreError, DisplaySize, FontFile, ImageInfo, LottieInfo, LutInfo,
    Rational, VideoInfo,
};
use fin_media::MediaInfo;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The version of what [`probe_reader`] puts in a [`Probe`] (plan 5D Decision 35). The library
/// catalog stores it per file; a higher number re-queues only the probe stage. **Bump it
/// whenever probe output changes** for the same bytes: a new kind or field, a changed
/// decision (sub-project 6 does, from 6A-2 on). 2: WebM/Matroska probes as media (6B-1
/// Task 12; refused before).
pub const PROBE_VERSION: u32 = 2;

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
    /// MP4/MOV, WebM/Matroska: what the file is (sub-project 6 spec §6.3): display size and
    /// rotation, colour, HDR, VFR, edit lists. In the `asset.probe` answer only: an
    /// `asset.add` never carries it (spec §4.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "MediaInfo")]
    pub media: Option<MediaInfo>,
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
            media: None,
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
        "use an MP4, MOV, M4A, WebM, MKV or WAV file, a PNG, JPEG, WebP or SVG image, a dotLottie \
         template, a TTF, OTF or TTC font, or a 3D .cube LUT",
    )
}

/// What a file's first bytes say it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sniffed {
    /// MP4/MOV, or WebM/Matroska (the EBML magic): both read by `fin-container`.
    Media,
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
    if head.get(4..8).is_some_and(mp4_box) || at(0, &[0x1A, 0x45, 0xDF, 0xA3]) {
        Sniffed::Media
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
pub(crate) fn read_all<R: Read + Seek>(
    file: &mut R,
    size: u64,
    path: &Path,
) -> Result<Vec<u8>, CoreError> {
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
fn probe_text<R: Read + Seek>(file: &mut R, size: u64, path: &Path) -> Result<Probe, CoreError> {
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

/// What a reader's identity is: its length and modification time, the two the fingerprint
/// folds in. For a file, [`FileMeta::of`] its metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMeta {
    pub size: u64,
    /// Nanoseconds since the Unix epoch (negative before it); 0 when unavailable.
    pub mtime_ns: i64,
}

impl FileMeta {
    pub fn of(meta: &Metadata) -> FileMeta {
        FileMeta {
            size: meta.len(),
            mtime_ns: mtime_ns(meta),
        }
    }
}

/// [`probe_reader`]'s answer: the fingerprint even when the probe failed (`None` only when
/// the reader itself failed under it), and the probe (whose own `fingerprint` is the same).
#[derive(Debug)]
pub struct ProbeOutcome {
    pub fingerprint: Option<String>,
    pub probe: Result<Probe, CoreError>,
}

/// Probes a file by its first bytes (errors: `ASSET_NOT_FOUND`, `ASSET_UNREADABLE`,
/// `UNSUPPORTED_CONTAINER` with `details.reason`, also for a truncated or malformed file).
///
/// `path` is used as an OS path exactly as given: `~` is not expanded (callers such as `fin`
/// do that), and relative paths resolve against the current directory. Error `details.path`
/// is absolute (canonical when the file exists; on Windows without a `\\?\` verbatim prefix).
///
/// MP4/MOV (fragmented too) and WebM/Matroska are read by `fin-container`; a broken or
/// hostile file is `MEDIA_CORRUPT`, `MEDIA_LIMIT`, `UNSUPPORTED_CONTAINER` or
/// `UNSUPPORTED_TRANSFORM` with `details.path`.
///
/// For a project file. A file under a library root goes through [`probe_with`]. The file is
/// opened without blocking and must be a regular file (a folder, FIFO, socket or device is
/// `ASSET_UNREADABLE` with `details.reason = "NotAFile"`); symlinks are followed.
pub fn probe(path: &Path) -> Result<Probe, CoreError> {
    let (mut file, meta) = open::open(path).map_err(|r| r.into_core(path, None))?;
    probe_reader(&mut file, FileMeta::of(&meta), path).probe
}

/// [`probe`] for a file that must be inside `root`: `path` is `root` joined with plain
/// components (no `..`), each reached from the root's open folder without following a
/// symlink, decided on the open handle so that a symlink swapped in meanwhile cannot redirect
/// it (Linux `openat2` with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`, else one `O_NOFOLLOW`
/// open per component; Windows: no reparse point, and the handle's final path inside the
/// root's). Anything else is `PATH_NOT_ALLOWED` with `details.path` (as given, never
/// resolved), `details.root` and `details.reason` (`outsideRoot` or `symlink`).
///
/// `root` itself is opened as given: callers pass it canonical.
pub fn probe_beneath(root: &Path, path: &Path) -> Result<Probe, CoreError> {
    let (mut file, meta) =
        open::open_beneath(root, path).map_err(|r| r.into_core(path, Some(root)))?;
    probe_reader(&mut file, FileMeta::of(&meta), path).probe
}

/// [`probe`] over the handle `open` returns for `path`, called once: the metadata, the
/// fingerprint and the probe all come from that one handle. `path` names the file in errors
/// only; it is never opened here. This is how a library file is probed: the opener is
/// `fin_library::open_for_read` with the catalogued row (plan 5D Decisions 19, 20).
pub fn probe_with(
    path: &Path,
    open: impl FnOnce(&Path) -> io::Result<File>,
) -> Result<Probe, CoreError> {
    let mut file = open(path).map_err(|e| io_error(path, &e))?;
    let meta = file.metadata().map_err(|e| io_error(path, &e))?;
    probe_reader(&mut file, FileMeta::of(&meta), path).probe
}

/// [`probe`] over any reader of `meta.size` bytes. The fingerprint is computed first and
/// survives a failed probe (a probe failure never loses it); both read through `r` alone.
/// Equal bytes and `meta` give the same [`Probe`] as [`probe`] of a file holding them.
pub fn probe_reader<R: Read + Seek>(mut r: R, meta: FileMeta, path: &Path) -> ProbeOutcome {
    let fp = fingerprint_reader(&mut r, meta);
    let parsed = probe_kind(&mut r, meta.size, path);
    let probe = match (parsed, &fp) {
        (Err(e), _) => Err(e),
        (Ok(_), Err(e)) => Err(io_error(path, e)),
        (Ok(p), Ok(fp)) => Ok(Probe {
            fingerprint: fp.clone(),
            size: meta.size,
            ..p
        }),
    };
    ProbeOutcome {
        fingerprint: fp.ok(),
        probe,
    }
}

/// The kind and its fields, from the reader's first bytes.
fn probe_kind<R: Read + Seek>(file: &mut R, size: u64, path: &Path) -> Result<Probe, CoreError> {
    let mut head = Vec::with_capacity(16);
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.take(16).read_to_end(&mut head))
        .and_then(|_| file.seek(SeekFrom::Start(0)))
        .map_err(|e| io_error(path, &e))?;
    match sniff(&head) {
        // fin-container never panics on input (it is fuzzed); a panic is a bug, reported as
        // a corrupt file rather than taking the server down.
        Sniffed::Media => {
            catch_unwind(AssertUnwindSafe(|| parse(file, path))).unwrap_or_else(|_| {
                Err(with_path(
                    CoreError::new("MEDIA_CORRUPT", "the container could not be read"),
                    path,
                ))
            })
        }
        Sniffed::Image => image::probe(file, path),
        Sniffed::Wav => wav::probe(file, size, path),
        Sniffed::Zip => lottie::probe(file, size, path),
        Sniffed::Font(format) => font::probe(file, size, format, path),
        Sniffed::Woff => Err(unsupported(path, "WOFF and WOFF2 fonts are not supported")),
        Sniffed::Text => probe_text(file, size, path),
        Sniffed::Unknown => Err(unsupported(path, "unknown file type")),
    }
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

/// `e` with the file's absolute `details.path` (and the unsupported-container fix).
fn with_path(mut e: CoreError, path: &Path) -> CoreError {
    if !e.details.is_object() {
        e.details = json!({});
    }
    e.details["path"] = json!(absolute(path).to_string_lossy());
    // A reason code the container gave (`contentEncoding`) is kept; else the message's text.
    if e.code == "UNSUPPORTED_CONTAINER" && e.details.get("reason").is_none() {
        e.details["reason"] = json!(e.message.trim_start_matches("cannot read this container: "));
    }
    if e.code == "UNSUPPORTED_CONTAINER" {
        e.fix = unsupported(path, "").fix;
    }
    e
}

/// The model's fields keep their 4C meaning (spec §4.4): `width`/`height` the coded size, the
/// rate the average (`samples × timescale / track duration`), the audio rate the track's
/// timescale. Schema 4 adds the display geometry (`display`, `rotation`, `pixelAspect`, as
/// `MediaInfo` reads them); `media` carries the rest and is never stored.
fn parse<R: Read + Seek>(file: &mut R, path: &Path) -> Result<Probe, CoreError> {
    let d = Demuxer::open(io::BufReader::new(file), &Limits::default())
        .map_err(|e| with_path(e, path))?;
    let info = d.info().clone();
    let video = match (d.video(), &info.video) {
        (Some(t), Some(v)) => {
            let total: u64 = (0..t.sample_count())
                .filter_map(|i| t.sample(i))
                .map(|s| u64::from(s.duration))
                .sum();
            let rate = i64::try_from(t.sample_count() as u64 * u64::from(t.table.timescale()))
                .ok()
                .zip(i64::try_from(total).ok())
                .filter(|(n, d)| *n > 0 && *d > 0)
                .and_then(|(n, d)| Rational::new(n, d).ok())
                .ok_or_else(|| unsupported(path, "unknown frame rate"))?;
            Some(VideoInfo {
                width: v.coded.width,
                height: v.coded.height,
                rate,
                display: Some(DisplaySize {
                    width: v.display.width,
                    height: v.display.height,
                }),
                rotation: Some(v.rotation),
                pixel_aspect: Some(v.pixel_aspect),
            })
        }
        _ => None,
    };
    let audio = match (d.audio(), &info.audio) {
        (Some(t), Some(a)) if a.channels > 0 => Some(AudioInfo {
            sample_rate: t.timescale,
            channels: a.channels,
        }),
        _ => None,
    };
    if video.is_none() && audio.is_none() {
        return Err(unsupported(path, "no video or audio track"));
    }
    if !info.duration.is_positive() {
        return Err(unsupported(path, "invalid movie duration"));
    }
    Ok(Probe {
        duration: Some(info.duration),
        video,
        audio,
        media: Some(info),
        ..Probe::of(AssetKind::Media)
    })
}

/// `"fp1:" + hex(blake3(size || mtime_ns || head || middle || tail))`, byte layout:
/// - `size`: file length, `u64` little-endian (8 bytes);
/// - `mtime_ns`: modification time as nanoseconds since the Unix epoch, `i64` little-endian
///   (8 bytes), saturating to `i64::MIN/MAX`, 0 if unavailable;
/// - `head`: bytes `[0, 64 KiB)`; `middle`: 64 KiB starting at `size/2 - 32 KiB`; `tail`: the
///   last 64 KiB.
///
/// Files under 192 KiB are hashed whole (once) in place of head/middle/tail. Opened as by
/// [`probe`]: without blocking, and only a regular file (else `InvalidInput`).
pub fn fingerprint(path: &Path) -> io::Result<String> {
    let (mut file, meta) = open::open(path)?;
    fingerprint_reader(&mut file, FileMeta::of(&meta))
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

/// [`fingerprint`] of a reader of `meta.size` bytes: the same layout, read through `r`.
pub fn fingerprint_reader<R: Read + Seek>(mut r: R, meta: FileMeta) -> io::Result<String> {
    let size = meta.size;
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());
    hasher.update(&meta.mtime_ns.to_le_bytes());
    r.seek(SeekFrom::Start(0))?;
    if size < WHOLE_FILE_LIMIT {
        let mut all = Vec::with_capacity(size as usize);
        (&mut r).take(size).read_to_end(&mut all)?;
        hasher.update(&all);
    } else {
        let mut buf = vec![0u8; SAMPLE as usize];
        for offset in [0, size / 2 - SAMPLE / 2, size - SAMPLE] {
            r.seek(SeekFrom::Start(offset))?;
            r.read_exact(&mut buf)?;
            hasher.update(&buf);
        }
    }
    Ok(format!("fp1:{}", hasher.finalize().to_hex()))
}
