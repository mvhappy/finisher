// NV12 for ingest.wgsl: `r8unorm` Y and `rg8unorm` interleaved UV, code values 0…255.

@group(0) @binding(1) var plane_y: texture_2d<f32>;
@group(0) @binding(2) var plane_uv: texture_2d<f32>;

fn code(k: i32, p: vec2i) -> f32 {
    if k == 0 {
        return round(textureLoad(plane_y, p, 0).r * 255.0);
    }
    let uv = textureLoad(plane_uv, p, 0);
    if k == 1 {
        return round(uv.r * 255.0);
    }
    return round(uv.g * 255.0);
}
