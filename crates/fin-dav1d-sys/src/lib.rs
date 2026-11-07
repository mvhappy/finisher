//! dav1d, VideoLAN's AV1 decoder (sub-project 6 spec §6.4), built from the pinned submodule
//! `third_party/dav1d` by `build.rs` with the `cc` crate (no meson, no ninja; nasm for the
//! x86_64 assembly), and the declarations of `csrc/dav1d_shim.c`, which owns every dav1d
//! struct so Rust mirrors none of their layouts. A crate of its own with no features, so the
//! C build runs once per profile whatever features `fin-codecs` has. The safe wrappers are
//! `fin-codecs`' `av1/dav1d_ffi.rs`.

use std::ffi::{c_char, c_void};

/// The shim's decoder handle.
#[repr(C)]
pub struct FinDav1d {
    _opaque: [u8; 0],
}

/// One decoded picture; `raw` goes back to [`fin_dav1d_picture_release`].
#[repr(C)]
pub struct FinDav1dPicture {
    pub data: [*const u8; 3],
    /// Luma, then both chroma planes, in bytes.
    pub stride: [i64; 2],
    pub width: i32,
    pub height: i32,
    /// `LAYOUT_*`.
    pub layout: i32,
    pub bpc: i32,
    pub pts: i64,
    pub has_colour: i32,
    pub primaries: i32,
    pub transfer: i32,
    pub matrix: i32,
    pub full_range: i32,
    /// `CHR_*`.
    pub chroma_position: i32,
    pub raw: *mut c_void,
}

/// What `av1C` needs from a sequence header OBU.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct FinDav1dSequence {
    pub profile: u8,
    pub major_level: u8,
    pub minor_level: u8,
    pub tier: u8,
    /// 0, 1, 2: 8, 10, 12 bits.
    pub hbd: u8,
    pub layout: u8,
    pub chroma_position: u8,
}

/// The shim's "call the other one first" result (dav1d's `-EAGAIN`, which differs by OS).
pub const AGAIN: i32 = 1;

/// `Dav1dPixelLayout` (the shim asserts these values).
pub const LAYOUT_I400: i32 = 0;
pub const LAYOUT_I420: i32 = 1;
pub const LAYOUT_I422: i32 = 2;
pub const LAYOUT_I444: i32 = 3;

/// `Dav1dChromaSamplePosition` (the shim asserts these values).
pub const CHR_UNKNOWN: i32 = 0;
pub const CHR_VERTICAL: i32 = 1;
pub const CHR_COLOCATED: i32 = 2;

unsafe extern "C" {
    pub fn fin_dav1d_version() -> *const c_char;
    pub fn fin_dav1d_open(
        threads: i32,
        frame_delay: i32,
        max_pixels: u32,
        out: *mut *mut FinDav1d,
    ) -> i32;
    pub fn fin_dav1d_frame_delay(threads: i32, frame_delay: i32) -> i32;
    pub fn fin_dav1d_send(d: *mut FinDav1d, bytes: *const u8, len: usize, pts: i64) -> i32;
    pub fn fin_dav1d_get_picture(d: *mut FinDav1d, out: *mut FinDav1dPicture) -> i32;
    pub fn fin_dav1d_picture_release(p: *mut FinDav1dPicture);
    pub fn fin_dav1d_flush(d: *mut FinDav1d);
    pub fn fin_dav1d_close(d: *mut FinDav1d);
    pub fn fin_dav1d_parse_sequence_header(
        bytes: *const u8,
        len: usize,
        out: *mut FinDav1dSequence,
    ) -> i32;
}
