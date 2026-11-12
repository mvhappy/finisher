//! How `fin view` writes one line (plan 4C Task 25): times as `mm:ss.d`, seconds as `2.0s`,
//! and the item, caption, transition and marker bodies at full detail.

use crate::{BlendMode, Item, ItemBody, ParamValue, Rational, Timeline, Track};

/// `t` in milliseconds, rounded half up (half away from zero for a negative time).
fn millis(t: Rational) -> i128 {
    let (n, d) = (i128::from(t.num()) * 1000, i128::from(t.den()));
    let q = (2 * n.abs() + d) / (2 * d);
    if n < 0 { -q } else { q }
}

/// The fraction of `ms` (0..1000) as at least `min` and at most three decimals.
fn fraction(ms: i128, min: usize) -> String {
    let mut s = format!("{ms:03}");
    while s.len() > min && s.ends_with('0') {
        s.pop();
    }
    if s.is_empty() { s } else { format!(".{s}") }
}

/// A timeline time: `mm:ss.d` (`h:mm:ss.d` from an hour), one to three decimals, rounded to
/// the millisecond.
pub fn clock(t: Rational) -> String {
    let ms = millis(t);
    let sign = if ms < 0 { "-" } else { "" };
    let ms = ms.abs();
    let (s, frac) = (ms / 1000, ms % 1000);
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    let f = fraction(frac, 1);
    if h > 0 {
        format!("{sign}{h}:{m:02}:{s:02}{f}")
    } else {
        format!("{sign}{m:02}:{s:02}{f}")
    }
}

/// A length in seconds: `2.0s`, `0.033s`.
pub fn secs(t: Rational) -> String {
    let ms = millis(t);
    let sign = if ms < 0 { "-" } else { "" };
    let ms = ms.abs();
    format!("{sign}{}{}s", ms / 1000, fraction(ms % 1000, 1))
}

/// A frame rate: `30fps`, `29.97fps`.
pub fn fps(rate: Rational) -> String {
    let ms = millis(rate).abs();
    format!("{}{}fps", ms / 1000, fraction(ms % 1000, 0))
}

/// A number as Rust's shortest round-trip form (`0.8`, `90`).
pub fn num(v: f64) -> String {
    format!("{v}")
}

fn pair(v: [f64; 2]) -> String {
    format!("{},{}", num(v[0]), num(v[1]))
}

/// A slot or parameter value.
pub fn value(v: &ParamValue) -> String {
    match v {
        ParamValue::Bool(b) => b.to_string(),
        ParamValue::Number(n) => num(*n),
        ParamValue::Vec2(p) => pair(*p),
        ParamValue::Text(t) => format!("{t:?}"),
        ParamValue::Asset { asset } => asset.clone(),
    }
}

fn blend(b: BlendMode) -> &'static str {
    match b {
        BlendMode::Normal => "normal",
        BlendMode::Multiply => "multiply",
        BlendMode::Screen => "screen",
        BlendMode::Overlay => "overlay",
        BlendMode::Add => "add",
        BlendMode::Darken => "darken",
        BlendMode::Lighten => "lighten",
    }
}

/// The timeline's header: id, name, resolution, rate, end, and its tracks as `name=id` with
/// their flags.
pub fn header(tl: &Timeline, end: Rational) -> String {
    let tracks: Vec<String> = tl.tracks.iter().map(track_label).collect();
    format!(
        "== {} {:?} {}x{} {} {}  {}",
        tl.id,
        tl.name,
        tl.resolution.width,
        tl.resolution.height,
        fps(tl.rate),
        clock(end),
        tracks.join(" ")
    )
}

fn track_label(t: &Track) -> String {
    let mut flags = Vec::new();
    for (on, name) in [(t.mute, "mute"), (t.solo, "solo"), (t.lock, "lock")] {
        if on {
            flags.push(name.to_string());
        }
    }
    if t.gain.to_bits() != 0.0_f64.to_bits() {
        flags.push(format!("{}dB", num(t.gain)));
    }
    if flags.is_empty() {
        format!("{}={}", t.name, t.id)
    } else {
        format!("{}={}({})", t.name, t.id, flags.join(","))
    }
}

/// An item's head (`c3 clip a1`; a caption's is its id alone) and its detail parts, which
/// the budget's first step drops. `keys` (`fin view --detail`): each animated path's keys as
/// `<key>@<at>`.
pub fn item_parts(item: &Item, keys: bool) -> (String, Vec<String>) {
    let mut detail = Vec::new();
    let head = match &item.body {
        ItemBody::Clip {
            asset,
            source_in,
            speed,
        } => {
            let len = item.duration.checked_mul(*speed).unwrap_or(item.duration);
            detail.push(format!("[{}+{}]", secs(*source_in), secs(len)));
            if *speed != Rational::from_int(1) {
                detail.push(format!("x{}", ratio(*speed)));
            }
            format!("{} clip {asset}", item.id)
        }
        ItemBody::Caption { .. } => item.id.clone(),
        ItemBody::Graphic { asset, slots } => {
            detail.extend(slots.iter().map(|(k, v)| format!("{k}={}", value(v))));
            format!("{} graphic {asset}", item.id)
        }
        ItemBody::Image { asset } => format!("{} image {asset}", item.id),
        ItemBody::Adjustment {} => format!("{} adjustment", item.id),
        ItemBody::Nested {
            timeline,
            source_in,
        } => {
            detail.push(format!("[{}+{}]", secs(*source_in), secs(item.duration)));
            format!("{} nested {timeline}", item.id)
        }
    };
    common(item, keys, &mut detail);
    (head, detail)
}

