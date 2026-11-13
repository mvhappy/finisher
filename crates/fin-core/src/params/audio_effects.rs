//! The built-in audio effects (sub-project 6 spec §8.4, plan 6C Decision 13): `eq`,
//! `compressor`, `gate` and `deEsser`, each with `enabled`. They go on clips of audio tracks
//! only ([`super::effects::effect_domain`] is `audio`; `effect.add` refuses them elsewhere).
//!
//! The registry's one-level rule for effect params forbids dotted names, so the EQ's eight
//! bands are flat: `band<n>Type` (not animatable), `band<n>Freq`, `band<n>Gain` and `band<n>Q`.
//! Numbers are animatable; enums are not. The DSP behind them is `fin-audio`'s.

use crate::params::effects::{enabled, number, spec};
use crate::params::{DefaultValue, ParamSpec, ParamType};

/// The audio effect kinds, in registry order.
pub(super) const KINDS: &[&str] = &["eq", "compressor", "gate", "deEsser"];

const BAND_TYPES: &[&str] = &[
    "off",
    "lowShelf",
    "highShelf",
    "peak",
    "lowPass",
    "highPass",
];

const fn choice(
    path: &'static str,
    kind: &'static str,
    values: &'static [&'static str],
    default: &'static str,
    fix: &'static str,
) -> ParamSpec {
    spec(
        path,
        kind,
        ParamType::Enum { values },
        false,
        DefaultValue::Text(default),
        None,
        fix,
    )
}

/// The EQ: `enabled`, then per band its type, frequency, gain and Q (Decision 13's default
/// frequencies, octave-ish from 80 Hz to 10 kHz; every band starts `off`).
macro_rules! eq_specs {
    ($($band:literal => $freq:literal),+ $(,)?) => {
        pub(super) const EQ: &[ParamSpec] = &[
            enabled("eq.enabled", "eq"),
            $(
                choice(
                    concat!("eq.band", $band, "Type"),
                    "eq",
                    BAND_TYPES,
                    "off",
                    "use a band type: off, lowShelf, highShelf, peak, lowPass or highPass",
                ),
                number(
                    concat!("eq.band", $band, "Freq"),
                    "eq",
                    20.0,
                    20_000.0,
                    $freq,
                    Some("Hz"),
                    "use a band frequency from {min} to {max} {unit}",
                ),
                number(
                    concat!("eq.band", $band, "Gain"),
                    "eq",
                    -24.0,
                    24.0,
                    0.0,
                    Some("dB"),
                    "use a band gain from {min} to {max} {unit}",
                ),
                number(
                    concat!("eq.band", $band, "Q"),
                    "eq",
                    0.1,
                    18.0,
                    0.707,
                    None,
                    "use a band Q from {min} to {max}",
                ),
            )+
        ];
    };
}

eq_specs!(
    1 => 80.0,
    2 => 160.0,
    3 => 315.0,
    4 => 630.0,
    5 => 1250.0,
    6 => 2500.0,
    7 => 5000.0,
    8 => 10_000.0,
);

/// The compressor, gate and de-esser.
pub(super) const DYNAMICS: &[ParamSpec] = &[
    enabled("compressor.enabled", "compressor"),
    choice(
        "compressor.detector",
        "compressor",
        &["rms", "peak"],
        "rms",
        "use a detector: rms or peak",
    ),
    number(
        "compressor.threshold",
        "compressor",
        -60.0,
        0.0,
        -18.0,
        Some("dB"),
        "use a threshold from {min} to {max} {unit}",
    ),
    number(
        "compressor.ratio",
        "compressor",
        1.0,
        20.0,
        4.0,
        None,
        "use a ratio from {min} to {max} (1 leaves the level unchanged)",
    ),
    number(
        "compressor.knee",
        "compressor",
        0.0,
        24.0,
        6.0,
        Some("dB"),
        "use a knee from {min} to {max} {unit}",
    ),
    number(
        "compressor.attack",
        "compressor",
        0.1,
        200.0,
        10.0,
        Some("ms"),
        "use an attack from {min} to {max} {unit}",
    ),
    number(
        "compressor.release",
        "compressor",
        5.0,
        2000.0,
        100.0,
        Some("ms"),
        "use a release from {min} to {max} {unit}",
    ),
    number(
        "compressor.makeup",
        "compressor",
        0.0,
        24.0,
        0.0,
        Some("dB"),
        "use a makeup gain from {min} to {max} {unit}",
    ),
    enabled("gate.enabled", "gate"),
    number(
        "gate.threshold",
        "gate",
        -90.0,
        0.0,
        -50.0,
        Some("dB"),
        "use a threshold from {min} to {max} {unit}",
    ),
    number(
        "gate.range",
        "gate",
        -90.0,
        0.0,
        -80.0,
        Some("dB"),
        "use a range from {min} to {max} {unit} (the attenuation when closed)",
    ),
    number(
        "gate.attack",
        "gate",
        0.1,
        50.0,
        1.0,
        Some("ms"),
        "use an attack from {min} to {max} {unit}",
    ),
    number(
        "gate.hold",
        "gate",
        0.0,
        500.0,
        50.0,
        Some("ms"),
        "use a hold from {min} to {max} {unit}",
    ),
    number(
        "gate.release",
        "gate",
        5.0,
        2000.0,
        100.0,
        Some("ms"),
        "use a release from {min} to {max} {unit}",
    ),
    enabled("deEsser.enabled", "deEsser"),
    number(
        "deEsser.frequency",
        "deEsser",
        2000.0,
        12_000.0,
        6500.0,
        Some("Hz"),
        "use a frequency from {min} to {max} {unit}",
    ),
    number(
        "deEsser.q",
        "deEsser",
        0.5,
        8.0,
        2.0,
        None,
        "use a Q from {min} to {max}",
    ),
    number(
        "deEsser.threshold",
        "deEsser",
        -60.0,
        0.0,
        -30.0,
        Some("dB"),
        "use a threshold from {min} to {max} {unit}",
    ),
    number(
        "deEsser.ratio",
        "deEsser",
        1.0,
        20.0,
        6.0,
        None,
        "use a ratio from {min} to {max}",
    ),
    number(
        "deEsser.range",
        "deEsser",
        0.0,
        24.0,
        12.0,
        Some("dB"),
        "use a range from {min} to {max} {unit} (the most it reduces)",
    ),
];
