//! Readback (spec §5.2): the 8-bit output texture is copied into a staging buffer with rows
//! padded to 256 bytes, mapped, and the padding stripped. One buffer is kept and reused while
//! the output size stays the same.

use fin_core::CoreError;

use crate::device::{Gpu, device_lost};

/// Bytes per padded row for `width` RGBA8 pixels.
pub(crate) fn padded_row(width: u32) -> u32 {
    padded(width, 4)
}

/// Bytes per padded row for `width` texels of `bpp` bytes.
pub(crate) fn padded(width: u32, bpp: u32) -> u32 {
    (width * bpp).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
}

/// A one-off staging buffer for a `width × height` plane of `bpp`-byte texels.
pub(crate) fn staging(device: &wgpu::Device, width: u32, height: u32, bpp: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fin-compositor plane readback"),
        size: u64::from(padded(width, bpp)) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}

#[derive(Default)]
pub(crate) struct Readback {
    buffer: Option<(u64, wgpu::Buffer)>,
}

impl Readback {
    /// The staging buffer for a `width × height` output.
    pub fn buffer(&mut self, device: &wgpu::Device, width: u32, height: u32) -> wgpu::Buffer {
        let size = u64::from(padded_row(width)) * u64::from(height);
        match &self.buffer {
            Some((s, b)) if *s == size => b.clone(),
            _ => {
                let b = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("fin-compositor readback"),
                    size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                self.buffer = Some((size, b.clone()));
                b
            }
        }
    }

    pub fn clear(&mut self) {
        self.buffer = None;
    }
}

/// Records the copy of `texture` into `buffer`.
pub(crate) fn copy(
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    buffer: &wgpu::Buffer,
    width: u32,
    height: u32,
) {
    copy_bytes(encoder, texture, buffer, width, height, 4);
}

/// Records the copy of `texture` (texels of `bpp` bytes) into `buffer`.
pub(crate) fn copy_bytes(
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    buffer: &wgpu::Buffer,
    width: u32,
    height: u32,
    bpp: u32,
) {
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded(width, bpp)),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// Maps `buffer` after the submission, waits, and returns the unpadded rows.
pub(crate) fn read(
    gpu: &Gpu,
    buffer: &wgpu::Buffer,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, CoreError> {
    read_bytes(gpu, buffer, width, height, 4)
}

/// [`read`] of texels of `bpp` bytes.
pub(crate) fn read_bytes(
    gpu: &Gpu,
    buffer: &wgpu::Buffer,
    width: u32,
    height: u32,
    bpp: u32,
) -> Result<Vec<u8>, CoreError> {
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    if let Err(e) = gpu.device().poll(wgpu::PollType::wait_indefinitely()) {
        gpu.check_lost()?;
        return Err(device_lost(&e.to_string()));
    }
    match rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            gpu.check_lost()?;
            return Err(CoreError::new(
                "INTERNAL",
                format!("GPU readback failed: {e}"),
            ));
        }
        Err(_) => {
            gpu.check_lost()?;
            return Err(device_lost("the readback never completed"));
        }
    }
    let row = padded(width, bpp) as usize;
    let len = (width * bpp) as usize;
    let mut out = Vec::with_capacity(len * height as usize);
    {
        let view = buffer
            .get_mapped_range(..)
            .map_err(|e| CoreError::new("INTERNAL", format!("GPU readback failed: {e}")))?;
        for y in 0..height as usize {
            out.extend_from_slice(&view[y * row..y * row + len]);
        }
    }
    buffer.unmap();
    Ok(out)
}
