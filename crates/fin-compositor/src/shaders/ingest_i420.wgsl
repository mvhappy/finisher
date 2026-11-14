// I420 for ingest.wgsl: three `r8unorm` planes, code values 0…255.

@group(0) @binding(1) var plane_y: texture_2d<f32>;
@group(0) @binding(2) var plane_u: texture_2d<f32>;
@group(0) @binding(3) var plane_v: texture_2d<f32>;

fn code(k: i32, p: vec2i) -> f32 {
    if k == 0 {
        return round(textureLoad(plane_y, p, 0).r * 255.0);
    }
    if k == 1 {
        return round(textureLoad(plane_u, p, 0).r * 255.0);
    }
    return round(textureLoad(plane_v, p, 0).r * 255.0);
}
