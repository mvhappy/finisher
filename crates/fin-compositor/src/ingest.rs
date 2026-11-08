//! Ingest on the GPU (spec §5.1 step 2): the Y, U and V planes are copied into `r8unorm`
//! textures (the copy path) and `ingest.wgsl` converts them to an `rgba16float` source texture
//! of the coded size. The coefficients are fin-media's, computed here.

use fin_media::colour::convert::{luma_coefficients, primaries_to_709};
use fin_media::{ChromaSiting, Range};

use crate::pool::{Key, Pool};
use crate::source::SourceFrame;

pub(crate) const SOURCE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub(crate) const SOURCE_USAGE: wgpu::TextureUsages =
    wgpu::TextureUsages::RENDER_ATTACHMENT.union(wgpu::TextureUsages::TEXTURE_BINDING);

/// The `Ingest` uniform block of `ingest.wgsl`.
pub(crate) fn uniforms(src: &SourceFrame) -> [f32; 24] {
    let tags = src.colour;
    let (cw, ch) = (src.size.width.div_ceil(2), src.size.height.div_ceil(2));
    // Chroma sample k sits at luma coordinate 2k + offset (as the reference reads it).
    let (ox, oy) = match tags.siting {
        ChromaSiting::Center => (0.5, 0.5),
        ChromaSiting::TopLeft => (0.0, 0.0),
        ChromaSiting::Left | ChromaSiting::Unknown => (0.0, 0.5),
    };
    let (y_off, y_scale, c_scale) = match tags.range {
        Range::Limited => (16.0, 219.0, 224.0),
        Range::Full => (0.0, 255.0, 255.0),
    };
    let identity = luma_coefficients(tags.matrix).is_none();
    let (kr, kb) = luma_coefficients(tags.matrix).unwrap_or((0.2126, 0.0722));
    let gamut = primaries_to_709(tags.primaries);
    let g = gamut.unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    [
        cw as f32,
        ch as f32,
        ox,
        oy,
        y_off,
        y_scale,
        128.0,
        c_scale,
        kr,
        kb,
        if identity { 1.0 } else { 0.0 },
        if gamut.is_some() { 1.0 } else { 0.0 },
        g[0][0],
        g[0][1],
        g[0][2],
        0.0,
        g[1][0],
        g[1][1],
        g[1][2],
        0.0,
        g[2][0],
        g[2][1],
        g[2][2],
        0.0,
    ]
}

/// Copies the three planes into pooled `r8unorm` textures (`source::readable` has checked
/// their sizes).
pub(crate) fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pool: &mut Pool,
    src: &SourceFrame,
) -> [wgpu::Texture; 3] {
    let (w, h) = (src.size.width, src.size.height);
    let dims = [
        (w, h),
        (w.div_ceil(2), h.div_ceil(2)),
        (w.div_ceil(2), h.div_ceil(2)),
    ];
    [0, 1, 2].map(|k| {
        let (pw, ph) = dims[k];
        let t = pool.acquire(
            device,
            Key {
                width: pw,
                height: ph,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                mips: 1,
            },
        );
        let stride = src.strides[k];
        let len = stride * (ph as usize - 1) + pw as usize;
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
