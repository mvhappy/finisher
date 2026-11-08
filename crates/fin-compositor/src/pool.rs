//! The texture pool (spec §5.1): intermediates keyed by (size, format, usage, mip levels),
//! handed out at most once per frame and recycled when the frame ends, so a steady sequence of
//! frames allocates nothing after its first.

use std::collections::HashMap;

/// What a pooled texture is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    pub usage: wgpu::TextureUsages,
    pub mips: u32,
}

/// How many textures the pool owns, and their bytes (base level ×4/3 for mipped ones).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PoolStats {
    pub textures: usize,
    pub bytes: u64,
}

#[derive(Default)]
pub(crate) struct Pool {
    free: HashMap<Key, Vec<wgpu::Texture>>,
    in_use: Vec<(Key, wgpu::Texture)>,
    stats: PoolStats,
}

fn bytes(k: &Key) -> u64 {
    let texel = k.format.block_copy_size(None).unwrap_or(4) as u64;
    let base = u64::from(k.width) * u64::from(k.height) * texel;
    if k.mips > 1 { base * 4 / 3 } else { base }
}

impl Pool {
    /// A texture for `key` no one else holds this frame.
    pub fn acquire(&mut self, device: &wgpu::Device, key: Key) -> wgpu::Texture {
        let t = match self.free.get_mut(&key).and_then(Vec::pop) {
            Some(t) => t,
            None => {
                self.stats.textures += 1;
                self.stats.bytes += bytes(&key);
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("fin-compositor pool"),
                    size: wgpu::Extent3d {
                        width: key.width,
                        height: key.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: key.mips,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: key.format,
                    usage: key.usage,
                    view_formats: &[],
                })
            }
        };
        self.in_use.push((key, t.clone()));
        t
    }

    /// Returns every texture handed out since the last call.
    pub fn end_frame(&mut self) {
        for (k, t) in self.in_use.drain(..) {
            self.free.entry(k).or_default().push(t);
        }
    }

    /// Drops every texture (after a lost device, whose textures are useless).
    pub fn clear(&mut self) {
        self.free.clear();
        self.in_use.clear();
        self.stats = PoolStats::default();
    }

    pub fn stats(&self) -> PoolStats {
        self.stats
    }
}
