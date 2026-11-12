//! One layout from a built [`System`].
//!
//! **Lines break only between the request's words** (never inside one, whatever UAX #14
//! allows there): a greedy breaker adds words to a line while the line still fits, and each
//! candidate line is shaped on its own with `Wrap::None`, `Shaping::Advanced`, no hinting and
//! the request's alignment inside `max_width`. A word wider than `max_width` takes a line of
//! its own and overflows.
//!
//! **Bidi.** The paragraph direction comes from the whole paragraph (UAX #9 P2–P3, first
//! strong character). A line whose own first strong character gives the other direction (or
//! none, in a right-to-left paragraph) is shaped behind a mark (LRM or RLM) forcing the
//! paragraph's, so it keeps the paragraph's order. The mark moves nothing: it takes no letter
//! spacing (its advance stays zero, so neither the pen nor the alignment width shifts), and
//! its own glyphs are ignored. A line that needs no mark gets none, so nothing synthetic sits
//! in front of its glyphs (a leading combining mark is not positioned against it).
//!
//! **Ink.** Glyphs are placed by their advances (and shaping offsets), but every reported
//! box is ink: the union of the placed glyphs' outline extents ([`crate::ink`]), side
//! bearings, overhangs and positioned marks included. A line fits when its ink lies inside
//! `0..max_width`. A word with no ink (only empty glyphs) gets a zero-width box at its pen
//! position.
//!
//! Every output length is rounded with `libm::roundf` to 1/64 px; sums saturate.

use std::collections::BTreeSet;
use std::ops::Range;

use cosmic_text::fontdb::Weight;
use cosmic_text::skrifa::MetadataProvider;
use cosmic_text::{Attrs, Buffer, Family, Hinting, LayoutGlyph, Metrics, Shaping, Wrap};
use fin_core::layout::{Align, LineBox, TextLayout, TextRequest, WordBox};

use crate::ink::Ink;
use crate::system::System;

/// `v` px in 1/64 px, rounded half away from zero (saturating).
pub(crate) fn to_64(v: f32) -> i32 {
    libm::roundf(v * 64.0) as i32
}

fn align(a: Align) -> cosmic_text::Align {
    match a {
        Align::Left => cosmic_text::Align::Left,
        Align::Center => cosmic_text::Align::Center,
        Align::Right => cosmic_text::Align::Right,
    }
}

const LRM: &str = "\u{200E}";
const RLM: &str = "\u{200F}";

/// Horizontal extent `(lo, hi)` accumulated in px.
#[derive(Clone, Copy)]
struct Span(f32, f32);

impl Span {
    const EMPTY: Span = Span(f32::INFINITY, f32::NEG_INFINITY);
    /// Both ends of `x..x + w` (`w` may be negative: an advance with letter spacing).
    fn add(&mut self, x: f32, w: f32) {
        self.0 = self.0.min(x).min(x + w);
        self.1 = self.1.max(x).max(x + w);
    }
    fn is_empty(self) -> bool {
        self.0 > self.1
    }
    /// `(x, width)` in 1/64 px: the ink, or a zero-width box where the pen was (`(0, 0)`
    /// when there was no glyph at all).
    fn boxed(self, pen: Span) -> (i32, i32) {
        if self.is_empty() {
            return if pen.is_empty() {
                (0, 0)
            } else {
                (to_64(pen.0), 0)
            };
        }
        let (lo, hi) = (to_64(self.0), to_64(self.1));
        (lo, hi.saturating_sub(lo))
    }
}

/// A glyph's ink in line px, `None` when it draws nothing.
/// How far to move every glyph of a line so the letter spacing after its last glyph is gone.
///
/// Letter spacing goes between glyphs, not after a line's last one. cosmic-text adds it to
/// every glyph's advance (the direction mark's excepted), each glyph's spacing lying on its
/// right in either direction, so a line of advance `w` ends one `spacing` past its rightmost
/// glyph, and is aligned as `w` wide. This re-aligns it as `w − spacing` wide, with
/// cosmic-text's own rule: in a box `max` wide (else in the line's own width) the gap is
/// `max(0, box − width)`; a left-to-right line's left edge sits at 0, ½ or all of the gap
/// (left, centre, right), a right-to-left line's right edge at the box's right less all, ½
/// or none of it.
fn trailing_shift(max: Option<f64>, align: Align, rtl: bool, w: f32, spacing: f32) -> f32 {
    let trimmed = w - spacing;
    let (box_c, box_d) = match max {
        Some(m) => (m as f32, m as f32),
        None => (w, trimmed),
    };
    let (gap_c, gap_d) = ((box_c - w).max(0.0), (box_d - trimmed).max(0.0));
    if rtl {
        let f = match align {
            Align::Left => 1.0,
            Align::Center => 0.5,
            Align::Right => 0.0,
        };
        // The rightmost glyph's own advance ended `spacing` before cosmic-text's right edge.
        (box_d - f * gap_d) - (box_c - f * gap_c) + spacing
    } else {
        let f = match align {
            Align::Left => 0.0,
            Align::Center => 0.5,
            Align::Right => 1.0,
        };
        f * (gap_d - gap_c)
    }
}

