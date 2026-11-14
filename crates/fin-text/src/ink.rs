//! Glyph ink: the horizontal extent of a glyph's outline, in font units at the system's
//! variation location, cached per system.
//!
//! The extent is the outline's tight bounds (curve extrema included, not just control
//! points), unhinted. A glyph with an empty outline (a space, a zero-width control) has no
//! ink. A glyph with no outline at all, or any glyph of a colour font (`COLR`, `CBDT`, `sbix`
//! or `SVG `, whose pictures are not outlines), is inked over its whole advance.

use std::collections::HashMap;

use cosmic_text::skrifa::instance::{Location, Size};
use cosmic_text::skrifa::outline::{DrawSettings, OutlinePen};
use cosmic_text::skrifa::raw::TableProvider;
use cosmic_text::skrifa::{FontRef, GlyphId, MetadataProvider, Tag};

/// What a glyph draws, horizontally, relative to its origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Ink {
    /// Nothing (an empty outline).
    None,
    /// `(xMin, xMax)` in font units.
    Outline(f32, f32),
    /// The glyph's advance box (no outline to measure).
    Advance,
}

/// Per face: units per em, the variation location, and whether it is a colour font.
pub(crate) struct FaceInk {
    pub(crate) upem: f32,
    location: Location,
    color: bool,
    cache: HashMap<u16, Ink>,
}

impl FaceInk {
    /// `None` when `font` has no usable `head`.
    pub(crate) fn new(font: &FontRef<'_>, weight: u16) -> Option<FaceInk> {
        let upem = f32::from(font.head().ok()?.units_per_em());
        let location = font
            .axes()
            .location([(Tag::new(b"wght"), f32::from(weight))]);
        let color = [b"COLR", b"CBDT", b"sbix", b"SVG "]
            .iter()
            .any(|t| font.table_data(Tag::new(t)).is_some());
        Some(FaceInk {
            upem,
            location,
            color,
            cache: HashMap::new(),
        })
    }

    /// Glyph `gid` of face `index` of `bytes` (the bytes this was built from).
    pub(crate) fn glyph(&mut self, bytes: &[u8], index: u32, gid: u16) -> Ink {
        if self.color {
            return Ink::Advance;
        }
        if let Some(ink) = self.cache.get(&gid) {
            return *ink;
        }
        let Ok(font) = FontRef::from_index(bytes, index) else {
            return Ink::Advance;
        };
        let ink = match font.outline_glyphs().get(GlyphId::new(u32::from(gid))) {
            None => Ink::Advance,
            Some(outline) => {
                let mut pen = Bounds::default();
                let settings = DrawSettings::unhinted(Size::unscaled(), &self.location);
                match outline.draw(settings, &mut pen) {
                    Err(_) => Ink::Advance,
                    Ok(_) if pen.lo > pen.hi => Ink::None,
                    Ok(_) => Ink::Outline(pen.lo, pen.hi),
                }
            }
        };
        self.cache.insert(gid, ink);
        ink
    }
}

/// Tight horizontal bounds of an outline.
struct Bounds {
    lo: f32,
    hi: f32,
    at: f32,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds {
            lo: f32::INFINITY,
            hi: f32::NEG_INFINITY,
            at: 0.0,
        }
    }
}

impl Bounds {
    fn add(&mut self, x: f32) {
        self.lo = self.lo.min(x);
        self.hi = self.hi.max(x);
    }

    /// Adds the curve's value at each `t` strictly inside (0, 1).
    fn extrema(&mut self, ts: impl IntoIterator<Item = f32>, at: impl Fn(f32) -> f32) {
        for t in ts {
            if t > 0.0 && t < 1.0 {
                self.add(at(t));
            }
        }
    }
}

impl OutlinePen for Bounds {
    fn move_to(&mut self, x: f32, _y: f32) {
        self.add(x);
        self.at = x;
    }

    fn line_to(&mut self, x: f32, _y: f32) {
        self.add(x);
        self.at = x;
    }

    fn quad_to(&mut self, cx: f32, _cy: f32, x: f32, _y: f32) {
        let p0 = self.at;
        self.add(x);
        // B'(t) = 0 at t = (p0 − c) / (p0 − 2c + p1).
        let d = p0 - 2.0 * cx + x;
        if d != 0.0 {
            let at = |t: f32| {
                let u = 1.0 - t;
                u * u * p0 + 2.0 * u * t * cx + t * t * x
            };
            self.extrema([(p0 - cx) / d], at);
        }
        self.at = x;
    }

    fn curve_to(&mut self, c1: f32, _c1y: f32, c2: f32, _c2y: f32, x: f32, _y: f32) {
        let p0 = self.at;
        self.add(x);
        // B'(t)/3 = a t² + b t + c.
        let a = -p0 + 3.0 * c1 - 3.0 * c2 + x;
        let b = 2.0 * (p0 - 2.0 * c1 + c2);
        let c = c1 - p0;
        let at = |t: f32| {
            let u = 1.0 - t;
            u * u * u * p0 + 3.0 * u * u * t * c1 + 3.0 * u * t * t * c2 + t * t * t * x
        };
        if a == 0.0 {
            if b != 0.0 {
                self.extrema([-c / b], at);
            }
        } else {
            let disc = b * b - 4.0 * a * c;
            if disc >= 0.0 {
                let r = libm::sqrtf(disc);
                self.extrema([(-b + r) / (2.0 * a), (-b - r) / (2.0 * a)], at);
            }
        }
        self.at = x;
    }

    fn close(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_extrema_are_tight() {
        let mut b = Bounds::default();
        b.move_to(0.0, 0.0);
        // Peaks at x = 50 for t = ½ though the control point is at 100.
        b.quad_to(100.0, 0.0, 0.0, 10.0);
        assert_eq!((b.lo, b.hi), (0.0, 50.0));
        let mut b = Bounds::default();
        b.move_to(0.0, 0.0);
        // Symmetric cubic: peaks at 0.75 × 100 = 75 for t = ½.
        b.curve_to(100.0, 0.0, 100.0, 10.0, 0.0, 10.0);
        assert_eq!((b.lo, b.hi), (0.0, 75.0));
    }

    #[test]
    fn test_fonts_read_their_outlines() {
        let data = fin_testfonts::test_overhang();
        let font = FontRef::new(&data).unwrap();
        let mut ink = FaceInk::new(&font, 400).unwrap();
        let gid = |c: char| font.charmap().map(c).unwrap().to_u32() as u16;
        assert_eq!(ink.upem, 1000.0);
        assert_eq!(ink.glyph(&data, 0, gid('W')), Ink::Outline(50.0, 750.0));
        assert_eq!(ink.glyph(&data, 0, gid('J')), Ink::Outline(-200.0, 450.0));
        assert_eq!(
            ink.glyph(&data, 0, gid('\u{301}')),
            Ink::Outline(-350.0, -150.0)
        );
        assert_eq!(ink.glyph(&data, 0, gid(' ')), Ink::None);
        assert_eq!(ink.glyph(&data, 0, 999), Ink::Advance, "no such glyph");
    }
}
