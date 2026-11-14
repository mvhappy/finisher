//! The wgpu compositor (spec §5.1, the 6A subset): per clip layer, the planes are uploaded and
//! ingested into an `rgba16float` source, the mip levels it needs are built, and the layer
//! pass samples at the explicit level with analytic coverage, fused with a fixed-function blend
//! onto the `rgba16float` accumulation (`normal`, `screen`, `add`) or written to a scratch
//! target that `blend.wgsl` composites into the other ping-pong target. The output pass
//! converts to `rgba8unorm`, read back with 256-byte rows.
//!
//! Pipelines are made once per (stage, mode) on first use; intermediates come from the texture
//! pool. Each frame runs inside out-of-memory, internal and validation error scopes. After
//! `GPU_DEVICE_LOST`, make a new [`Gpu`] and a new `Compositor`.

use fin_core::eval::{FramePlan, Layer, Role};
use fin_core::{CoreError, Resolution, TransitionKind};
use fin_media::Size;
use serde_json::json;

use crate::blend;
use crate::device::{Gpu, map_error};
use crate::ingest::{self, level_key};
use crate::layer;
use crate::mipgen::{chain_sizes, pick};
use crate::output::RenderOutput;
use crate::pipelines::Pipelines;
pub use crate::pipelines::Stage;
use crate::placement::{place, weight};
use crate::pool::{Key, Pool, PoolStats};
use crate::readback::{self, Readback};
use crate::source::{SourceFrame, SourceProvider, walk};

pub(crate) const ACC: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub(crate) const OUT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub(crate) fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_ne_bytes()).collect()
}