/// `dx` shifts the glyph from where cosmic-text placed it.
fn glyph_ink(
    sys: &mut System,
    face: Option<usize>,
    g: &LayoutGlyph,
    dx: f32,
) -> Option<(f32, f32)> {
    let x = g.x + dx;
    let advance = Some((x.min(x + g.w), x.max(x + g.w)));
    let Some(f) = face.map(|i| &mut sys.faces[i]) else {
        return advance;
    };
    let Some(ink) = f.ink.as_mut() else {
        return advance;
    };
    match ink.glyph(&f.bytes, f.index, g.glyph_id) {
        Ink::None => None,
        Ink::Advance => advance,
        Ink::Outline(lo, hi) => {
            let origin = x + g.font_size * g.x_offset;
            let scale = g.font_size / ink.upem;
            Some((origin + lo * scale, origin + hi * scale))
        }
    }
}

/// One shaped line: its words `first..end`, ink box, word boxes and glyph facts.
struct Line {
    words: Range<usize>,
    x: i32,
    width: i32,
    baseline: i32,
    boxes: Vec<(i32, i32)>,
    used: Vec<bool>,
    missing: BTreeSet<u32>,
}

/// How far `line`'s ink reaches outside `0..max` (both sides summed), in 1/64 px.
fn excess(line: &Line, max: Option<i32>) -> i64 {
    let Some(m) = max else { return 0 };
    let (lo, hi) = (i64::from(line.x), i64::from(line.x) + i64::from(line.width));
    (-lo).max(0) + (hi - i64::from(m)).max(0)
}

fn fits(line: &Line, max: Option<i32>) -> bool {
    excess(line, max) == 0
}

fn mapped(sys: &System, c: char) -> bool {
    sys.faces.iter().any(|f| {
        cosmic_text::skrifa::FontRef::from_index(&f.bytes, f.index)
            .is_ok_and(|font| font.charmap().map(c).is_some())
    })
}

/// The paragraph is right-to-left when its first strong character is (UAX #9 P2–P3).
fn paragraph_rtl(words: &[String]) -> bool {
    let text = words.join(" ");
    let bidi = unicode_bidi::BidiInfo::new(&text, None);
    bidi.paragraphs.first().is_some_and(|p| p.level.is_rtl())
}

struct Shaper<'a> {
    req: &'a TextRequest,
    /// The paragraph's direction.
    rtl: bool,
    metrics: Metrics,
    max: Option<i32>,
}

