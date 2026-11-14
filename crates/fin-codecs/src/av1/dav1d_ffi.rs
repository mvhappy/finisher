//! The dav1d shim's calls (`fin-dav1d-sys`), each wrapped once. This module and `svt_ffi.rs`
//! are the only places with `unsafe` in `fin-codecs`.

use std::ffi::CStr;
use std::ptr::{self, NonNull};

use fin_core::CoreError;
use fin_dav1d_sys as sys;
use fin_media::{ChromaSiting, ColourTags, PixelFormat, Size};
use serde_json::json;

use crate::frame::{DecodedFrame, Plane};

/// What a send or receive did.
pub(crate) enum Status<T> {
    Done(T),
    /// The decoder wants the other call first.
    Again,
    Error(i32),
    /// A picture Finisher does not take (`UNSUPPORTED_PIXEL_FORMAT`).
    Refused(CoreError),
}

/// The linked dav1d's version string.
pub(crate) fn version() -> String {
    // SAFETY: dav1d returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(sys::fin_dav1d_version()) }
        .to_string_lossy()
        .into_owned()
}

/// The frame delay dav1d resolves for `threads` and `frame_delay` (0: its choice).
pub(crate) fn frame_delay(threads: i32, frame_delay: i32) -> i32 {
    // SAFETY: plain values in and out.
    unsafe { sys::fin_dav1d_frame_delay(threads, frame_delay) }
}

/// A dav1d decoder; closed on drop.
pub(crate) struct Context {
    d: NonNull<sys::FinDav1d>,
}

// SAFETY: a dav1d context may move between threads; it is used from one thread at a time
// (`&mut self` on every call). dav1d's worker threads synchronise among themselves.
unsafe impl Send for Context {}

impl Context {
    /// A decoder with `threads` workers (0: one per logical processor) and up to
    /// `frame_delay` frames in flight (0: dav1d's choice); pictures over `max_pixels` are
    /// refused.
    pub fn open(threads: i32, frame_delay: i32, max_pixels: u32) -> Result<Context, i32> {
        let mut d = ptr::null_mut();
        // SAFETY: `d` is valid to write; the shim fills it on success.
        let r = unsafe { sys::fin_dav1d_open(threads, frame_delay, max_pixels, &mut d) };
        match (r, NonNull::new(d)) {
            (0, Some(d)) => Ok(Context { d }),
            (0, None) => Err(-1),
            (e, _) => Err(e),
        }
    }

    /// Sends `data` (copied) with `pts` as its timestamp. On `Again` the packet was not
    /// taken: receive a picture and send it again.
    pub fn send(&mut self, data: &[u8], pts: i64) -> Status<()> {
        // SAFETY: the decoder is open; `data` is a live slice of its length.
        match unsafe { sys::fin_dav1d_send(self.d.as_ptr(), data.as_ptr(), data.len(), pts) } {
            0 => Status::Done(()),
            sys::AGAIN => Status::Again,
            e => Status::Error(e),
        }
    }

    /// The next picture, copied out into tight planes.
    pub fn receive(&mut self) -> Status<DecodedFrame> {
        // SAFETY: an all-zero record is valid (null pointers, zero values); the shim fills it.
        let mut p: sys::FinDav1dPicture = unsafe { std::mem::zeroed() };
        // SAFETY: the decoder is open and `p` is valid to write.
        match unsafe { sys::fin_dav1d_get_picture(self.d.as_ptr(), &mut p) } {
            0 => {}
            sys::AGAIN => return Status::Again,
            e => return Status::Error(e),
        }
        let frame = copy_picture(&p);
        // SAFETY: `p` holds the reference the shim gave us; released once.
        unsafe { sys::fin_dav1d_picture_release(&mut p) };
        match frame {
            Ok(f) => Status::Done(f),
            Err(e) => Status::Refused(e),
        }
    }

    /// Drops buffered packets and pictures (a seek).
    pub fn flush(&mut self) {
        // SAFETY: the decoder is open.
        unsafe { sys::fin_dav1d_flush(self.d.as_ptr()) };
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: the decoder came from fin_dav1d_open and is closed once.
        unsafe { sys::fin_dav1d_close(self.d.as_ptr()) };
    }
}

fn colour(p: &sys::FinDav1dPicture) -> ColourTags {
    if p.has_colour == 0 {
        return ColourTags::default();
    }
    let mut t = ColourTags::from_cicp(
        p.primaries as u16,
        p.transfer as u16,
        p.matrix as u16,
        p.full_range != 0,
    );
    t.siting = match p.chroma_position {
        sys::CHR_VERTICAL => ChromaSiting::Left,
        sys::CHR_COLOCATED => ChromaSiting::TopLeft,
        _ => ChromaSiting::Unknown,
    };
    t
}

