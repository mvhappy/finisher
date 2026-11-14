//! `Compositor::scale_nv12`: the GPU half of [`crate::scale`]. The planes are ingested into an
//! `rgba16float` level 0 (the layer path's ingest stage), the box levels down to `⌊L⌋` are built
//! by the mip stage, `fs_scale` (`scale.wgsl`) samples that level bilinearly into an
//! `rgba32float` picture of the output size, `fs_luma` and `fs_chroma` write its NV12 planes
//! (`r8unorm`, `rg8unorm`), and both are read back. Runs inside the compositor's error scopes.

use fin_core::CoreError;
use fin_media::Size;
use serde_json::json;

use crate::device::Gpu;
use crate::gpu::{Compositor, bind, pass, too_large, uniform, view};
use crate::ingest::{self, level_key};
use crate::mipgen::chain_sizes;
use crate::nv12::Nv12Frame;
use crate::pipelines::Stage;
use crate::pool::Key;
use crate::readback;
use crate::scale::{code8, scale_level};
use crate::source::{SourceFrame, readable};

/// The scaled picture before NV12: full `f32`, so the planes are made from what the reference
/// keeps.
/// What `encode_scale` reads back.
enum Planes {
    Nv12,
    Rgba,
}

pub(crate) const SCALED: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
pub(crate) const LUMA: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
pub(crate) const CHROMA: wgpu::TextureFormat = wgpu::TextureFormat::Rg8Unorm;