impl Shaper<'_> {
    /// The mark forcing the paragraph's direction on a line of `words`, or `""` when the
    /// line's own first strong character already gives it.
    fn mark(&self, words: &[String]) -> &'static str {
        match (self.rtl, paragraph_rtl(words)) {
            (false, false) | (true, true) => "",
            (false, true) => LRM,
            (true, false) => RLM,
        }
    }

    fn shape(&self, sys: &mut System, words: Range<usize>) -> Line {
        let mark = self.mark(&self.req.words[words.clone()]);
        let mut text = String::from(mark);
        let mut ranges = Vec::with_capacity(words.len());
        for (k, w) in self.req.words[words.clone()].iter().enumerate() {
            if k > 0 {
                text.push(' ');
            }
            let start = text.len();
            text.push_str(w);
            ranges.push(start..text.len());
        }
        let st = &self.req.style;
        let mut buffer = Buffer::new_empty(self.metrics);
        buffer.set_wrap(Wrap::None);
        buffer.set_hinting(Hinting::Disabled);
        buffer.set_size(self.req.max_width.map(|w| w as f32), None);
        let plain = Attrs::new()
            .family(Family::Name(sys.primary))
            .weight(Weight(st.weight));
        let attrs = plain.clone().letter_spacing(st.letter_spacing as f32);
        // The mark is its own span without letter spacing (cosmic-text adds the spacing to
        // every glyph's advance, the mark's hidden glyph included).
        let spans = [(mark, plain), (&text[mark.len()..], attrs.clone())];
        buffer.set_rich_text(spans, &attrs, Shaping::Advanced, Some(align(st.align)));
        buffer.shape_until_scroll(&mut sys.fs, false);

        let mut line = Line {
            words: words.clone(),
            x: 0,
            width: 0,
            baseline: 0,
            boxes: vec![(0, 0); words.len()],
            used: vec![false; sys.faces.len()],
            missing: BTreeSet::new(),
        };
        let (mut ink, mut pen) = (Span::EMPTY, Span::EMPTY);
        let mut spans = vec![(Span::EMPTY, Span::EMPTY); words.len()];
        let lead = mark.len();
        let spacing = (st.letter_spacing * st.size) as f32;
        for run in buffer.layout_runs() {
            line.baseline = to_64(run.line_y);
            let tracked = run.glyphs.iter().any(|g| g.end > lead);
            let dx = if tracked {
                trailing_shift(self.req.max_width, st.align, run.rtl, run.line_w, spacing)
            } else {
                0.0
            };
            for g in run.glyphs {
                if g.end <= lead {
                    continue; // the direction mark
                }
                let face = sys.faces.iter().position(|f| f.id == g.font_id);
                if let Some(face) = face {
                    line.used[face] = true;
                }
                let cluster = &text[g.start.max(lead)..g.end];
                if g.glyph_id == 0 {
                    let unmapped = cluster.chars().filter(|&c| !mapped(sys, c));
                    line.missing.extend(unmapped.map(u32::from));
                }
                if cluster.chars().all(char::is_whitespace) {
                    continue;
                }
                let drawn = glyph_ink(sys, face, g, dx);
                pen.add(g.x + dx, 0.0);
                if let Some((lo, hi)) = drawn {
                    ink.add(lo, hi - lo);
                }
                let at = g.start.max(lead);
                let k = ranges.partition_point(|r| r.end <= at);
                if ranges.get(k).is_some_and(|r| r.start <= at) {
                    let (word_ink, word_pen) = &mut spans[k];
                    word_pen.add(g.x + dx, 0.0);
                    if let Some((lo, hi)) = drawn {
                        word_ink.add(lo, hi - lo);
                    }
                }
            }
        }
        (line.x, line.width) = ink.boxed(pen);
        line.boxes = spans.into_iter().map(|(i, p)| i.boxed(p)).collect();
        line
    }

    /// Greedy: the longest run of words from `first` that fits, at least one word. When the
    /// first word alone already reaches outside the box (an overhang at the line's start), a
    /// longer run is taken while it reaches out no further.
    fn next_line(&self, sys: &mut System, first: usize) -> Line {
        let n = self.req.words.len();
        if self.max.is_none() {
            return self.shape(sys, first..n);
        }
        let mut best = self.shape(sys, first..first + 1);
        let allowed = excess(&best, self.max);
        for end in first + 2..=n {
            let candidate = self.shape(sys, first..end);
            if excess(&candidate, self.max) > allowed {
                break;
            }
            best = candidate;
        }
        best
    }
}

pub(crate) fn layout(sys: &mut System, req: &TextRequest) -> TextLayout {
    let n = req.words.len();
    let mut out = TextLayout {
        lines: Vec::new(),
        words: vec![
            WordBox {
                line: 0,
                x: 0,
                width: 0
            };
            n
        ],
        width: 0,
        height: 0,
        overflow: false,
        missing: Vec::new(),
        faces: Vec::new(),
    };
    if n == 0 {
        return out;
    }
    let st = &req.style;
    let line_px = (st.size * st.line_height) as f32;
    let rtl = paragraph_rtl(&req.words);
    let shaper = Shaper {
        req,
        rtl,
        metrics: Metrics::new(st.size as f32, line_px),
        max: req.max_width.map(|w| to_64(w as f32)),
    };
    let height = to_64(line_px);
    let mut used = vec![false; sys.faces.len()];
    let mut missing = BTreeSet::new();
    let mut first = 0;
    while first < n {
        let line = shaper.next_line(sys, first);
        let index = out.lines.len() as u32;
        let top = (index as i32).saturating_mul(height);
        out.overflow |= !fits(&line, shaper.max);
        for (k, (x, width)) in line.words.clone().zip(line.boxes) {
            out.words[k] = WordBox {
                line: index,
                x,
                width,
            };
        }
        for (u, l) in used.iter_mut().zip(&line.used) {
            *u |= *l;
        }
        missing.extend(line.missing);
        out.width = out.width.max(line.width);
        out.lines.push(LineBox {
            x: line.x,
            top,
            width: line.width,
            height,
            baseline: top.saturating_add(line.baseline),
            rtl,
            first_word: line.words.start as u32,
            end_word: line.words.end as u32,
        });
        first = line.words.end;
    }
    out.height = (out.lines.len() as i32).saturating_mul(height);
    if req.max_lines.is_some_and(|m| out.lines.len() > m as usize) {
        out.overflow = true;
    }
    out.missing = missing.into_iter().collect();
    for (face, _) in sys.faces.iter().zip(used).filter(|(_, u)| *u) {
        if !out.faces.contains(&face.sha256) {
            out.faces.push(face.sha256.clone());
        }
    }
    out
}
