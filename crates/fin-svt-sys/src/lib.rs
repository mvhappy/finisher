//! SVT-AV1's encoder (sub-project 6 spec §6.4), built from the pinned submodule
//! `third_party/svt-av1` by `build.rs`, and the declarations of `csrc/svt_shim.c`, which owns
//! SVT-AV1's configuration struct so Rust mirrors none of its layout. A crate of its own with
//! no features, so the cmake build runs once per profile whatever features `fin-codecs` has.
//! The safe wrappers are `fin-codecs`' `av1/svt_ffi.rs`.

use std::ffi::{c_char, c_void};

/// The shim's encoder handle.
#[repr(C)]
pub struct FinSvt {
    _opaque: [u8; 0],
}

/// One output packet; `raw` goes back to [`fin_svt_release`].
#[repr(C)]
pub struct FinSvtPacket {
    pub data: *const u8,
    pub len: u32,
    pub pts: i64,
    pub dts: i64,
    pub flags: u32,
    pub pic_type: u32,
    pub raw: *mut c_void,
}

unsafe extern "C" {
    pub fn fin_svt_new(err: *mut i32) -> *mut FinSvt;
    pub fn fin_svt_param(s: *mut FinSvt, name: *const c_char, value: *const c_char) -> i32;
    pub fn fin_svt_init(s: *mut FinSvt) -> i32;
    pub fn fin_svt_header(s: *mut FinSvt, out: *mut u8, cap: u32, len: *mut u32) -> i32;
    #[allow(clippy::too_many_arguments)]
    pub fn fin_svt_send(
        s: *mut FinSvt,
        y: *const u8,
        u: *const u8,
        v: *const u8,
        y_stride: u32,
        uv_stride: u32,
        luma_bytes: u32,
        pts: i64,
        force_key: i32,
    ) -> i32;
    pub fn fin_svt_packet(s: *mut FinSvt, done: u8, out: *mut FinSvtPacket) -> i32;
    pub fn fin_svt_release(p: *mut FinSvtPacket);
    pub fn fin_svt_free(s: *mut FinSvt);
}

/// `EB_BUFFERFLAG_EOS`.
pub const FLAG_EOS: u32 = 1;
/// `EB_AV1_KEY_PICTURE`.
pub const KEY_PICTURE: u32 = 3;
