// Scaling for proxies and thumbnails (plan 6B-1 Task 14), as `reference::scale` does it:
// `fs_scale` samples one box level bilinearly (texel centres, clamped to the edge) into an
// `rgba32float` picture of the output size; `fs_luma` and `fs_chroma` write its BT.709
// limited-range NV12 planes (`nv12.rs`: left-sited chroma, ¼ ½ ¼ across, the two rows' mean).

struct Scale {
    // The level's width and height, the output's width and height.
    sizes: vec4f,
}

@group(0) @binding(0) var<uniform> u: Scale;
@group(0) @binding(1) var level: texture_2d<f32>;
@group(0) @binding(2) var scaled: texture_2d<f32>;

fn texel(x: i32, y: i32) -> vec3f {
    let size = vec2i(textureDimensions(level));
    let p = clamp(vec2i(x, y), vec2i(0, 0), size - vec2i(1, 1));
    return textureLoad(level, p, 0).rgb;
}

@fragment
fn fs_scale(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let uv = (floor(pos.xy) + vec2f(0.5, 0.5)) / u.sizes.zw;
    let f = uv * u.sizes.xy - vec2f(0.5, 0.5);
    let f0 = floor(f);
    let t = f - f0;
    let x = i32(f0.x);
    let y = i32(f0.y);
    let a = texel(x, y);
    let b = texel(x + 1, y);
    let c = texel(x, y + 1);
    let d = texel(x + 1, y + 1);
    let top = a + (b - a) * t.x;
    let bottom = c + (d - c) * t.x;
    return vec4f(top + (bottom - top) * t.y, 1.0);
}

const KR: f32 = 0.2126;
const KB: f32 = 0.0722;
const KG: f32 = 1.0 - KR - KB;

fn rgb_at(x: i32, y: i32) -> vec3f {
    let size = vec2i(textureDimensions(scaled));
    let p = clamp(vec2i(x, y), vec2i(0, 0), size - vec2i(1, 1));
    return clamp(textureLoad(scaled, p, 0).rgb, vec3f(0.0), vec3f(1.0));
}

fn luma(c: vec3f) -> f32 {
    return KR * c.r + KG * c.g + KB * c.b;
}

// A code as an 8-bit unorm value: rounded half to even, so the store is exact.
fn code8(code: f32) -> f32 {
    return round(clamp(code, 0.0, 255.0)) / 255.0;
}

@fragment
fn fs_luma(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let c = rgb_at(i32(floor(pos.x)), i32(floor(pos.y)));
    return vec4f(code8(16.0 + 219.0 * luma(c)), 0.0, 0.0, 1.0);
}

@fragment
fn fs_chroma(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let x = 2 * i32(floor(pos.x));
    let y0 = 2 * i32(floor(pos.y));
    let size = vec2i(textureDimensions(scaled));
    let y1 = min(y0 + 1, size.y - 1);
    let xl = max(x - 1, 0);
    let xr = min(x + 1, size.x - 1);
    let r0 = 0.25 * rgb_at(xl, y0) + 0.5 * rgb_at(x, y0) + 0.25 * rgb_at(xr, y0);
    let r1 = 0.25 * rgb_at(xl, y1) + 0.5 * rgb_at(x, y1) + 0.25 * rgb_at(xr, y1);
    let c = (r0 + r1) * 0.5;
    let y = luma(c);
    let pb = (c.b - y) / (2.0 * (1.0 - KB));
    let pr = (c.r - y) / (2.0 * (1.0 - KR));
    return vec4f(code8(128.0 + 224.0 * pb), code8(128.0 + 224.0 * pr), 0.0, 1.0);
}
