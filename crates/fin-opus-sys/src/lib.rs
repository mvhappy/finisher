//! libopus 1.6.1 (float), the Opus reference codec (6C plan Decision 11), built from the pinned
//! submodule `third_party/opus` by `build.rs` with the `cc` crate (no meson, cmake, Python or
//! assembler; no `dnn/`, DRED, OSCE or intrinsics), and the declarations of
//! `shim/fin_opus_shim.c`, which owns every `OpusMSEncoder`/`OpusMSDecoder` so Rust mirrors
//! no libopus struct and calls no variadic `_ctl`. A crate of its own with no features, so the
//! C build runs once per profile. The safe wrappers are `fin-codecs`' `audio/opus.rs`.

use std::ffi::c_char;

#[cfg(test)]
mod version;

/// The shim's encoder handle.
#[repr(C)]
pub struct FinOpusEnc {
    _opaque: [u8; 0],
}

/// The shim's decoder handle.
#[repr(C)]
pub struct FinOpusDec {
    _opaque: [u8; 0],
}

/// What an encoder chose (`FinOpusEncInfo`).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FinOpusEncInfo {
    pub streams: i32,
    pub coupled: i32,
    /// Samples at 48 kHz: the stream's pre-skip.
    pub lookahead: i32,
    /// One entry per channel (the first `channels`).
    pub mapping: [u8; 8],
}

/// `OPUS_OK`.
pub const OK: i32 = 0;
/// `OPUS_BAD_ARG`.
pub const BAD_ARG: i32 = -1;
/// `OPUS_BUFFER_TOO_SMALL`.
pub const BUFFER_TOO_SMALL: i32 = -2;
/// `OPUS_INVALID_PACKET`.
pub const INVALID_PACKET: i32 = -4;

unsafe extern "C" {
    pub fn fin_opus_version() -> *const c_char;
    pub fn fin_opus_strerror(error: i32) -> *const c_char;
    pub fn fin_opus_enc_open(
        channels: i32,
        bitrate: i32,
        out: *mut *mut FinOpusEnc,
        info: *mut FinOpusEncInfo,
    ) -> i32;
    pub fn fin_opus_enc_encode(
        e: *mut FinOpusEnc,
        pcm: *const f32,
        frame_size: i32,
        out: *mut u8,
        max: i32,
    ) -> i32;
    pub fn fin_opus_enc_close(e: *mut FinOpusEnc);
    pub fn fin_opus_dec_open(
        channels: i32,
        streams: i32,
        coupled: i32,
        mapping: *const u8,
        gain: i32,
        out: *mut *mut FinOpusDec,
    ) -> i32;
    pub fn fin_opus_dec_decode(
        d: *mut FinOpusDec,
        data: *const u8,
        len: i32,
        pcm: *mut f32,
        max_frames: i32,
    ) -> i32;
    pub fn fin_opus_dec_reset(d: *mut FinOpusDec) -> i32;
    pub fn fin_opus_dec_close(d: *mut FinOpusDec);
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    #[test]
    fn the_linked_library_is_1_6_1() {
        // SAFETY: libopus returns a static NUL-terminated string.
        let v = unsafe { CStr::from_ptr(super::fin_opus_version()) };
        assert_eq!(v.to_str().unwrap(), "libopus 1.6.1");
    }
}
