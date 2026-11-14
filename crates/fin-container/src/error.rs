//! The container error codes (spec §17): `MEDIA_CORRUPT {at, box}`, `MEDIA_LIMIT {limit,
//! value}`, `UNSUPPORTED_CONTAINER {format, reason}`, `UNSUPPORTED_TRANSFORM`, and `IO_ERROR` for the
//! reader or writer failing.

use fin_core::CoreError;
use serde_json::json;

/// A structurally broken file: `at` is the byte offset of the box, `kind` its type.
pub fn corrupt(message: impl Into<String>, at: u64, kind: &str) -> CoreError {
    CoreError::new("MEDIA_CORRUPT", message).with_details(json!({ "at": at, "box": kind }))
}

/// A file past one of `fin-container`'s limits.
pub fn limit(name: &str, value: u64, max: u64) -> CoreError {
    CoreError::new(
        "MEDIA_LIMIT",
        format!("the file exceeds the container limit {name} ({value} > {max})"),
    )
    .with_details(json!({ "limit": name, "value": value, "max": max }))
}

pub fn unsupported_container(format: &str, why: &str) -> CoreError {
    CoreError::new(
        "UNSUPPORTED_CONTAINER",
        format!("cannot read this container: {why}"),
    )
    .with_details(json!({ "format": format, "reason": why }))
    .with_fix("convert the file to MP4 or MOV, or install the codec plugin")
}

/// A Matroska track whose frames are compressed or encrypted (`ContentEncoding`):
/// `UNSUPPORTED_CONTAINER {format, reason: contentEncoding}`.
pub fn content_encoding(format: &str) -> CoreError {
    CoreError::new(
        "UNSUPPORTED_CONTAINER",
        "cannot read this container: the track's frames are compressed or encrypted \
         (ContentEncoding)",
    )
    .with_details(json!({ "format": format, "reason": "contentEncoding" }))
    .with_fix("remux the file without header compression or encryption, or convert it to MP4")
}

/// A Matroska video track with a projection other than flat, or a pose.
pub fn unsupported_projection(projection_type: u64) -> CoreError {
    CoreError::new(
        "UNSUPPORTED_TRANSFORM",
        "the video track has a 360° projection or a pose",
    )
    .with_details(json!({ "projectionType": projection_type }))
    .with_fix("re-export the clip as flat video without a projection")
}

pub fn unsupported_transform(matrix: &[i32; 9]) -> CoreError {
    CoreError::new(
        "UNSUPPORTED_TRANSFORM",
        "the track matrix is not a rotation by a multiple of 90°",
    )
    .with_details(json!({ "matrix": matrix }))
    .with_fix("re-export the clip without a custom transform")
}

pub fn io(e: &std::io::Error) -> CoreError {
    CoreError::new("IO_ERROR", format!("media I/O failed: {e}"))
        .with_details(json!({ "reason": format!("{:?}", e.kind()) }))
}

/// A caller error in the muxer's input (`INVALID_RANGE`).
pub fn invalid(message: impl Into<String>, details: serde_json::Value) -> CoreError {
    CoreError::new("INVALID_RANGE", message).with_details(details)
}

/// `mp4-atom` failed to encode or decode a box whose size `fin-container` already checked.
pub fn atom(e: mp4_atom::Error, at: u64, kind: &str) -> CoreError {
    corrupt(format!("malformed {kind} box: {e}"), at, kind)
}