/// A uniform buffer holding `v` (written through the queue: `create_buffer_init` maps at
/// creation, which panics on a lost device).
pub(crate) fn uniform(gpu: &Gpu, v: &[f32]) -> wgpu::Buffer {
    let bytes = floats(v);
    let b = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("fin-compositor uniforms"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue().write_buffer(&b, 0, &bytes);
    b
}

pub(crate) fn view(t: &wgpu::Texture) -> wgpu::TextureView {
    t.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A bind group of `resources` at bindings `first`, `first + 1`, …
pub(crate) fn bind_from(
    device: &wgpu::Device,
    pipeline: &wgpu::RenderPipeline,
    first: u32,
    resources: Vec<wgpu::BindingResource<'_>>,
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry<'_>> = resources
        .into_iter()
        .enumerate()
        .map(|(i, resource)| wgpu::BindGroupEntry {
            binding: first + i as u32,
            resource,
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    })
}

pub(crate) fn bind(
    device: &wgpu::Device,
    pipeline: &wgpu::RenderPipeline,
    resources: Vec<wgpu::BindingResource<'_>>,
) -> wgpu::BindGroup {
    bind_from(device, pipeline, 0, resources)
}

/// One full-target draw (limited to `scissor`, `[x, y, w, h]`, when given).
pub(crate) fn pass(
    enc: &mut wgpu::CommandEncoder,
    target: &wgpu::Texture,
    load: wgpu::LoadOp<wgpu::Color>,
    pipeline: &wgpu::RenderPipeline,
    group: &wgpu::BindGroup,
    scissor: Option<[u32; 4]>,
) {
    let v = view(target);
    let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &v,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    rp.set_pipeline(pipeline);
    rp.set_bind_group(0, group, &[]);
    if let Some([x, y, w, h]) = scissor {
        rp.set_scissor_rect(x, y, w, h);
    }
    rp.draw(0..3, 0..1);
}

pub(crate) fn too_large(what: &str, size: Size, max: u32) -> CoreError {
    CoreError::new(
        "MEDIA_LIMIT",
        format!("the {what} is larger than the GPU's {max}-pixel texture limit"),
    )
    .with_details(json!({ "what": what, "width": size.width, "height": size.height, "limit": max }))
}

/// The per-frame state the layer passes share.
struct Frame<'a> {
    gpu: &'a Gpu,
    enc: wgpu::CommandEncoder,
    acc: [wgpu::Texture; 2],
    cur: usize,
    acc_key: Key,
    resolution: Resolution,
    out: Size,
}

/// The GPU compositor: pipelines, the texture pool, the sampler and the readback buffer.
pub struct Compositor {
    pub(crate) pipelines: Pipelines,
    pub(crate) pool: Pool,
    sampler: wgpu::Sampler,
    readback: Readback,
    shader_blends: bool,
    /// The last render's readback (map and copy out), for diagnostics.
    last_readback_micros: u64,
}

impl Compositor {
    pub fn new(gpu: &Gpu) -> Compositor {
        let device = gpu.device();
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fin-compositor bilinear"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        Compositor {
            pipelines: Pipelines::new(device),
            pool: Pool::default(),
            sampler,
            readback: Readback::default(),
            shader_blends: false,
            last_readback_micros: 0,
        }
    }

    /// Microseconds the last successful [`Compositor::render`] spent reading the picture
    /// back from the GPU (`render.stats`' `readback` phase).
    pub fn last_readback_micros(&self) -> u64 {
        self.last_readback_micros
    }

    /// Sends every blend mode through `blend.wgsl`, never a fixed-function blend (to check the
    /// two agree; renders stay within T1 of the reference either way).
    pub fn set_shader_blends(&mut self, on: bool) {
        self.shader_blends = on;
    }

    /// How many pipelines were created, and for which stages.
    pub fn pipelines_created(&self) -> usize {
        self.pipelines.created
    }

    pub fn pipeline_stages(&self) -> Vec<Stage> {
        self.pipelines.map.keys().copied().collect()
    }

    pub fn pool_stats(&self) -> PoolStats {
        self.pool.stats()
    }

    /// Renders `plan` at `out` (the plan's resolution scaled to `out`).
    pub fn render(
        &mut self,
        gpu: &Gpu,
        plan: &FramePlan,
        src: &mut dyn SourceProvider,
        out: Size,
    ) -> Result<RenderOutput, CoreError> {
        gpu.check_lost()?;
        let _ = gpu.take_uncaptured();
        if out.width == 0 || out.height == 0 {
            let skipped = walk(plan, src, |_, _| Ok::<(), CoreError>(()))?;
            return Ok(RenderOutput {
                size: out,
                rgba8: Vec::new(),
                skipped,
            });
        }
        let (skipped, buffer) = self.scoped(gpu, |c| c.encode(gpu, plan, src, out))?;
        let started = std::time::Instant::now();
        let rgba8 = readback::read(gpu, &buffer, out.width, out.height)?;
        self.last_readback_micros =
            u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        Ok(RenderOutput {
            size: out,
            rgba8,
            skipped,
        })
    }

    /// Runs `f` (which records and submits GPU work) inside out-of-memory, internal and
    /// validation error scopes and ends the pool's frame; the first error a scope caught, or a
    /// lost device, wins over `f`'s own answer. Anything but `INTERNAL` clears the pool and the
    /// readback buffer.
    pub(crate) fn scoped<T>(
        &mut self,
        gpu: &Gpu,
        f: impl FnOnce(&mut Self) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let oom = gpu
            .device()
            .push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        // A pipeline the backend failed to compile (an FXC error) is an internal error, and
        // the cause of the validation errors that follow it; it is reported first.
        let internal = gpu.device().push_error_scope(wgpu::ErrorFilter::Internal);
        let validation = gpu.device().push_error_scope(wgpu::ErrorFilter::Validation);
        let encoded = f(self);
        let invalid = pollster::block_on(validation.pop());
        let broken = pollster::block_on(internal.pop());
        let no_memory = pollster::block_on(oom.pop());
        self.pool.end_frame();
        let failed = gpu
            .check_lost()
            .and_then(|()| no_memory.map_or(Ok(()), |e| Err(map_error(&e))))
            .and_then(|()| broken.map_or(Ok(()), |e| Err(map_error(&e))))
            .and_then(|()| invalid.map_or(Ok(()), |e| Err(map_error(&e))))
            .and_then(|()| gpu.take_uncaptured());
        if let Err(e) = failed {
            if e.code != "INTERNAL" {
                self.pool.clear();
                self.readback.clear();
            }
            return Err(e);
        }
        encoded
    }

    fn encode(
        &mut self,
        gpu: &Gpu,
        plan: &FramePlan,
        src: &mut dyn SourceProvider,
        out: Size,
    ) -> Result<(Vec<crate::Skipped>, wgpu::Buffer), CoreError> {
        let device = gpu.device();
        let max = device.limits().max_texture_dimension_2d;
        if out.width > max || out.height > max {
            return Err(too_large("output", out, max));
        }
        let acc_key = Key {
            width: out.width,
            height: out.height,
            format: ACC,
            usage: ingest::SOURCE_USAGE,
            mips: 1,
        };
        let acc = [
            self.pool.acquire(device, acc_key),
            self.pool.acquire(device, acc_key),
        ];
        let mut f = Frame {
            gpu,
            enc: device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default()),
            acc,
            cur: 0,
            acc_key,
            resolution: plan.resolution,
            out,
        };
        {
            let v = view(&f.acc[0]);
            f.enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &v,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        let skipped = walk(plan, src, |l, frame| self.draw(&mut f, l, frame))?;
        let target = self.pool.acquire(
            device,
            Key {
                width: out.width,
                height: out.height,
                format: OUT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                mips: 1,
            },
        );
        let p = self.pipelines.get(device, Stage::Output);
        let acc_view = view(&f.acc[f.cur]);
        // `fs_output` reads `acc` only (binding 1 of blend.wgsl).
        let g = bind_from(
            device,
            &p,
            1,
            vec![wgpu::BindingResource::TextureView(&acc_view)],
        );
        pass(
            &mut f.enc,
            &target,
            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            &p,
            &g,
            None,
        );
        let buffer = self.readback.buffer(device, out.width, out.height);
        readback::copy(&mut f.enc, &target, &buffer, out.width, out.height);
        gpu.queue().submit([f.enc.finish()]);
        Ok((skipped, buffer))
    }

    fn draw(&mut self, f: &mut Frame<'_>, l: &Layer, src: &SourceFrame) -> Result<(), CoreError> {
        let Some(p) = place(
            l,
            src.size,
            src.rotation,
            src.pixel_aspect,
            f.resolution,
            f.out,
        ) else {
            return Ok(());
        };
        let Some(edges) = layer::edges(&p.quad) else {
            return Ok(());
        };
        if p.is_empty() {
            return Ok(());
        }
        let (device, queue) = (f.gpu.device(), f.gpu.queue());
        let max = device.limits().max_texture_dimension_2d;
        if src.size.width > max || src.size.height > max {
            return Err(too_large("source picture", src.size, max));
        }
        // Ingest into level 0, then the levels the sample needs.
        let planes = ingest::upload(device, queue, &mut self.pool, src);
        let sizes = chain_sizes(src.size.width, src.size.height, p.level);
        let levels: Vec<wgpu::Texture> = sizes
            .iter()
            .map(|&(w, h)| self.pool.acquire(device, level_key(w, h)))
            .collect();
        let pi = self.pipelines.get(device, ingest::stage(src.format));
        let ub = uniform(f.gpu, &ingest::uniforms(src));
        let pv: Vec<_> = planes.iter().map(view).collect();
        let mut res = vec![ub.as_entire_binding()];
        res.extend(pv.iter().map(wgpu::BindingResource::TextureView));
        let g = bind(device, &pi, res);
        pass(
            &mut f.enc,
            &levels[0],
            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            &pi,
            &g,
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
            pass(
                &mut f.enc,
                &levels[k],
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                &pm,
                &g,
                None,
            );
        }
        let (lo, hi, frac) = pick(levels.len(), p.level);
        // Alpha, and whether a dissolve needs the backdrop.
        let opacity = l.opacity.clamp(0.0, 1.0) as f32;
        let (alpha, dissolve) = match l.transition.as_ref().filter(|t| t.role == Role::In) {
            None => (opacity, None),
            Some(t) => {
                let w = weight(t.curve, t.progress);
                match t.kind {
                    TransitionKind::Crossfade => (opacity * w, None),
                    TransitionKind::Dissolve => (opacity, Some(w)),
                }
            }
        };
        let fused = match (self.shader_blends, dissolve, blend::fused(l.blend)) {
            (false, None, Some(_)) => Some(blend::index(l.blend)),
            _ => None,
        };
        let pl = self.pipelines.get(device, Stage::Layer(fused));
        let ub = uniform(f.gpu, &layer::uniforms(&p, &edges, frac, alpha));
        let (lv, hv) = (view(&levels[lo]), view(&levels[hi]));
        let g = bind(
            device,
            &pl,
            vec![
                ub.as_entire_binding(),
                wgpu::BindingResource::TextureView(&lv),
                wgpu::BindingResource::TextureView(&hv),
                wgpu::BindingResource::Sampler(&self.sampler),
            ],
        );
        let [x0, y0, x1, y1] = p.bounds;
        let scissor = Some([x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32]);
        if fused.is_some() {
            pass(
                &mut f.enc,
                &f.acc[f.cur],
                wgpu::LoadOp::Load,
                &pl,
                &g,
                scissor,
            );
            return Ok(());
        }
        let scratch = self.pool.acquire(device, f.acc_key);
        let clear = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);
        pass(&mut f.enc, &scratch, clear, &pl, &g, scissor);
        let pb = self
            .pipelines
            .get(device, Stage::Blend(blend::index(l.blend)));
        let ub = uniform(
            f.gpu,
            &[
                dissolve.unwrap_or(0.0),
                if dissolve.is_some() { 1.0 } else { 0.0 },
                0.0,
                0.0,
            ],
        );
        let (av, sv) = (view(&f.acc[f.cur]), view(&scratch));
        let g = bind(
            device,
            &pb,
            vec![
                ub.as_entire_binding(),
                wgpu::BindingResource::TextureView(&av),
                wgpu::BindingResource::TextureView(&sv),
            ],
        );
        let next = 1 - f.cur;
        pass(
            &mut f.enc,
            &f.acc[next],
            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            &pb,
            &g,
            None,
        );
        f.cur = next;
        Ok(())
    }
}
