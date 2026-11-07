//! Built-in caption styles (`preset:<name>`) and the platforms' safe zones (spec §5.5), as
//! data.
//!
//! Safe zones are insets in px at a reference frame size, scaled to the timeline resolution
//! ([`safe_box`]). Sources, each read on the date in `checked`:
//! - `tiktok`: TikTok Ads Manager, In-Feed / TopView safe-zone template (ads.tiktok.com help
//!   center, "TopView ad specifications", template ZIP), drawn at 720×1280: 160 px top, 440 px
//!   bottom, 80 px each side.
//! - `reels`: Meta Business Help Center, Reels ads safe zones: keep 14 % of the top, 35 % of the
//!   bottom and 6 % of each side free of text and logos (on 1080×1920: 269, 672 and 65 px).
//! - `shorts`: YouTube Shorts video-ad guidance (Google Ads help, Shorts ad specs) as creator
//!   guides cite it: 200 px top, 400 px bottom, 100 px each side on 1080×1920. Google publishes
//!   no organic Shorts template; re-check this one first when the platforms change.
//! - `generic-16x9`: EBU R 95 (Safe areas for 16:9 television production), graphics-safe area:
//!   5 % of the width and of the height on each side.
//! - `generic-9x16`: the same 5 % rule on a vertical frame.

use std::sync::LazyLock;

use crate::layout::{Align, TextStyle};
use crate::model::{Anchor, CaptionBox, CaptionPosition, CaptionStyle, Resolution, Stroke};
use crate::time::Rational;

/// A named safe zone: insets from each edge in px at the reference size `width`×`height`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafeZone {
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
    /// Where the values come from (the module docs give the details).
    pub source: &'static str,
    /// When the source was read (`YYYY-MM-DD`).
    pub checked: &'static str,
}

const ZONES: &[SafeZone] = &[
    SafeZone {
        name: "tiktok",
        width: 720,
        height: 1280,
        top: 160,
        bottom: 440,
        left: 80,
        right: 80,
        source: "TikTok Ads Manager In-Feed/TopView safe-zone template",
        checked: "2026-11-07",
    },
    SafeZone {
        name: "reels",
        width: 1080,
        height: 1920,
        top: 269,
        bottom: 672,
        left: 65,
        right: 65,
        source: "Meta Business Help Center, Reels ads safe zones (14% top, 35% bottom, 6% sides)",
        checked: "2026-11-07",
    },
    SafeZone {
        name: "shorts",
        width: 1080,
        height: 1920,
        top: 200,
        bottom: 400,
        left: 100,
        right: 100,
        source: "YouTube Shorts video-ad guidance (Google Ads), as cited by creator guides",
        checked: "2026-11-07",
    },
    SafeZone {
        name: "generic-9x16",
        width: 1080,
        height: 1920,
        top: 96,
        bottom: 96,
        left: 54,
        right: 54,
        source: "EBU R 95 graphics-safe area (5% each side), on a vertical frame",
        checked: "2026-11-07",
    },
    SafeZone {
        name: "generic-16x9",
        width: 1920,
        height: 1080,
        top: 54,
        bottom: 54,
        left: 96,
        right: 96,
        source: "EBU R 95 graphics-safe area (5% each side)",
        checked: "2026-11-07",
    },
];

/// Every safe zone, in a fixed order (the names are `params` style's `safeZone` values).
pub fn safe_zones() -> &'static [SafeZone] {
    ZONES
}

/// The zone named `name`.
pub fn safe_zone(name: &str) -> Option<&'static SafeZone> {
    ZONES.iter().find(|z| z.name == name)
}

/// The box `[x, y, width, height]` (px) of `zone` on a frame of `resolution`: each inset scaled
/// by its own axis (`resolution.width / zone.width` across, `height / zone.height` down).
/// Arithmetic only (Global Constraints, Determinism).
pub fn safe_box(zone: &SafeZone, resolution: Resolution) -> [f64; 4] {
    let (w, h) = (f64::from(resolution.width), f64::from(resolution.height));
    let kx = w / f64::from(zone.width);
    let ky = h / f64::from(zone.height);
    let (x, y) = (f64::from(zone.left) * kx, f64::from(zone.top) * ky);
    let width = w - f64::from(zone.left + zone.right) * kx;
    let height = h - f64::from(zone.top + zone.bottom) * ky;
    [x, y, width, height]
}

fn text(family: &str, weight: u16, size: f64, align: Align) -> TextStyle {
    TextStyle {
        family: family.into(),
        weight,
        italic: false,
        size,
        line_height: 1.2,
        letter_spacing: 0.0,
        align,
    }
}

fn half() -> Rational {
    Rational::new(1, 2).unwrap_or_default()
}

static PRESETS: LazyLock<Vec<CaptionStyle>> = LazyLock::new(|| {
    let base = CaptionStyle::default();
    vec![
        // Short-form default: big bold words, outlined, the spoken word highlighted.
        CaptionStyle {
            id: "preset:bold".into(),
            name: "Bold".into(),
            text: text("Inter", 700, 72.0, Align::Center),
            stroke: Some(Stroke {
                color: "#000000".into(),
                width: 6.0,
            }),
            highlight: Some("#ffd400".into()),
            safe_zone: "tiktok".into(),
            max_chars: 24,
            max_lines: 2,
            gap: half(),
            max_duration: Rational::new(3, 1).ok(),
            ..base.clone()
        },
        // Plain words on a translucent box.
        CaptionStyle {
            id: "preset:clean".into(),
            name: "Clean".into(),
            text: text("Inter", 400, 56.0, Align::Center),
            background: Some(CaptionBox {
                color: "#000000b3".into(),
                padding: 16.0,
                radius: 12.0,
            }),
            safe_zone: "reels".into(),
            max_chars: 32,
            max_lines: 2,
            gap: half(),
            ..base.clone()
        },
        // One or two words at a time, mid-frame.
        CaptionStyle {
            id: "preset:punch".into(),
            name: "Punch".into(),
            text: text("Inter", 700, 96.0, Align::Center),
            stroke: Some(Stroke {
                color: "#000000".into(),
                width: 8.0,
            }),
            position: CaptionPosition {
                anchor: Anchor::Middle,
                offset: 0.0,
            },
            safe_zone: "shorts".into(),
            max_chars: 12,
            max_lines: 1,
            gap: Rational::new(1, 4).unwrap_or_default(),
            max_duration: Rational::new(3, 2).ok(),
            ..base.clone()
        },
        // Landscape subtitles: 42 characters a line, two lines (common subtitle guidance).
        CaptionStyle {
            id: "preset:subtitle".into(),
            name: "Subtitle".into(),
            text: text("Noto Sans", 400, 48.0, Align::Center),
            stroke: Some(Stroke {
                color: "#000000".into(),
                width: 3.0,
            }),
            safe_zone: "generic-16x9".into(),
            max_chars: 42,
            max_lines: 2,
            gap: Rational::from_int(1),
            max_duration: Rational::new(6, 1).ok(),
            ..base
        },
    ]
});

/// Every built-in style, `preset:<name>`, in a fixed order.
pub fn presets() -> &'static [CaptionStyle] {
    &PRESETS
}

/// The preset `id` (`preset:bold`).
pub fn preset(id: &str) -> Option<&'static CaptionStyle> {
    PRESETS.iter().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    /// The registry's `safeZone` values are the zones' names, in order.
    #[test]
    fn zone_names_are_the_registry_values() {
        let names: Vec<&str> = super::safe_zones().iter().map(|z| z.name).collect();
        assert_eq!(names, crate::params::SAFE_ZONES);
    }
}
