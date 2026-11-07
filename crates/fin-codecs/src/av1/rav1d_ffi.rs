//! The `rav1d` calls (its public API is dav1d's C ABI), each wrapped once. This module and
//! `svt_ffi.rs` are the only places with `unsafe` in `fin-codecs`.

use std::mem::MaybeUninit;
use std::ptr::NonNull;

use fin_core::CoreError;
use fin_media::{ChromaSiting, ColourTags, PixelFormat, Size};
use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{
    DAV1D_CHR_COLOCATED, DAV1D_CHR_VERTICAL, DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420,
    DAV1D_PIXEL_LAYOUT_I422, DAV1D_PIXEL_LAYOUT_I444, Dav1dSequenceHeader,
};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_flush,
    dav1d_get_picture, dav1d_open, dav1d_parse_sequence_header, dav1d_picture_unref,
    dav1d_send_data,
};
use serde_json::json;

use crate::frame::{DecodedFrame, Plane};

/// `-EAGAIN` as dav1d returns it.
const EAGAIN: i32 = -(libc_eagain());

const fn libc_eagain() -> i32 {
    // EAGAIN is 35 on Apple platforms and the BSDs, 11 on Linux and Windows' CRT.
    if cfg!(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    )) {
        35
    } else {
        11
    }
}

/// What a send or receive did.
pub(crate) enum Status<T> {
    Done(T),
    /// The decoder wants the other call first.
    Again,
    Error(i32),
    /// A picture Finisher does not take (`UNSUPPORTED_PIXEL_FORMAT`).
    Refused(CoreError),
}

/// A dav1d context; closed on drop.
pub(crate) struct Context {
    c: Option<Dav1dContext>,
}

// SAFETY: a dav1d context may move between threads; it is used from one thread at a time
// (`&mut self` on every call).
unsafe impl Send for Context {}

impl Context {
    /// A context with `threads` worker threads (0: dav1d's choice) and a frame delay of 1, so a
    /// frame comes out as soon as its packet is in.
    pub fn open(threads: i32) -> Result<Context, i32> {
        let mut s = MaybeUninit::<Dav1dSettings>::uninit();
        // SAFETY: `s` is valid to write; dav1d_default_settings fills every field.
        let mut s = unsafe {
            dav1d_default_settings(NonNull::new_unchecked(s.as_mut_ptr()));
            s.assume_init()
        };
        s.n_threads = threads;
        s.max_frame_delay = 1;
        let mut c: Option<Dav1dContext> = None;
        // SAFETY: both pointers are valid for the call.
        let r = unsafe { dav1d_open(Some(NonNull::from(&mut c)), Some(NonNull::from(&mut s))) };
        match (r.0, c) {
            (0, Some(_)) => Ok(Context { c }),
            (e, _) => Err(e),
        }
    }

    /// Sends `data` (copied into a dav1d buffer) with `pts` as its timestamp. On `Again` the
    /// packet was not taken: receive a picture and send it again.
    pub fn send(&mut self, data: &[u8], pts: i64) -> Status<()> {
        let mut d = Dav1dData::default();
        // SAFETY: `d` is valid to write; the returned buffer has `data.len()` bytes.
        let buf = unsafe { dav1d_data_create(Some(NonNull::from(&mut d)), data.len()) };
        if buf.is_null() {
            return Status::Error(-12);
        }
        // SAFETY: `buf` points to `data.len()` writable bytes that `d` owns.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len()) };
        d.m.timestamp = pts;
        let mut status = Status::Done(());
        while d.sz > 0 {
            // SAFETY: the context is open and `d` is a valid dav1d buffer.
            let r = unsafe { dav1d_send_data(self.c, Some(NonNull::from(&mut d))) };
            match r.0 {
                0 => {}
                EAGAIN if d.sz == data.len() => {
                    status = Status::Again;
                    break;
                }
                // Partly taken: the rest needs a picture out first, which this API does not
                // allow mid-packet; treat it as an error.
                EAGAIN => {
                    status = Status::Error(EAGAIN);
                    break;
                }
                e => {
                    status = Status::Error(e);
                    break;
                }
            }
        }
        // SAFETY: `d` is a dav1d buffer; unref releases what was not taken.
        unsafe { dav1d_data_unref(Some(NonNull::from(&mut d))) };
        status
    }

