//! vo-aacenc 0.1.3, VisualOn's AAC-LC encoder (6C plan Decision 12), built from the pinned
//! submodule `third_party/vo-aacenc` by `build.rs` with the `cc` crate (no autotools; C paths
//! only, no ARM assembly), and the declarations of `shim/fin_aacenc_shim.c`, which owns the
//! codec handle so Rust mirrors no vo-aacenc struct or function table.
//!
//! **Linux only** (Decision 9): on every other target the crate is empty and `build.rs`
//! compiles nothing. A crate of its own with no features, so the C build runs once per profile.
//! The safe wrapper is `fin-codecs`' `audio/aac_linux.rs`.

#[cfg(test)]
mod version;

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::c_char;

    /// The shim's encoder handle.
    #[repr(C)]
    pub struct FinAacEnc {
        _opaque: [u8; 0],
    }

    /// The samples (per channel) one access unit carries (`FIN_AACENC_FRAME`).
    pub const FRAME: usize = 1024;

    /// `VO_ERR_NONE`.
    pub const OK: u32 = 0;
    /// `VO_ERR_OUTOF_MEMORY`.
    pub const OUT_OF_MEMORY: u32 = 0x8000_0002;
    /// `VO_ERR_INVALID_ARG`.
    pub const INVALID_ARG: u32 = 0x8000_0004;
    /// `VO_ERR_OUTPUT_BUFFER_SMALL`.
    pub const OUTPUT_BUFFER_SMALL: u32 = 0x8000_0006;
    /// `VO_ERR_AUDIO_UNSCHANNEL`.
    pub const UNSUPPORTED_CHANNELS: u32 = 0x8200_0001;
    /// `VO_ERR_AUDIO_UNSSAMPLERATE`.
    pub const UNSUPPORTED_RATE: u32 = 0x8200_0002;
    /// `VO_ERR_AUDIO_UNSFEATURE`.
    pub const UNSUPPORTED_FEATURE: u32 = 0x8200_0003;

    unsafe extern "C" {
        pub fn fin_aacenc_version() -> *const c_char;
        pub fn fin_aacenc_open(
            rate: i32,
            channels: i32,
            bitrate: i32,
            out: *mut *mut FinAacEnc,
        ) -> u32;
        pub fn fin_aacenc_encode(
            e: *mut FinAacEnc,
            pcm: *const i16,
            out: *mut u8,
            max: i32,
            written: *mut i32,
        ) -> u32;
        pub fn fin_aacenc_close(e: *mut FinAacEnc);
    }

    #[cfg(test)]
    mod tests {
        use std::ffi::CStr;

        #[test]
        fn the_linked_library_is_0_1_3() {
            // SAFETY: the shim returns a static NUL-terminated string.
            let v = unsafe { CStr::from_ptr(super::fin_aacenc_version()) };
            assert_eq!(v.to_str().unwrap(), "vo-aacenc 0.1.3");
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::*;
