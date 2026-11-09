// HDR to SDR and the gamut soft clip (spec §10.1): a line-for-line port of fin-media's
// `colour::softclip` and `colour::tonemap` (`to_sdr` and the transfers before it); T1 holds
// between them. The EETF's constants come from `tonemap::Eetf`, computed in Rust. Uses
// `from_linear` (common.wgsl). Zero inputs to `pow` are answered before the call, as `pow(0, y)`
// is not defined everywhere.

const KNEE: f32 = 0.8;

const PQ_M1: f32 = 2610.0 / 16384.0;
const PQ_M2: f32 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f32 = 3424.0 / 4096.0;
const PQ_C2: f32 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f32 = 2392.0 / 4096.0 * 32.0;
const PQ_PEAK: f32 = 10000.0;

const HLG_A: f32 = 0.17883277;
const HLG_B: f32 = 0.28466892;
const HLG_C: f32 = 0.55991073;
const HLG_PEAK: f32 = 1000.0;
const YS: vec3f = vec3f(0.2627, 0.6780, 0.0593);

struct Eetf {
    black: f32,
    range: f32,
    max_lum: f32,
    min_lum: f32,
    knee: f32,
}

fn softclip(x: f32) -> f32 {
    if x <= KNEE {
        return x;
    }
    let t = (x - KNEE) / (1.0 - KNEE);
    return KNEE + (1.0 - KNEE) / sqrt(1.0 + 1.0 / (t * t));
}

fn pq_eotf(e_in: f32) -> f32 {
    let e = clamp(e_in, 0.0, 1.0);
    if e <= 0.0 {
        return 0.0;
    }
    let p = pow(e, 1.0 / PQ_M2);
    let num = max(p - PQ_C1, 0.0);
    if num <= 0.0 {
        return 0.0;
    }
    let den = PQ_C2 - PQ_C3 * p;
    return PQ_PEAK * pow(num / den, 1.0 / PQ_M1);
}

fn pq_inverse_eotf(nits: f32) -> f32 {
    var y = 0.0;
    if nits > 0.0 {
        y = pow(clamp(nits, 0.0, PQ_PEAK) / PQ_PEAK, PQ_M1);
    }
    return pow((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y), PQ_M2);
}

fn hlg_inverse_oetf(e_in: f32) -> f32 {
    let e = clamp(e_in, 0.0, 1.0);
    if e <= 0.5 {
        return e * e / 3.0;
    }
    return (exp((e - HLG_C) / HLG_A) + HLG_B) / 12.0;
}

fn hlg_ootf(scene: vec3f) -> vec3f {
    let ys = YS.x * scene.x + YS.y * scene.y + YS.z * scene.z;
    if ys <= 0.0 {
        return vec3f(0.0);
    }
    let gain = HLG_PEAK * pow(ys, 0.2);
    return vec3f(gain * scene.x, gain * scene.y, gain * scene.z);
}

fn eetf_apply_pq(c: Eetf, e: f32) -> f32 {
    let e1 = clamp((e - c.black) / c.range, 0.0, 1.0);
    var e2 = e1;
    if c.knee < 1.0 && e1 >= c.knee {
        let t = (e1 - c.knee) / (1.0 - c.knee);
        let t2 = t * t;
        let t3 = t2 * t;
        e2 = (2.0 * t3 - 3.0 * t2 + 1.0) * c.knee
            + (t3 - 2.0 * t2 + t) * (1.0 - c.knee)
            + (-2.0 * t3 + 3.0 * t2) * c.max_lum;
    }
    let r = 1.0 - e2;
    let e3 = e2 + c.min_lum * (r * r * r * r);
    return e3 * c.range + c.black;
}

fn eetf_apply(c: Eetf, nits: f32) -> f32 {
    return pq_eotf(eetf_apply_pq(c, pq_inverse_eotf(nits)));
}

// Linear light through the primaries matrix (rows g0, g1, g2), the soft clip, the BT.1886
// encode (fin-media's `convert::linear_to_709`).
fn linear_to_709(l: vec3f, g0: vec3f, g1: vec3f, g2: vec3f) -> vec3f {
    let r = g0.x * l.x + g0.y * l.y + g0.z * l.z;
    let g = g1.x * l.x + g1.y * l.y + g1.z * l.z;
    let b = g2.x * l.x + g2.y * l.y + g2.z * l.z;
    return vec3f(from_linear(softclip(r)), from_linear(softclip(g)), from_linear(softclip(b)));
}

// Display light in cd/m² to display-encoded Rec.709 SDR (fin-media's `tonemap::to_sdr`).
fn to_sdr(rgb: vec3f, c: Eetf, target_peak: f32, g0: vec3f, g1: vec3f, g2: vec3f) -> vec3f {
    let m = max(max(rgb.x, rgb.y), rgb.z);
    if m <= 0.0 {
        return vec3f(0.0);
    }
    let s = eetf_apply(c, m) / m / target_peak;
    return linear_to_709(vec3f(rgb.x * s, rgb.y * s, rgb.z * s), g0, g1, g2);
}
