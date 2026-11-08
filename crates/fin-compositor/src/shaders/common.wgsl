// Prepended to every pass (spec §5.1). A full-screen triangle: a pass's scissor rectangle
// limits it to the pixels it may touch. BT.1886 transfer as fin-media's `convert` writes it
// (γ 2.4, black 0, negative inputs 0).

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4f(x, y, 0.0, 1.0);
}

fn to_linear(v: f32) -> f32 {
    if v > 0.0 {
        return pow(v, 2.4);
    }
    return 0.0;
}

fn from_linear(v: f32) -> f32 {
    if v > 0.0 {
        return pow(v, 1.0 / 2.4);
    }
    return 0.0;
}
