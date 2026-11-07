//! The SVT-AV1 encoder calls, through `csrc/svt_shim.c` (which owns SVT-AV1's configuration
//! struct), each wrapped once. This module and `rav1d_ffi.rs` are the only places with
//! `unsafe` in `fin-codecs`.

use std::ffi::{CString, c_void};

use fin_media::Size;

#[repr(C)]
struct FinSvt {
    _opaque: [u8; 0],
}

#[repr(C)]
struct FinSvtPacket {
    data: *const u8,
    len: u32,
    pts: i64,
    dts: i64,
    flags: u32,
    pic_type: u32,
    raw: *mut c_void,
}

unsafe extern "C" {
    fn fin_svt_new(err: *mut i32) -> *mut FinSvt;
    fn fin_svt_param(s: *mut FinSvt, name: *const i8, value: *const i8) -> i32;
    fn fin_svt_init(s: *mut FinSvt) -> i32;
    fn fin_svt_header(s: *mut FinSvt, out: *mut u8, cap: u32, len: *mut u32) -> i32;
    fn fin_svt_send(
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
    fn fin_svt_packet(s: *mut FinSvt, done: u8, out: *mut FinSvtPacket) -> i32;
    fn fin_svt_release(p: *mut FinSvtPacket);
    fn fin_svt_free(s: *mut FinSvt);
}

/// `EB_BUFFERFLAG_EOS`.
pub(crate) const FLAG_EOS: u32 = 1;
/// `EB_AV1_KEY_PICTURE`.
pub(crate) const KEY_PICTURE: u32 = 3;

/// One packet copied out of SVT-AV1.
pub(crate) struct Packet {
    pub data: Vec<u8>,
    pub pts: i64,
    pub dts: i64,
    pub flags: u32,
    pub pic_type: u32,
}

/// An SVT-AV1 encoder handle; freed on drop.
pub(crate) struct Encoder {
    s: *mut FinSvt,
}

// SAFETY: the handle is used from one thread at a time (`&mut self`); SVT-AV1's own threads
// are internal to it.
unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new() -> Result<Encoder, i32> {
        let mut err = 0;
        // SAFETY: `err` is valid to write.
        let s = unsafe { fin_svt_new(&mut err) };
        if s.is_null() {
            Err(err)
        } else {
            Ok(Encoder { s })
        }
    }

    /// Sets one parameter by its SVT-AV1 name (`preset`, `crf`, `keyint`, `lp`, …).
    pub fn param(&mut self, name: &str, value: &str) -> Result<(), i32> {
        let (n, v) = (
            CString::new(name).map_err(|_| -1)?,
            CString::new(value).map_err(|_| -1)?,
        );
        // SAFETY: the handle is live and both strings are NUL-terminated.
        match unsafe { fin_svt_param(self.s, n.as_ptr(), v.as_ptr()) } {
            0 => Ok(()),
            e => Err(e),
        }
    }

    pub fn init(&mut self) -> Result<(), i32> {
        // SAFETY: the handle is live.
        match unsafe { fin_svt_init(self.s) } {
            0 => Ok(()),
            e => Err(e),
        }
    }

    /// The stream header (the sequence header OBU).
    pub fn header(&mut self) -> Result<Vec<u8>, i32> {
        let mut out = vec![0u8; 1024];
        let mut len = 0u32;
        // SAFETY: `out` has `out.len()` writable bytes; `len` is valid to write.
        let r = unsafe { fin_svt_header(self.s, out.as_mut_ptr(), out.len() as u32, &mut len) };
        if r != 0 || len as usize > out.len() {
            return Err(if r != 0 { r } else { -1 });
        }
        out.truncate(len as usize);
        Ok(out)
    }

    /// Sends a tight 8-bit 4:2:0 picture.
    pub fn send(
        &mut self,
        [y, u, v]: [&[u8]; 3],
        size: Size,
        pts: i64,
        key: bool,
    ) -> Result<(), i32> {
        let (width, height) = (size.width, size.height);
        let cw = width.div_ceil(2);
        if y.len() < (width * height) as usize
            || u.len() < (cw * height.div_ceil(2)) as usize
            || v.len() < u.len()
        {
            return Err(-1);
        }
        // SAFETY: the planes hold the picture SVT-AV1 reads (checked above) for the call.
        let r = unsafe {
            fin_svt_send(
                self.s,
                y.as_ptr(),
                u.as_ptr(),
                v.as_ptr(),
                width,
                cw,
                width * height,
                pts,
                key as i32,
            )
        };
        if r == 0 { Ok(()) } else { Err(r) }
    }

    /// Signals the end of the stream.
    pub fn send_eos(&mut self) -> Result<(), i32> {
        // SAFETY: a null luma pointer asks the shim for an end-of-stream buffer.
        let r = unsafe {
            fin_svt_send(
                self.s,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
            )
        };
        if r == 0 { Ok(()) } else { Err(r) }
    }

    /// The next packet; with `done` (every picture sent) this waits for one.
    pub fn packet(&mut self, done: bool) -> Result<Option<Packet>, i32> {
        let mut p = FinSvtPacket {
            data: std::ptr::null(),
            len: 0,
            pts: 0,
            dts: 0,
            flags: 0,
            pic_type: 0,
            raw: std::ptr::null_mut(),
        };
        // SAFETY: the handle is live and `p` is valid to write.
        match unsafe { fin_svt_packet(self.s, done as u8, &mut p) } {
            0 => {}
            1 => return Ok(None),
            e => return Err(e),
        }
        let data = match p.data.is_null() {
            true => Vec::new(),
            // SAFETY: SVT-AV1's packet holds `len` bytes until it is released below.
            false => unsafe { std::slice::from_raw_parts(p.data, p.len as usize) }.to_vec(),
        };
        let out = Packet {
            data,
            pts: p.pts,
            dts: p.dts,
            flags: p.flags,
            pic_type: p.pic_type,
        };
        // SAFETY: `p` came from fin_svt_packet and is released once.
        unsafe { fin_svt_release(&mut p) };
        Ok(Some(out))
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: the handle came from fin_svt_new and is freed once.
        unsafe { fin_svt_free(self.s) };
    }
}