/// A speed: `1.5`, or `n/d` when it has no short decimal.
fn ratio(r: Rational) -> String {
    let ms = millis(r);
    if i128::from(r.num()) * 1000 == ms * i128::from(r.den()) {
        format!("{}{}", ms / 1000, fraction(ms % 1000, 0))
    } else {
        r.to_string()
    }
}

fn common(item: &Item, keys: bool, out: &mut Vec<String>) {
    let c = &item.common;
    let t = &c.transform;
    if t.position != [0.0, 0.0] {
        out.push(format!("pos {}", pair(t.position)));
    }
    if t.scale != [1.0, 1.0] {
        out.push(if t.scale[0].to_bits() == t.scale[1].to_bits() {
            format!("scale {}", num(t.scale[0]))
        } else {
            format!("scale {}", pair(t.scale))
        });
    }
    if t.rotation != 0.0 {
        out.push(format!("rot {}", num(t.rotation)));
    }
    if t.anchor != [0.5, 0.5] {
        out.push(format!("anchor {}", pair(t.anchor)));
    }
    if !c.crop.is_default() {
        let k = &c.crop;
        out.push(format!(
            "crop {},{},{},{}",
            num(k.left),
            num(k.right),
            num(k.top),
            num(k.bottom)
        ));
    }
    if c.opacity.to_bits() != 1.0_f64.to_bits() {
        out.push(format!("opacity {}", num(c.opacity)));
    }
    if c.blend != BlendMode::Normal {
        out.push(format!("blend {}", blend(c.blend)));
    }
    if let Some(m) = &c.mask {
        out.push(format!("mask {}", m.matte));
    }
    if let Some(a) = &c.audio {
        if a.gain != 0.0 {
            out.push(format!("gain {}dB", num(a.gain)));
        }
        if a.pan != 0.0 {
            out.push(format!("pan {}", num(a.pan)));
        }
        if !a.fade_in.is_zero() || !a.fade_out.is_zero() {
            out.push(format!("fade {}/{}", secs(a.fade_in), secs(a.fade_out)));
        }
    }
    if !c.keyframes.is_empty() && keys {
        let paths: Vec<String> = c
            .keyframes
            .iter()
            .map(|(path, list)| {
                let at: Vec<String> = list
                    .iter()
                    .map(|k| format!("{}@{}", k.id, secs(k.at)))
                    .collect();
                format!("{path}[{}]", at.join(","))
            })
            .collect();
        out.push(format!("kf: {}", paths.join(" ")));
    } else if !c.keyframes.is_empty() {
        let paths: Vec<&str> = c.keyframes.keys().map(String::as_str).collect();
        out.push(format!("kf: {}", paths.join(",")));
    }
    if !c.effects.is_empty() {
        let fx: Vec<String> = c
            .effects
            .iter()
            .map(|e| {
                if e.enabled {
                    e.kind.clone()
                } else {
                    format!("{}(off)", e.kind)
                }
            })
            .collect();
        out.push(format!("fx: {}", fx.join(",")));
    }
    if let Some(lk) = &c.link {
        out.push(format!(" ⟷ {lk}"));
    }
}

/// `start–end` (or `at` alone).
pub fn span(start: Rational, end: Option<Rational>) -> String {
    match end {
        Some(e) => format!("{}–{}", clock(start), clock(e)),
        None => clock(start),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Rational {
        s.parse().unwrap()
    }

    #[test]
    fn clocks_and_seconds() {
        assert_eq!(clock(r("0")), "00:00.0");
        assert_eq!(clock(r("4")), "00:04.0");
        assert_eq!(clock(r("15/2")), "00:07.5");
        assert_eq!(clock(r("1/30")), "00:00.033");
        assert_eq!(clock(r("1/2000")), "00:00.001", "half up");
        assert_eq!(clock(r("3725")), "1:02:05.0");
        assert_eq!(secs(r("7/2")), "3.5s");
        assert_eq!(secs(r("21/4")), "5.25s");
        assert_eq!(fps(r("30")), "30fps");
        assert_eq!(fps(r("30000/1001")), "29.97fps");
        assert_eq!(ratio(r("3/2")), "1.5");
        assert_eq!(ratio(r("1/3")), "1/3");
        assert_eq!(num(90.0), "90");
        assert_eq!(num(0.8), "0.8");
    }
}
