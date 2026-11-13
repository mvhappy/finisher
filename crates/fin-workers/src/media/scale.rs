//! Decoded frames to the pictures an encoder takes (plan 6B-1 Task 14): the frame as the
//! compositor reads it (the container's colour tags over the bitstream's, spec §10.1's untagged
//! rule, the track's HDR metadata), scaled on the GPU by `Compositor::scale_nv12` (tone-mapped
//! and gamut-converted to BT.709 SDR on the way: Decision 9) into tight NV12.

use fin_codecs::{DecodedFrame, RawFrame};
use fin_compositor::{Compositor, Gpu, SourceFrame};
use fin_core::CoreError;
use fin_media::colour::tonemap::Conversion;
use fin_media::jobs::ToneMapped;
use fin_media::{ColourTags, Size, VideoTrackInfo};

use super::sources::merge;

/// The colour tags a reader gives `f` of `video`.
pub fn colour_of(f: &DecodedFrame, video: &VideoTrackInfo) -> ColourTags {
    merge(video.colour, f.colour).resolve_defaults(f.size.height)
}

/// How a source with these tags is brought to SDR: by the ingest shader for PQ and HLG.
pub fn tone_mapped(colour: &ColourTags, video: &VideoTrackInfo) -> Option<ToneMapped> {
    match Conversion::for_source(colour, video.hdr.as_ref()) {
        Conversion::Hdr(..) => Some(ToneMapped::Shader),
        _ => None,
    }
}

/// `f` as the compositor reads it.
pub fn source_of<'a>(f: &'a DecodedFrame, video: &VideoTrackInfo) -> SourceFrame<'a> {
    SourceFrame {
        format: f.format,
        size: f.size,
        planes: [&f.planes[0].data, &f.planes[1].data, &f.planes[2].data],
        strides: [f.planes[0].stride, f.planes[1].stride, f.planes[2].stride],
        colour: colour_of(f, video),
        hdr: video.hdr,
        rotation: video.rotation,
        pixel_aspect: video.pixel_aspect,
    }
}

/// `f` scaled to `out` as an NV12 picture for an encoder.
pub fn scale(
    compositor: &mut Compositor,
    gpu: &Gpu,
    f: &DecodedFrame,
    video: &VideoTrackInfo,
    out: Size,
) -> Result<RawFrame, CoreError> {
    let nv12 = compositor.scale_nv12(gpu, &source_of(f, video), out)?;
    Ok(RawFrame::nv12(nv12.size, nv12.y, nv12.uv))
}
