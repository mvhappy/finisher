//! The `rav1d` calls (its public API is dav1d's C ABI), each wrapped once. This module and
//! `svt_ffi.rs` are the only places with `unsafe` in `fin-codecs`.

use std::mem::MaybeUninit;
use std::ptr::NonNull;

use fin_media::{ChromaSiting, ColourTags, PixelFormat, Size};
use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{
    DAV1D_CHR_COLOCATED, DAV1D_CHR_VERTICAL, DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I420,
    DAV1D_PIXEL_LAYOUT_I422, Dav1dSequenceHeader,
};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_flush,
    dav1d_get_picture, dav1d_open, dav1d_parse_sequence_header, dav1d_picture_unref,
    dav1d_send_data,
};

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
        Status::Done(frame)
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

fn copy_picture(p: &Dav1dPicture) -> DecodedFrame {
    let (w, h) = (p.p.w.max(0) as u32, p.p.h.max(0) as u32);
    let bytes = if p.p.bpc > 8 { 2 } else { 1 };
    let (cw, ch) = match p.p.layout {
        DAV1D_PIXEL_LAYOUT_I400 => (0, 0),
        DAV1D_PIXEL_LAYOUT_I420 => (w.div_ceil(2), h.div_ceil(2)),
        DAV1D_PIXEL_LAYOUT_I422 => (w.div_ceil(2), h),
        _ => (w, h),
    };
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
    let format = match (p.p.layout, bytes) {
        (DAV1D_PIXEL_LAYOUT_I420, 1) | (DAV1D_PIXEL_LAYOUT_I400, 1) => PixelFormat::I420,
        (DAV1D_PIXEL_LAYOUT_I420, _) | (DAV1D_PIXEL_LAYOUT_I400, _) => PixelFormat::I420p10,
        _ => PixelFormat::I444,
    };
    // SAFETY: a picture from dav1d_get_picture carries its sequence header.
    let colour = p
        .seq_hdr
        .map(|s| colour(unsafe { s.as_ref() }))
        .unwrap_or_default();
    DecodedFrame {
        pts: p.m.timestamp,
        format,
        size: Size::new(w, h),
        planes: [plane(0, w, h), plane(1, cw, ch), plane(2, cw, ch)],
        colour,
    }
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
