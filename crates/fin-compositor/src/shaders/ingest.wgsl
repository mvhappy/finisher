// Ingest (spec §5.1 step 2): Y′CbCr 4:2:0 code values to display-encoded R′G′B′ in an
// `rgba16float` texture of the coded size: bilinear chroma at the source's siting (clamped to
// the edge), range expansion, the source matrix, then the conversion to the Rec.709 SDR
// working space (`tonemap.wgsl`): the wide-gamut matrix and gamut compression, or the HDR tone mapping.
// The coefficients are fin-media's, passed from Rust. A layout prefix (`ingest_*.wgsl`) declares
// the planes at bindings 1… and `code(k, p)`: plane k's (0 Y, 1 Cb, 2 Cr) code value at texel p.

struct Ingest {
    // Chroma plane width and height; the chroma siting offset in luma pixels (x, y).
    chroma: vec4f,
    // Y′ offset and scale, the chroma midpoint and scale (code values).
    range: vec4f,
    // Kr, Kb; 1 for the identity (GBR) matrix; the conversion: 0 none, 1 wide-gamut SDR,
    // 2 PQ, 3 HLG (`tonemap::Conversion`).
    coef: vec4f,
    // The linear-light primaries conversion, row by row; each row's w is that channel's
    // compression threshold.
    gamut0: vec4f,
    gamut1: vec4f,
    gamut2: vec4f,
    // The EETF (`tonemap::Eetf`): black, range, max_lum, min_lum; then knee, the target peak.
    tone0: vec4f,
    tone1: vec4f,
    // The compression scales (`softclip::Compression`), per channel.
    comp: vec4f,
}

@group(0) @binding(0) var<uniform> u: Ingest;

// Chroma plane k's code value at a clamped texel.
fn code_at(k: i32, x: i32, y: i32) -> f32 {
    let w = i32(u.chroma.x);
    let h = i32(u.chroma.y);
    let p = vec2i(clamp(x, 0, w - 1), clamp(y, 0, h - 1));
    return code(k, p);
}

// Bilinear at chroma coordinates (fx, fy), in the reference's order.
fn chroma_at(k: i32, fx: f32, fy: f32) -> f32 {
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let a = code_at(k, xi, yi);
    let b = code_at(k, xi + 1, yi);
    let c = code_at(k, xi, yi + 1);
    let d = code_at(k, xi + 1, yi + 1);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    return top + (bottom - top) * ty;
}

@fragment
fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let x = floor(pos.x);
    let y = floor(pos.y);
    let yv = code(0, vec2i(i32(x), i32(y)));
    let cb = chroma_at(1, (x - u.chroma.z) / 2.0, (y - u.chroma.w) / 2.0);
    let cr = chroma_at(2, (x - u.chroma.z) / 2.0, (y - u.chroma.w) / 2.0);
    var rgb: vec3f;
    if u.coef.z > 0.5 {
        // Identity: Y is G, Cb is B, Cr is R, each expanded as luma.
        rgb = vec3f(
            (cr - u.range.x) / u.range.y,
            (yv - u.range.x) / u.range.y,
            (cb - u.range.x) / u.range.y,
        );
    } else {
        let l = (yv - u.range.x) / u.range.y;
        let pb = (cb - u.range.z) / u.range.w;
        let pr = (cr - u.range.z) / u.range.w;
        let kr = u.coef.x;
        let kb = u.coef.y;
        let r = l + 2.0 * (1.0 - kr) * pr;
        let b = l + 2.0 * (1.0 - kb) * pb;
        let g = (l - kr * r - kb * b) / (1.0 - kr - kb);
        rgb = vec3f(r, g, b);
    }
    let g0 = u.gamut0.xyz;
    let g1 = u.gamut1.xyz;
    let g2 = u.gamut2.xyz;
    let k = Compression(vec3f(u.gamut0.w, u.gamut1.w, u.gamut2.w), u.comp.xyz);
    let mode = u.coef.w;
    if mode > 0.5 && mode < 1.5 {
        rgb = linear_to_709(vec3f(to_linear(rgb.r), to_linear(rgb.g), to_linear(rgb.b)), g0, g1, g2, k);
    } else if mode > 1.5 {
        let c = Eetf(u.tone0.x, u.tone0.y, u.tone0.z, u.tone0.w, u.tone1.x);
        var nits: vec3f;
        if mode < 2.5 {
            nits = vec3f(pq_eotf(rgb.r), pq_eotf(rgb.g), pq_eotf(rgb.b));
        } else {
            nits = hlg_ootf(vec3f(hlg_inverse_oetf(rgb.r), hlg_inverse_oetf(rgb.g), hlg_inverse_oetf(rgb.b)));
        }
        rgb = to_sdr(nits, c, u.tone1.y, g0, g1, g2, k);
    }
    return vec4f(rgb, 1.0);
}