    /// The next picture, copied out into tight planes.
    pub fn receive(&mut self) -> Status<DecodedFrame> {
        let mut p = Dav1dPicture::default();
        // SAFETY: the context is open and `p` is valid to write.
        let r = unsafe { dav1d_get_picture(self.c, Some(NonNull::from(&mut p))) };
        match r.0 {
            0 => {}
            EAGAIN => return Status::Again,
            e => return Status::Error(e),
        }
        let frame = copy_picture(&p);
        // SAFETY: `p` holds a reference dav1d gave us.
        unsafe { dav1d_picture_unref(Some(NonNull::from(&mut p))) };
        match frame {
            Ok(f) => Status::Done(f),
            Err(e) => Status::Refused(e),
        }
    }

    /// Drops buffered packets and pictures (a seek).
    pub fn flush(&mut self) {
        if let Some(c) = self.c {
            // SAFETY: the context is open.
            unsafe { dav1d_flush(c) };
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: the context came from dav1d_open and is closed once.
        unsafe { dav1d_close(Some(NonNull::from(&mut self.c))) };
    }
}

fn colour(seq: &Dav1dSequenceHeader) -> ColourTags {
    let mut t = ColourTags::from_cicp(
        seq.pri as u16,
        seq.trc as u16,
        seq.mtrx as u16,
        seq.color_range != 0,
    );
    t.siting = match seq.chr {
        DAV1D_CHR_VERTICAL => ChromaSiting::Left,
        DAV1D_CHR_COLOCATED => ChromaSiting::TopLeft,
        _ => ChromaSiting::Unknown,
    };
    t
}

/// The pixel format of a dav1d layout and bit depth: 4:2:0, 4:2:2 and 4:4:4 at 8 or 10 bits.
/// Monochrome (I400) and 12-bit pictures are `UNSUPPORTED_PIXEL_FORMAT`.
pub(crate) fn pixel_format(layout: u32, bpc: i32) -> Result<PixelFormat, CoreError> {
    let f = match (layout, bpc) {
        (DAV1D_PIXEL_LAYOUT_I420, 8) => Some(PixelFormat::I420),
        (DAV1D_PIXEL_LAYOUT_I420, 10) => Some(PixelFormat::I420p10),
        (DAV1D_PIXEL_LAYOUT_I422, 8) => Some(PixelFormat::I422),
        (DAV1D_PIXEL_LAYOUT_I422, 10) => Some(PixelFormat::I422p10),
        (DAV1D_PIXEL_LAYOUT_I444, 8) => Some(PixelFormat::I444),
        (DAV1D_PIXEL_LAYOUT_I444, 10) => Some(PixelFormat::I444p10),
        _ => None,
    };
    f.ok_or_else(|| {
        let name = match layout {
            DAV1D_PIXEL_LAYOUT_I400 => "monochrome",
            DAV1D_PIXEL_LAYOUT_I420 => "4:2:0",
            DAV1D_PIXEL_LAYOUT_I422 => "4:2:2",
            DAV1D_PIXEL_LAYOUT_I444 => "4:4:4",
            _ => "unknown",
        };
        CoreError::new(
            "UNSUPPORTED_PIXEL_FORMAT",
            format!("{name} pictures at {bpc} bits are not supported"),
        )
        .with_details(json!({ "backend": "rav1d", "layout": name, "bitDepth": bpc }))
        .with_fix("transcode the source to 8- or 10-bit 4:2:0")
    })
}

/// Chroma plane size for `format` at `w × h`.
fn chroma_size(format: PixelFormat, w: u32, h: u32) -> (u32, u32) {
    match format {
        PixelFormat::I420 | PixelFormat::I420p10 | PixelFormat::Nv12 | PixelFormat::P010 => {
            (w.div_ceil(2), h.div_ceil(2))
        }
        PixelFormat::I422 | PixelFormat::I422p10 => (w.div_ceil(2), h),
        _ => (w, h),
    }
}

fn copy_picture(p: &Dav1dPicture) -> Result<DecodedFrame, CoreError> {
    let format = pixel_format(p.p.layout, p.p.bpc)?;
    let (w, h) = (p.p.w.max(0) as u32, p.p.h.max(0) as u32);
    let bytes = if p.p.bpc > 8 { 2 } else { 1 };
    let (cw, ch) = chroma_size(format, w, h);
    let plane = |k: usize, pw: u32, ph: u32| {
        let stride = p.stride[if k == 0 { 0 } else { 1 }];
        let row = pw as usize * bytes;
        let mut data = vec![0u8; row * ph as usize];
        if let Some(src) = p.data[k] {
            for y in 0..ph as usize {
                // SAFETY: dav1d's plane `k` has `ph` rows of `stride` bytes, each with at least
                // `row` bytes in use; the picture is referenced until unref.
                let line = unsafe {
                    std::slice::from_raw_parts(
                        (src.as_ptr() as *const u8).offset(y as isize * stride),
                        row,
                    )
                };
                data[y * row..(y + 1) * row].copy_from_slice(line);
            }
        }
        Plane {
            data,
            stride: row,
            width: pw,
            height: ph,
        }
    };
    // SAFETY: a picture from dav1d_get_picture carries its sequence header.
    let colour = p
        .seq_hdr
        .map(|s| colour(unsafe { s.as_ref() }))
        .unwrap_or_default();
    Ok(DecodedFrame {
        pts: p.m.timestamp,
        format,
        size: Size::new(w, h),
        planes: [plane(0, w, h), plane(1, cw, ch), plane(2, cw, ch)],
        colour,
    })
}

/// What an AV1 sequence header OBU says, for `av1C`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SequenceInfo {
    pub profile: u8,
    pub level: u8,
    pub tier: u8,
    pub high_bitdepth: bool,
    pub twelve_bit: bool,
    pub monochrome: bool,
    pub subsampling: (bool, bool),
    pub chroma_position: u8,
}