fn key(width: u32, height: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> Key {
    Key {
        width,
        height,
        format,
        usage,
        mips: 1,
    }
}

impl Compositor {
    /// `src` (coded orientation) scaled to `out` as BT.709 limited-range NV12 (module docs of
    /// [`crate::scale`]). `UNSUPPORTED_PIXEL_FORMAT` for a picture a compositor cannot read,
    /// `MEDIA_LIMIT` past the GPU's texture size, and the GPU's errors as `render` maps them.
    pub fn scale_nv12(
        &mut self,
        gpu: &Gpu,
        src: &SourceFrame,
        out: Size,
    ) -> Result<Nv12Frame, CoreError> {
        self.check_scale(gpu, src, out)?;
        let buffers = self.scoped(gpu, |c| c.encode_scale(gpu, src, out, Planes::Nv12))?;
        let (cw, ch) = (out.width.div_ceil(2), out.height.div_ceil(2));
        let y = readback::read_bytes(gpu, &buffers[0], out.width, out.height, 1)?;
        let uv = readback::read_bytes(gpu, &buffers[1], cw, ch, 2)?;
        Ok(Nv12Frame { size: out, y, uv })
    }

    /// `src` (coded orientation) scaled to `out` as opaque 8-bit RGBA, row-major (the pictures
    /// of thumbnails and filmstrips): the scaled R′G′B′ picture clamped to 0…1, scaled to 255 and
    /// rounded half to even, as `reference::scale::scale_rgba8` does. The same errors as
    /// [`Compositor::scale_nv12`].
    pub fn scale_rgba8(
        &mut self,
        gpu: &Gpu,
        src: &SourceFrame,
        out: Size,
    ) -> Result<Vec<u8>, CoreError> {
        self.check_scale(gpu, src, out)?;
        let buffers = self.scoped(gpu, |c| c.encode_scale(gpu, src, out, Planes::Rgba))?;
        let raw = readback::read_bytes(gpu, &buffers[0], out.width, out.height, 16)?;
        Ok(raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .enumerate()
            .map(|(i, v)| if i % 4 == 3 { 255 } else { code8(v) })
            .collect())
    }

    fn check_scale(&mut self, gpu: &Gpu, src: &SourceFrame, out: Size) -> Result<(), CoreError> {
        gpu.check_lost()?;
        let _ = gpu.take_uncaptured();
        readable(src).map_err(|details| {
            CoreError::new(
                "UNSUPPORTED_PIXEL_FORMAT",
                "the compositor cannot read this picture",
            )
            .with_details(details)
        })?;
        if out.width < 2 || out.height < 2 {
            return Err(
                CoreError::new("INVALID_OP", "a scaled picture is at least 2 × 2")
                    .with_details(json!({ "width": out.width, "height": out.height })),
            );
        }
        Ok(())
    }

    fn encode_scale(
        &mut self,
        gpu: &Gpu,
        src: &SourceFrame,
        out: Size,
        planes_out: Planes,
    ) -> Result<Vec<wgpu::Buffer>, CoreError> {
        let (device, queue) = (gpu.device(), gpu.queue());
        let max = device.limits().max_texture_dimension_2d;
        if src.size.width > max || src.size.height > max {
            return Err(too_large("source picture", src.size, max));
        }
        if out.width > max || out.height > max {
            return Err(too_large("output", out, max));
        }
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
        // Ingest into level 0, then the box levels down to ⌊L⌋.
        let planes = ingest::upload(device, queue, &mut self.pool, src);
        let level = scale_level(src.size, out).floor();
        let sizes = chain_sizes(src.size.width, src.size.height, level);
        let levels: Vec<wgpu::Texture> = sizes
            .iter()
            .map(|&(w, h)| self.pool.acquire(device, level_key(w, h)))
            .collect();
        let pi = self.pipelines.get(device, ingest::stage(src.format));
        let ub = uniform(gpu, &ingest::uniforms(src));
        let pv: Vec<_> = planes.iter().map(view).collect();
        let mut res = vec![ub.as_entire_binding()];
        res.extend(pv.iter().map(wgpu::BindingResource::TextureView));
        pass(
            &mut enc,
            &levels[0],
            clear,
            &pi,
            &bind(device, &pi, res),
            None,
        );
        for k in 1..levels.len() {
            let pm = self.pipelines.get(device, Stage::Mip);
            let above = view(&levels[k - 1]);
            let g = bind(
                device,
                &pm,
                vec![wgpu::BindingResource::TextureView(&above)],
            );
            pass(&mut enc, &levels[k], clear, &pm, &g, None);
        }
        let from = &levels[(level as usize).min(levels.len() - 1)];

        // Bilinear to the output size, at full precision.
        let target = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let scaled = self.pool.acquire(
            device,
            key(
                out.width,
                out.height,
                SCALED,
                target | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            ),
        );
        let ps = self.pipelines.get(device, Stage::Scale);
        let ub = uniform(
            gpu,
            &[
                from.width() as f32,
                from.height() as f32,
                out.width as f32,
                out.height as f32,
            ],
        );
        let fv = view(from);
        let g = bind(
            device,
            &ps,
            vec![
                ub.as_entire_binding(),
                wgpu::BindingResource::TextureView(&fv),
            ],
        );
        pass(&mut enc, &scaled, clear, &ps, &g, None);

        if let Planes::Rgba = planes_out {
            let b = readback::staging(device, out.width, out.height, 16);
            readback::copy_bytes(&mut enc, &scaled, &b, out.width, out.height, 16);
            queue.submit([enc.finish()]);
            return Ok(vec![b]);
        }

        // The NV12 planes.
        let (cw, ch) = (out.width.div_ceil(2), out.height.div_ceil(2));
        let copy = target | wgpu::TextureUsages::COPY_SRC;
        let luma = self
            .pool
            .acquire(device, key(out.width, out.height, LUMA, copy));
        let chroma = self.pool.acquire(device, key(cw, ch, CHROMA, copy));
        let sv = view(&scaled);
        for (stage, plane) in [(Stage::Luma, &luma), (Stage::Chroma, &chroma)] {
            let p = self.pipelines.get(device, stage);
            let g =
                crate::gpu::bind_from(device, &p, 2, vec![wgpu::BindingResource::TextureView(&sv)]);
            pass(&mut enc, plane, clear, &p, &g, None);
        }
        let by = readback::staging(device, out.width, out.height, 1);
        let buv = readback::staging(device, cw, ch, 2);
        readback::copy_bytes(&mut enc, &luma, &by, out.width, out.height, 1);
        readback::copy_bytes(&mut enc, &chroma, &buv, cw, ch, 2);
        queue.submit([enc.finish()]);
        Ok(vec![by, buv])
    }
}
