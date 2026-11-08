// Shader blends (spec §5.1 step 6, step 8): `multiply`, `overlay`, `darken`, `lighten`, and
// any mode under a dissolve, read the accumulation and the layer's scratch target (premultiplied)
// and write the other ping-pong target. Over the opaque frame: cb·(1 − a) + a·B(cb, cs), `add`
// as cb + a·cs; a dissolve then mixes with the backdrop in linear light. `fs_output` converts
// the accumulation for the 8-bit output target.

// The blend mode, per pipeline: 0 normal, 1 multiply, 2 screen, 3 overlay, 4 add, 5 darken,
// 6 lighten.
override MODE: u32 = 0u;

struct Blend {
    // The dissolve weight, and 1 under a dissolve.
    params: vec4f,
}

@group(0) @binding(0) var<uniform> u: Blend;
@group(0) @binding(1) var acc: texture_2d<f32>;
@group(0) @binding(2) var layer: texture_2d<f32>;

// One assignment per case and a single return: with `MODE` substituted, FXC (D3D12) rejects
// returns inside the switch ("X3507: not all control paths return a value").
fn mode_of(cb: f32, cs: f32) -> f32 {
    var r = cs;
    switch MODE {
        case 1u: {
            r = cb * cs;
        }
        case 2u: {
            r = cb + cs - cb * cs;
        }
        case 3u: {
            r = select(1.0 - 2.0 * (1.0 - cb) * (1.0 - cs), 2.0 * cb * cs, cb <= 0.5);
        }
        case 5u: {
            r = min(cb, cs);
        }
        case 6u: {
            r = max(cb, cs);
        }
        default: {}
    }
    return r;
}

fn mix_linear(d: f32, b: f32, w: f32) -> f32 {
    let dl = to_linear(d);
    return from_linear(dl + w * (to_linear(b) - dl));
}

@fragment
fn fs_blend(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let ip = vec2i(floor(pos.xy));
    let dst = textureLoad(acc, ip, 0).rgb;
    let l = textureLoad(layer, ip, 0);
    let a = l.a;
    // Nothing of the layer here; also the guard of the un-premultiplying division.
    if a <= 0.0 {
        return vec4f(dst, 1.0);
    }
    let cs = l.rgb / a;
    var out: vec3f;
    if MODE == 4u {
        out = dst + a * cs;
    } else {
        let b = vec3f(mode_of(dst.r, cs.r), mode_of(dst.g, cs.g), mode_of(dst.b, cs.b));
        out = dst * (1.0 - a) + a * b;
    }
    if u.params.y > 0.5 {
        let w = u.params.x;
        out = vec3f(mix_linear(dst.r, out.r, w), mix_linear(dst.g, out.g, w), mix_linear(dst.b, out.b, w));
    }
    return vec4f(out, 1.0);
}

@fragment
fn fs_output(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let c = textureLoad(acc, vec2i(floor(pos.xy)), 0).rgb;
    return vec4f(clamp(c, vec3f(0.0), vec3f(1.0)), 1.0);
}
