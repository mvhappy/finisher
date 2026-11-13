//! Ingest on the GPU (spec §5.1 step 2): the planes are copied into textures (the copy path)
//! and `ingest.wgsl` converts them to an `rgba16float` source texture of the coded size. The
//! coefficients are fin-media's, computed here.
//!
//! I420 is three `r8unorm` planes, NV12 `r8unorm` Y and `rg8unorm` UV, P010 `r16uint` Y and
//! `rg16uint` UV read with `textureLoad` (16-bit normalised formats need an optional feature).
//! Each layout has its own fetch prefix (`ingest_*.wgsl`) in front of the shared body, so every
//! format reaches the conversion as code values, in the reference's order of operations.

use fin_media::colour::convert::{IDENTITY, luma_coefficients};
use fin_media::colour::tonemap::{Conversion, Eetf, Hdr};
use fin_media::{ChromaSiting, PixelFormat, Range};

use crate::gpu::Stage;
use crate::pool::{Key, Pool};
use crate::source::SourceFrame;

/// The ingest shaders by layout (I420, NV12, P010): a label and the fetch prefix.
pub(crate) const SHADERS: [(&str, &str); 3] = [
    ("ingest i420", include_str!("shaders/ingest_i420.wgsl")),
    ("ingest nv12", include_str!("shaders/ingest_nv12.wgsl")),
    ("ingest p010", include_str!("shaders/ingest_p010.wgsl")),
];
/// The body every ingest shader shares.
pub(crate) const BODY: &str = include_str!("shaders/ingest.wgsl");
/// The colour conversion the body ends with (fin-media's `tonemap` and `softclip`, ported).
pub(crate) const TONEMAP: &str = include_str!("shaders/tonemap.wgsl");

/// The ingest stage for `format` (`source::readable` has checked it is one of the three).
pub(crate) fn stage(format: PixelFormat) -> Stage {
    match format {
        PixelFormat::Nv12 => Stage::IngestNv12,
        PixelFormat::P010 => Stage::IngestP010,
        _ => Stage::Ingest,
    }
}

/// The index into [`SHADERS`] of an ingest stage.
pub(crate) fn shader_index(stage: Stage) -> usize {
    match stage {
        Stage::IngestNv12 => 1,
        Stage::IngestP010 => 2,
        _ => 0,
    }
}

/// Per plane of `format`: the texture format and its bytes per texel, and whether it is the
/// full-size (luma) plane.
fn layout(format: PixelFormat) -> &'static [(wgpu::TextureFormat, u32, bool)] {
    use wgpu::TextureFormat as F;
    match format {
        PixelFormat::Nv12 => &[(F::R8Unorm, 1, true), (F::Rg8Unorm, 2, false)],
        PixelFormat::P010 => &[(F::R16Uint, 2, true), (F::Rg16Uint, 4, false)],
        _ => &[
            (F::R8Unorm, 1, true),
            (F::R8Unorm, 1, false),
            (F::R8Unorm, 1, false),
        ],
    }
}

pub(crate) const SOURCE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub(crate) const SOURCE_USAGE: wgpu::TextureUsages =
    wgpu::TextureUsages::RENDER_ATTACHMENT.union(wgpu::TextureUsages::TEXTURE_BINDING);

/// The `Ingest` uniform block of `ingest.wgsl`.
pub(crate) fn uniforms(src: &SourceFrame) -> [f32; 36] {
    let tags = src.colour;
    let (cw, ch) = (src.size.width.div_ceil(2), src.size.height.div_ceil(2));
    // Chroma sample k sits at luma coordinate 2k + offset (as the reference reads it).
    let (ox, oy) = match tags.siting {
        ChromaSiting::Center => (0.5, 0.5),
        ChromaSiting::TopLeft => (0.0, 0.0),
        ChromaSiting::Left | ChromaSiting::Unknown => (0.0, 0.5),
    };
    // fin-media's `expand` at the source's depth: limited range scaled by 2ⁿ⁻⁸, full range
    // over 2ⁿ − 1, chroma centred at 2ⁿ⁻¹.
    let bits = if src.format == PixelFormat::P010 {
        10
    } else {
        8
    };
    let scale = (1u32 << (bits - 8)) as f32;
    let max = ((1u32 << bits) - 1) as f32;
    let (y_off, y_scale, c_scale) = match tags.range {
        Range::Limited => (16.0 * scale, 219.0 * scale, 224.0 * scale),
        Range::Full => (0.0, max, max),
    };
    let gbr = luma_coefficients(tags.matrix).is_none();
    let (kr, kb) = luma_coefficients(tags.matrix).unwrap_or((0.2126, 0.0722));
    // The conversion (`tonemap::Conversion`) as the shader's mode, matrix and EETF constants.
    let none = Eetf {
        black: 0.0,
        range: 1.0,
        max_lum: 1.0,
        min_lum: 0.0,
        knee: 1.0,
    };
    let (mode, g, e, target) = match Conversion::for_source(&tags, src.hdr.as_ref()) {
        Conversion::None => (0.0, IDENTITY, none, 1.0),
        Conversion::Gamut(m) => (1.0, m, none, 1.0),
        Conversion::Hdr(kind, p) => (
            if kind == Hdr::Pq { 2.0 } else { 3.0 },
            p.gamut,
            p.eetf,
            p.target_peak,
        ),
    };
    [
        cw as f32,
        ch as f32,
        ox,
        oy,
        y_off,
        y_scale,
        128.0 * scale,
        c_scale,
        kr,
        kb,
        if gbr { 1.0 } else { 0.0 },
        mode,
        g.matrix[0][0],
        g.matrix[0][1],
        g.matrix[0][2],
        g.compression.threshold[0],
        g.matrix[1][0],
        g.matrix[1][1],
        g.matrix[1][2],
        g.compression.threshold[1],
        g.matrix[2][0],
        g.matrix[2][1],
        g.matrix[2][2],
        g.compression.threshold[2],
        e.black,
        e.range,
        e.max_lum,
        e.min_lum,
        e.knee,
        target,
        0.0,
        0.0,
        g.compression.scale[0],
        g.compression.scale[1],
        g.compression.scale[2],
        0.0,
    ]
}

/// Copies the planes into pooled textures (`source::readable` has checked their sizes).
pub(crate) fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pool: &mut Pool,
    src: &SourceFrame,
) -> Vec<wgpu::Texture> {
    let (w, h) = (src.size.width, src.size.height);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    layout(src.format)
        .iter()
        .enumerate()
        .map(|(k, &(format, texel, luma))| {
            let (pw, ph) = if luma { (w, h) } else { (cw, ch) };
            let t = pool.acquire(
                device,
                Key {
                    width: pw,
                    height: ph,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    mips: 1,
                },
            );
            let stride = src.strides[k];
            let len = stride * (ph as usize - 1) + (pw * texel) as usize;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &t,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &src.planes[k][..len],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride as u32),
                    rows_per_image: Some(ph),
                },
                wgpu::Extent3d {
                    width: pw,
                    height: ph,
                    depth_or_array_layers: 1,
                },
            );
            t
        })
        .collect()
}

/// The source texture key for a level of `width × height`.
pub(crate) fn level_key(width: u32, height: u32) -> Key {
    Key {
        width,
        height,
        format: SOURCE_FORMAT,
        usage: SOURCE_USAGE,
        mips: 1,
    }
}
