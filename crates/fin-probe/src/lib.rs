//! Container probe (MP4/MOV) and fast file fingerprint used to register media assets.

use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

#[cfg(feature = "fixtures")]
pub mod fixtures;

use fin_core::{AudioInfo, CoreError, Rational, VideoInfo};
use mp4::{Mp4Reader, Mp4Track, TrackType};
use serde_json::json;

const SAMPLE: u64 = 64 * 1024;
/// Files smaller than this are hashed whole instead of as head/middle/tail samples.
const WHOLE_FILE_LIMIT: u64 = 3 * SAMPLE;

/// What the server needs to register a media asset.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub duration: Rational,
    pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>,
    pub fingerprint: String,
    pub size: u64,
}

/// Absolute form of `path`: canonical when the file exists, otherwise just absolutised.
fn absolute(path: &Path) -> PathBuf {
    dunce::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn unsupported(path: &Path, reason: &str) -> CoreError {
    CoreError::new(
        "UNSUPPORTED_CONTAINER",
        format!("cannot read media container: {reason}"),
    )
    .with_details(json!({ "path": absolute(path).to_string_lossy(), "reason": reason }))
    .with_fix("use an MP4 or MOV file")
}

/// Probes an MP4/MOV file (errors: `ASSET_NOT_FOUND`, `ASSET_UNREADABLE`,
/// `UNSUPPORTED_CONTAINER`).
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
    // The mp4 crate can panic on malformed boxes; treat that as an unreadable container.
    let parsed = catch_unwind(AssertUnwindSafe(|| parse(&file, size, path)))
        .unwrap_or_else(|_| Err(unsupported(path, "malformed container")))?;
    let fingerprint = fingerprint_file(&mut file, &meta).map_err(|e| io_error(path, &e))?;
    Ok(Probe {
        fingerprint,
        size,
        ..parsed
    })
}

fn io_error(path: &Path, e: &io::Error) -> CoreError {
    let path = absolute(path).to_string_lossy().into_owned();
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
        duration,
        video,
        audio,
        fingerprint: String::new(),
        size,
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
