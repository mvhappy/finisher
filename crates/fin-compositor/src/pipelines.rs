//! The render pipelines of the wgpu compositor (spec §5.1): one per (stage, blend mode), made
//! on first use from the shader modules built at start. Every module is `common.wgsl` followed
//! by its body; the ingest modules also carry `tonemap.wgsl` (the colour conversion, spec
//! §10.1) and their layout's fetch prefix.

use std::collections::HashMap;

use crate::blend;
use crate::gpu::{ACC, OUT};
use crate::ingest;
use crate::scale::gpu as scale;

const COMMON: &str = include_str!("shaders/common.wgsl");

/// A pipeline's identity: one per (stage, blend mode); every target format is fixed by stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Ingest of I420 (`IngestNv12`, `IngestP010`: of the platform decoders' layouts).
    Ingest,
    IngestNv12,
    IngestP010,
    Mip,
    /// The layer pass, fused with the blend of mode `Some(index)`, or to a scratch target.
    Layer(Option<u32>),
    /// `blend.wgsl` for mode `index` (`blend::index`).
    Blend(u32),
    Output,
    /// Proxies and thumbnails (`scale.wgsl`): bilinear from one box level, then NV12's planes.
    Scale,
    Luma,
    Chroma,
}

pub(crate) struct Pipelines {
    /// By `ingest::SHADERS` index.
    ingest: [wgpu::ShaderModule; 3],
    mip: wgpu::ShaderModule,
    layer: wgpu::ShaderModule,
    blend: wgpu::ShaderModule,
    scale: wgpu::ShaderModule,
    pub(crate) map: HashMap<Stage, wgpu::RenderPipeline>,
    pub(crate) created: usize,
}

fn module(device: &wgpu::Device, label: &str, body: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(format!("{COMMON}\n{body}").into()),
    })
}

impl Pipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Pipelines {
        Pipelines {
            ingest: ingest::SHADERS.map(|(l, fetch)| {
                module(device, l, &[ingest::TONEMAP, fetch, ingest::BODY].concat())
            }),
            mip: module(device, "mip", include_str!("shaders/mip.wgsl")),
            layer: module(device, "layer", include_str!("shaders/layer.wgsl")),
            blend: module(device, "blend", include_str!("shaders/blend.wgsl")),
            scale: module(device, "scale", include_str!("shaders/scale.wgsl")),
            map: HashMap::new(),
            created: 0,
        }
    }

    pub(crate) fn get(&mut self, device: &wgpu::Device, stage: Stage) -> wgpu::RenderPipeline {
        if let Some(p) = self.map.get(&stage) {
            return p.clone();
        }
        let (module, entry, format, blend, constants): (_, _, _, _, &[(&str, f64)]) = match stage {
            Stage::Ingest | Stage::IngestNv12 | Stage::IngestP010 => {
                let m = &self.ingest[ingest::shader_index(stage)];
                (m, "fs_main", ACC, None, &[])
            }
            Stage::Mip => (&self.mip, "fs_main", ACC, None, &[]),
            Stage::Layer(fused) => (
                &self.layer,
                "fs_main",
                ACC,
                fused.and_then(|m| blend::fused(blend::MODES[m as usize])),
                &[],
            ),
            Stage::Blend(m) => (
                &self.blend,
                "fs_blend",
                ACC,
                None,
                &[("MODE", f64::from(m))],
            ),
            Stage::Output => (&self.blend, "fs_output", OUT, None, &[]),
            Stage::Scale => (&self.scale, "fs_scale", scale::SCALED, None, &[]),
            Stage::Luma => (&self.scale, "fs_luma", scale::LUMA, None, &[]),
            Stage::Chroma => (&self.scale, "fs_chroma", scale::CHROMA, None, &[]),
        };
        let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(entry),
            layout: None,
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants,
                    ..Default::default()
                },
            }),
            multiview_mask: None,
            cache: None,
        });
        self.created += 1;
        self.map.insert(stage, p.clone());
        p
    }
}