/// The pixel format of a dav1d layout and bit depth: 4:2:0, 4:2:2 and 4:4:4 at 8 or 10 bits.
/// Monochrome (I400) and 12-bit pictures are `UNSUPPORTED_PIXEL_FORMAT`.
pub(crate) fn pixel_format(layout: i32, bpc: i32) -> Result<PixelFormat, CoreError> {
    let f = match (layout, bpc) {
        (sys::LAYOUT_I420, 8) => Some(PixelFormat::I420),
        (sys::LAYOUT_I420, 10) => Some(PixelFormat::I420p10),
        (sys::LAYOUT_I422, 8) => Some(PixelFormat::I422),
        (sys::LAYOUT_I422, 10) => Some(PixelFormat::I422p10),
        (sys::LAYOUT_I444, 8) => Some(PixelFormat::I444),
        (sys::LAYOUT_I444, 10) => Some(PixelFormat::I444p10),
        _ => None,
    };
    f.ok_or_else(|| {
        let name = match layout {
            sys::LAYOUT_I400 => "monochrome",
            sys::LAYOUT_I420 => "4:2:0",
            sys::LAYOUT_I422 => "4:2:2",
            sys::LAYOUT_I444 => "4:4:4",
            _ => "unknown",
        };
        CoreError::new(
            "UNSUPPORTED_PIXEL_FORMAT",
            format!("{name} pictures at {bpc} bits are not supported"),
        )
        .with_details(
            json!({ "backend": super::decode::BACKEND.0, "layout": name, "bitDepth": bpc }),
        )
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

fn copy_picture(p: &sys::FinDav1dPicture) -> Result<DecodedFrame, CoreError> {
    let format = pixel_format(p.layout, p.bpc)?;
    let (w, h) = (p.width.max(0) as u32, p.height.max(0) as u32);
    let bytes = if p.bpc > 8 { 2 } else { 1 };
    let (cw, ch) = chroma_size(format, w, h);
    let plane = |k: usize, pw: u32, ph: u32| {
        let stride = p.stride[if k == 0 { 0 } else { 1 }] as isize;
        let row = pw as usize * bytes;
        let mut data = vec![0u8; row * ph as usize];
        if !p.data[k].is_null() {
            for y in 0..ph as usize {
                // SAFETY: dav1d's plane `k` has `ph` rows of `stride` bytes, each with at least
                // `row` bytes in use; the picture is referenced until it is released.
                let line = unsafe {
                    std::slice::from_raw_parts(p.data[k].offset(y as isize * stride), row)
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
    Ok(DecodedFrame {
        pts: p.pts,
        format,
        size: Size::new(w, h),
        planes: [plane(0, w, h), plane(1, cw, ch), plane(2, cw, ch)],
        colour: colour(p),
        hw: false,
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
    let mut s = sys::FinDav1dSequence::default();
    // SAFETY: `obus` is a live slice of its length; `s` is valid to write.
    let r = unsafe { sys::fin_dav1d_parse_sequence_header(obus.as_ptr(), obus.len(), &mut s) };
    if r != 0 {
        return None;
    }
    let layout = s.layout as i32;
    let (ssx, ssy) = match layout {
        sys::LAYOUT_I420 | sys::LAYOUT_I400 => (true, true),
        sys::LAYOUT_I422 => (true, false),
        _ => (false, false),
    };
    Some(SequenceInfo {
        profile: s.profile,
        level: (s.major_level.saturating_sub(2) << 2) | s.minor_level,
        tier: s.tier,
        high_bitdepth: s.hbd >= 1,
        twelve_bit: s.hbd == 2,
        monochrome: layout == sys::LAYOUT_I400,
        subsampling: (ssx, ssy),
        chroma_position: s.chroma_position,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_and_depths_map_exactly() {
        let ok = |l, b| pixel_format(l, b).unwrap();
        assert_eq!(ok(sys::LAYOUT_I420, 8), PixelFormat::I420);
        assert_eq!(ok(sys::LAYOUT_I420, 10), PixelFormat::I420p10);
        assert_eq!(ok(sys::LAYOUT_I422, 8), PixelFormat::I422);
        assert_eq!(ok(sys::LAYOUT_I422, 10), PixelFormat::I422p10);
        assert_eq!(ok(sys::LAYOUT_I444, 8), PixelFormat::I444);
        assert_eq!(ok(sys::LAYOUT_I444, 10), PixelFormat::I444p10);
        for (l, b) in [
            (sys::LAYOUT_I400, 8),
            (sys::LAYOUT_I400, 10),
            (sys::LAYOUT_I420, 12),
            (sys::LAYOUT_I444, 12),
        ] {
            let e = pixel_format(l, b).unwrap_err();
            assert_eq!(e.code, "UNSUPPORTED_PIXEL_FORMAT");
            assert_eq!(e.details["bitDepth"], b);
            assert_eq!(e.details["backend"], "dav1d");
        }
        assert_eq!(
            pixel_format(sys::LAYOUT_I400, 8).unwrap_err().details["layout"],
            "monochrome"
        );
        // Chroma planes follow the subsampling.
        assert_eq!(chroma_size(PixelFormat::I422p10, 7, 5), (4, 5));
        assert_eq!(chroma_size(PixelFormat::I420, 7, 5), (4, 3));
        assert_eq!(chroma_size(PixelFormat::I444p10, 7, 5), (7, 5));
    }

    #[test]
    fn the_pinned_dav1d_is_linked() {
        assert_eq!(version(), "1.5.4");
        // One thread: no frame threading; four: dav1d's ceil(sqrt(4)).
        assert_eq!(frame_delay(1, 0), 1);
        assert_eq!(frame_delay(4, 0), 2);
        assert_eq!(frame_delay(8, 3), 3);
    }
}
