// One clip layer (spec §5.1 steps 4–6): the inverse matrix maps the output pixel centre to
// the source, sampled bilinearly (clamped to the edge) in mip levels ⌊L⌋ and ⌈L⌉ and mixed by
// the fraction of the explicit level L (never a derivative LOD). Each level is its own texture,
// sized as the reference's chain (⌈w/2⌉ × ⌈h/2⌉; WebGPU's own levels round down). Analytic edge
// coverage and opacity give alpha. The output is premultiplied: fused with a fixed-function
// blend onto the accumulation, or written to a scratch target for blend.wgsl. Every number here
// comes from Rust (placement, coverage edges, level).

struct Layer {
    // Output pixels to coded-normalised source coordinates: row 0, then the level fraction.
    inv0: vec4f,
    // Row 1, then alpha (opacity × the crossfade weight).
    inv1: vec4f,
    // The cropped quad's edges: (nx, ny, ax, ay), signed distance nx·(x − ax) + ny·(y − ay),
    // positive inside.
    edges: array<vec4f, 4>,
}

@group(0) @binding(0) var<uniform> u: Layer;
@group(0) @binding(1) var level_lo: texture_2d<f32>;
@group(0) @binding(2) var level_hi: texture_2d<f32>;
@group(0) @binding(3) var bilinear: sampler;

@fragment
fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let p = vec3f(pos.xy, 1.0);
    var cov = 1.0;
    for (var i = 0; i < 4; i++) {
        let e = u.edges[i];
        let d = e.x * (pos.x - e.z) + e.y * (pos.y - e.w);
        cov *= clamp(d + 0.5, 0.0, 1.0);
    }
    let uv = vec2f(dot(u.inv0.xyz, p), dot(u.inv1.xyz, p));
    let a_lo = textureSampleLevel(level_lo, bilinear, uv, 0.0).rgb;
    let a_hi = textureSampleLevel(level_hi, bilinear, uv, 0.0).rgb;
    let cs = a_lo + (a_hi - a_lo) * u.inv0.w;
    let a = cov * u.inv1.w;
    return vec4f(cs * a, a);
}
