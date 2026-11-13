//! Deterministic test signals (mono `f32`; no `rand`).

use crate::Stereo;

const TAU: f64 = 2.0 * core::f64::consts::PI;

fn frames(secs: f64, rate: u32) -> usize {
    libm::round(secs * f64::from(rate)) as usize
}

/// `10^(db/20)`.
pub fn db_to_amp(db: f64) -> f64 {
    libm::pow(10.0, db / 20.0)
}

/// A sine of peak level `dbfs`; the phase is computed from the index, never accumulated.
pub fn sine(hz: f64, dbfs: f64, secs: f64, rate: u32) -> Vec<f32> {
    let a = db_to_amp(dbfs);
    (0..frames(secs, rate))
        .map(|n| (a * libm::sin(TAU * hz * n as f64 / f64::from(rate))) as f32)
        .collect()
}

/// A logarithmic sweep from `from_hz` to `to_hz` over `secs`, unit amplitude.
pub fn sweep(from_hz: f64, to_hz: f64, secs: f64, rate: u32) -> Vec<f32> {
    let k = libm::log(to_hz / from_hz);
    (0..frames(secs, rate))
        .map(|n| {
            let t = n as f64 / f64::from(rate);
            let phase = TAU * from_hz * secs / k * (libm::exp(t * k / secs) - 1.0);
            libm::sin(phase) as f32
        })
        .collect()
}

/// `len` frames of silence with a 1.0 at index `at` (if inside).
pub fn impulse(at: usize, len: usize) -> Vec<f32> {
    let mut v = vec![0.0; len];
    if at < len {
        v[at] = 1.0;
    }
    v
}

/// `secs` of silence.
pub fn silence(secs: f64, rate: u32) -> Vec<f32> {
    vec![0.0; frames(secs, rate)]
}

/// A `total` long signal of silence holding a sine of `burst` seconds from `start`.
pub fn tone_burst(hz: f64, dbfs: f64, start: f64, burst: f64, total: f64, rate: u32) -> Vec<f32> {
    let mut v = silence(total, rate);
    let (s, tone) = (frames(start, rate), sine(hz, dbfs, burst, rate));
    for (i, x) in tone.into_iter().enumerate() {
        if let Some(d) = v.get_mut(s + i) {
            *d = x;
        }
    }
    v
}

/// xorshift64* (Vigna); a zero seed is replaced.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[-1, 1)` on a 24-bit grid.
    pub fn uniform(&mut self) -> f32 {
        let top = (self.next_u64() >> 40) as i64; // 24 bits
        ((top - (1 << 23)) as f64 / f64::from(1u32 << 23)) as f32
    }
}

/// Uniform white noise in `[-1, 1)`.
pub fn noise(seed: u64, secs: f64, rate: u32) -> Vec<f32> {
    let mut r = Rng::new(seed);
    (0..frames(secs, rate)).map(|_| r.uniform()).collect()
}

/// Pink noise (Voss-McCartney, 12 rows), scaled to peak well inside `[-1, 1]`.
pub fn pink(seed: u64, secs: f64, rate: u32) -> Vec<f32> {
    const ROWS: usize = 12;
    let mut r = Rng::new(seed);
    let mut rows = [0.0f32; ROWS];
    let mut sum = 0.0f32;
    for row in &mut rows {
        *row = r.uniform();
        sum += *row;
    }
    (0..frames(secs, rate))
        .map(|n| {
            // Row `k` refreshes when the lowest set bit of the counter is `k`.
            let k = (((n + 1) as u64).trailing_zeros() as usize).min(ROWS - 1);
            sum -= rows[k];
            rows[k] = r.uniform();
            sum += rows[k];
            (r.uniform() + sum) / (ROWS + 1) as f32
        })
        .collect()
}

fn stereo_of(mono: Vec<f32>) -> Stereo {
    Stereo {
        l: mono.clone(),
        r: mono,
    }
}

fn segments(parts: &[(f64, f64)]) -> Stereo {
    let mut out = Vec::new();
    for &(secs, lufs) in parts {
        // A 1 kHz sine at the same peak level on both channels measures that many LUFS.
        out.extend(sine(1000.0, lufs, secs, crate::RATE));
    }
    stereo_of(out)
}

/// EBU Tech 3341 stereo sequences: a 1 kHz sine at the same peak level on both channels, whose
/// dBFS equals its LUFS. Case 1: 20 s at -23 dBFS (expected integrated -23 LUFS); 2: 20 s at -33
/// (-33); 3: -36/-23/-36 for 10/60/10 s (-23); 4: -72/-36/-23/-36/-72 for 10/10/20/10/10 s
/// (-23); 5: -26/-20/-26 for 20/20.1/20 s (-23). Cases built from programme material (and the
/// multichannel ones) are `None`.
pub fn ebu3341_case(n: u32) -> Option<Stereo> {
    let parts: &[(f64, f64)] = match n {
        1 => &[(20.0, -23.0)],
        2 => &[(20.0, -33.0)],
        3 => &[(10.0, -36.0), (60.0, -23.0), (10.0, -36.0)],
        4 => &[
            (10.0, -72.0),
            (10.0, -36.0),
            (20.0, -23.0),
            (10.0, -36.0),
            (10.0, -72.0),
        ],
        5 => &[(20.0, -26.0), (20.1, -20.0), (20.0, -26.0)],
        _ => return None,
    };
    Some(segments(parts))
}

/// EBU Tech 3342 loudness-range sequences: 1 kHz stereo sines, 20 s per segment. Case 1: -20 then
/// -30 dBFS (expected LRA 10 LU); 2: -20 then -15 (5 LU); 3: -40 then -20 (20 LU); 4:
/// -50/-35/-20/-35/-50 (15 LU). Other cases are `None`.
pub fn ebu3342_case(n: u32) -> Option<Stereo> {
    let levels: &[f64] = match n {
        1 => &[-20.0, -30.0],
        2 => &[-20.0, -15.0],
        3 => &[-40.0, -20.0],
        4 => &[-50.0, -35.0, -20.0, -35.0, -50.0],
        _ => return None,
    };
    let parts: Vec<(f64, f64)> = levels.iter().map(|&l| (20.0, l)).collect();
    Some(segments(&parts))
}
