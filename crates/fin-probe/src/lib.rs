//! Container probe (MP4/MOV) and fast file fingerprint used to register media assets.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

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
    fs::canonicalize(path)
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

/// Probes an MP4/MOV file. Errors: `ASSET_NOT_FOUND`, `UNSUPPORTED_CONTAINER`.
pub fn probe(path: &Path) -> Result<Probe, CoreError> {
    let file = File::open(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => CoreError::new("ASSET_NOT_FOUND", "media file does not exist")
            .with_details(json!({ "path": absolute(path).to_string_lossy() }))
            .with_fix("check the path or relink the asset"),
        _ => unsupported(path, &format!("cannot open file: {e}")),
    })?;
    let size = file
        .metadata()
        .map_err(|e| unsupported(path, &e.to_string()))?
        .len();
    // The mp4 crate can panic on malformed boxes; treat that as an unreadable container.
    let parsed = catch_unwind(AssertUnwindSafe(|| parse(file, size, path)))
        .unwrap_or_else(|_| Err(unsupported(path, "malformed container")))?;
    let fingerprint = fingerprint(path).map_err(|e| unsupported(path, &e.to_string()))?;
    Ok(Probe {
        fingerprint,
        size,
        ..parsed
    })
}

fn parse(file: File, size: u64, path: &Path) -> Result<Probe, CoreError> {
    let reader = Mp4Reader::read_header(io::BufReader::new(file), size)
        .map_err(|e| unsupported(path, &e.to_string()))?;
    let mvhd = &reader.moov.mvhd;
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
    let audio = first(TrackType::Audio).map(audio_info);
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

fn audio_info(track: &Mp4Track) -> AudioInfo {
    match &track.trak.mdia.minf.stbl.stsd.mp4a {
        Some(a) => AudioInfo {
            sample_rate: u32::from(a.samplerate.value()),
            channels: a.channelcount,
        },
        None => AudioInfo {
            sample_rate: track.timescale(),
            channels: 0,
        },
    }
}

/// `"fp1:" + hex(blake3(size_le || mtime_ns_le || head || middle || tail))`, where `size` is a
/// `u64`, `mtime_ns` a `u128` (ns since the Unix epoch, 0 if unavailable), both little-endian,
/// and head/middle/tail are 64 KiB samples. Files under 192 KiB are hashed whole (once) in
/// place of the three samples.
pub fn fingerprint(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let meta = file.metadata()?;
    let size = meta.len();
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());
    hasher.update(&mtime_ns.to_le_bytes());
    let mut buf = vec![0u8; SAMPLE as usize];
    let mut feed = |file: &mut File, offset: u64, len: u64| -> io::Result<()> {
        file.seek(SeekFrom::Start(offset))?;
        let chunk = &mut buf[..len as usize];
        file.read_exact(chunk)?;
        hasher.update(chunk);
        Ok(())
    };
    if size < WHOLE_FILE_LIMIT {
        let mut all = Vec::with_capacity(size as usize);
        file.read_to_end(&mut all)?;
        hasher.update(&all);
    } else {
        feed(&mut file, 0, SAMPLE)?;
        feed(&mut file, size / 2 - SAMPLE / 2, SAMPLE)?;
        feed(&mut file, size - SAMPLE, SAMPLE)?;
    }
    Ok(format!("fp1:{}", hasher.finalize().to_hex()))
}
