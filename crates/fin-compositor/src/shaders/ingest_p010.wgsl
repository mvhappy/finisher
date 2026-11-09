// P010 for ingest.wgsl: `r16uint` Y and `rg16uint` interleaved UV (no optional features, unlike
// 16-bit normalised formats), the 10-bit code value in each word's high bits.

@group(0) @binding(1) var plane_y: texture_2d<u32>;
@group(0) @binding(2) var plane_uv: texture_2d<u32>;

fn code(k: i32, p: vec2i) -> f32 {
    if k == 0 {
        return f32(textureLoad(plane_y, p, 0).r >> 6u);
    }
    let uv = textureLoad(plane_uv, p, 0);
    if k == 1 {
        return f32(uv.r >> 6u);
    }
    return f32(uv.g >> 6u);
}
