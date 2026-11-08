// One mip level (spec §5.1): each texel the exact 2×2 box average of the level above, an odd
// last column or row averaged with itself. WebGPU generates no mipmaps.

@group(0) @binding(0) var above: texture_2d<f32>;

@fragment
fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let size = vec2i(textureDimensions(above));
    let x = i32(floor(pos.x));
    let y = i32(floor(pos.y));
    let x0 = 2 * x;
    let x1 = min(2 * x + 1, size.x - 1);
    let y0 = 2 * y;
    let y1 = min(2 * y + 1, size.y - 1);
    let a = textureLoad(above, vec2i(x0, y0), 0);
    let b = textureLoad(above, vec2i(x1, y0), 0);
    let c = textureLoad(above, vec2i(x0, y1), 0);
    let d = textureLoad(above, vec2i(x1, y1), 0);
    return ((a + b) + (c + d)) * 0.25;
}