/// Parses a sequence header OBU with dav1d's parser.
pub(crate) fn parse_sequence_header(obus: &[u8]) -> Option<SequenceInfo> {
    let mut seq = MaybeUninit::<Dav1dSequenceHeader>::zeroed();
    // SAFETY: `seq` is valid to write; `obus` is a live slice of its length.
    let r = unsafe {
        dav1d_parse_sequence_header(
            NonNull::new(seq.as_mut_ptr()),
            NonNull::new(obus.as_ptr() as *mut u8),
            obus.len(),
        )
    };
    if r.0 != 0 {
        return None;
    }
    // SAFETY: dav1d filled the header on success.
    let s = unsafe { seq.assume_init() };
    let op = s.operating_points[0];
    let (ssx, ssy) = match s.layout {
        DAV1D_PIXEL_LAYOUT_I420 | DAV1D_PIXEL_LAYOUT_I400 => (true, true),
        DAV1D_PIXEL_LAYOUT_I422 => (true, false),
        _ => (false, false),
    };
    Some(SequenceInfo {
        profile: s.profile,
        level: (op.major_level.saturating_sub(2) << 2) | op.minor_level,
        tier: op.tier,
        high_bitdepth: s.hbd >= 1,
        twelve_bit: s.hbd == 2,
        monochrome: s.layout == DAV1D_PIXEL_LAYOUT_I400,
        subsampling: (ssx, ssy),
        chroma_position: s.chr as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_and_depths_map_exactly() {
        let ok = |l, b| pixel_format(l, b).unwrap();
        assert_eq!(ok(DAV1D_PIXEL_LAYOUT_I420, 8), PixelFormat::I420);
        assert_eq!(ok(DAV1D_PIXEL_LAYOUT_I420, 10), PixelFormat::I420p10);
        assert_eq!(ok(DAV1D_PIXEL_LAYOUT_I422, 8), PixelFormat::I422);
        assert_eq!(ok(DAV1D_PIXEL_LAYOUT_I422, 10), PixelFormat::I422p10);
        assert_eq!(ok(DAV1D_PIXEL_LAYOUT_I444, 8), PixelFormat::I444);
        assert_eq!(ok(DAV1D_PIXEL_LAYOUT_I444, 10), PixelFormat::I444p10);
        for (l, b) in [
            (DAV1D_PIXEL_LAYOUT_I400, 8),
            (DAV1D_PIXEL_LAYOUT_I400, 10),
            (DAV1D_PIXEL_LAYOUT_I420, 12),
            (DAV1D_PIXEL_LAYOUT_I444, 12),
        ] {
            let e = pixel_format(l, b).unwrap_err();
            assert_eq!(e.code, "UNSUPPORTED_PIXEL_FORMAT");
            assert_eq!(e.details["bitDepth"], b);
        }
        assert_eq!(
            pixel_format(DAV1D_PIXEL_LAYOUT_I400, 8)
                .unwrap_err()
                .details["layout"],
            "monochrome"
        );
        // Chroma planes follow the subsampling.
        assert_eq!(chroma_size(PixelFormat::I422p10, 7, 5), (4, 5));
        assert_eq!(chroma_size(PixelFormat::I420, 7, 5), (4, 3));
        assert_eq!(chroma_size(PixelFormat::I444p10, 7, 5), (7, 5));
    }
}
