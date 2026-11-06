//! One layout from a built [`System`]: `Shaping::Advanced`, `Wrap::Word`, no hinting, every
//! output length rounded with `libm::roundf` to 1/64 px.

use std::collections::BTreeSet;
use std::ops::Range;

use cosmic_text::fontdb::Weight;
use cosmic_text::skrifa::MetadataProvider;
use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping, Wrap};
use fin_core::layout::{Align, LineBox, TextLayout, TextRequest, WordBox};

use crate::system::System;

/// `v` px in 1/64 px, rounded half away from zero.
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

/// The joined paragraph and each word's byte range in it.
fn paragraph(words: &[String]) -> (String, Vec<Range<usize>>) {
    let mut text = String::new();
    let mut ranges = Vec::with_capacity(words.len());
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            text.push(' ');
        }
        let start = text.len();
        text.push_str(w);
        ranges.push(start..text.len());
    }
    (text, ranges)
}

fn word_at(ranges: &[Range<usize>], at: usize) -> Option<usize> {
    let i = ranges.partition_point(|r| r.end <= at);
    ranges.get(i).filter(|r| r.start <= at).map(|_| i)
}

fn mapped(sys: &System, c: char) -> bool {
    sys.faces.iter().any(|f| {
        cosmic_text::skrifa::FontRef::from_index(&f.bytes, f.index)
            .is_ok_and(|font| font.charmap().map(c).is_some())
    })
}

/// Horizontal extent `(lo, hi)` accumulated in px.
#[derive(Clone, Copy)]
struct Span(f32, f32);

impl Span {
    const EMPTY: Span = Span(f32::INFINITY, f32::NEG_INFINITY);
    fn add(&mut self, lo: f32, hi: f32) {
        self.0 = self.0.min(lo);
        self.1 = self.1.max(hi);
    }
    /// `(x, width)` in 1/64 px; `(0, 0)` when nothing was added.
    fn boxed(self) -> (i32, i32) {
        if self.0 > self.1 {
            return (0, 0);
        }
        let (lo, hi) = (to_64(self.0), to_64(self.1));
        (lo, hi - lo)
    }
}

pub(crate) fn layout(sys: &mut System, req: &TextRequest) -> TextLayout {
    let (text, ranges) = paragraph(&req.words);
    let mut out = TextLayout {
        lines: Vec::new(),
        words: vec![
            WordBox {
                line: 0,
                x: 0,
                width: 0
            };
            ranges.len()
        ],
        width: 0,
        height: 0,
        overflow: false,
        missing: Vec::new(),
        faces: Vec::new(),
    };
    if text.is_empty() {
        return out;
    }
    let st = &req.style;
    let metrics = Metrics::new(st.size as f32, (st.size * st.line_height) as f32);
    let mut buffer = Buffer::new_empty(metrics);
    buffer.set_wrap(Wrap::Word);
    buffer.set_size(req.max_width.map(|w| w as f32), None);
    let attrs = Attrs::new()
        .family(Family::Name(sys.primary))
        .weight(Weight(st.weight))
        .letter_spacing(st.letter_spacing as f32);
    buffer.set_text(&text, &attrs, Shaping::Advanced, Some(align(st.align)));
    buffer.shape_until_scroll(&mut sys.fs, false);

    let mut used = vec![false; sys.faces.len()];
    let mut missing = BTreeSet::new();
    let mut word_span: Vec<Option<(u32, Span)>> = vec![None; ranges.len()];
    let max_width = req.max_width.map(|w| to_64(w as f32));
    for (line_i, run) in buffer.layout_runs().enumerate() {
        let line_i = line_i as u32;
        let mut ink = Span::EMPTY;
        for g in run.glyphs {
            if let Some(face) = sys.faces.iter().position(|f| f.id == g.font_id) {
                used[face] = true;
            }
            let cluster = &text[g.start..g.end];
            if g.glyph_id == 0 {
                missing.extend(cluster.chars().filter(|&c| !mapped(sys, c)).map(u32::from));
            }
            if cluster.chars().all(char::is_whitespace) {
                continue;
            }
            ink.add(g.x, g.x + g.w);
            if let Some(w) = word_at(&ranges, g.start) {
                let entry = word_span[w].get_or_insert((line_i, Span::EMPTY));
                if entry.0 == line_i {
                    entry.1.add(g.x, g.x + g.w);
                }
            }
        }
        let (x, width) = ink.boxed();
        if max_width.is_some_and(|m| x < 0 || x + width > m) {
            out.overflow = true;
        }
        let top = to_64(run.line_top);
        let height = to_64(run.line_height);
        out.height = top + height;
        out.width = out.width.max(width);
        out.lines.push(LineBox {
            x,
            top,
            width,
            height,
            baseline: to_64(run.line_y),
            rtl: run.rtl,
            first_word: 0,
            end_word: 0,
        });
    }

    // Words without ink (empty strings) stay on the line of the word before them.
    let mut line = 0;
    for (w, span) in word_span.into_iter().enumerate() {
        let (x, width) = match span {
            Some((l, s)) => {
                line = l;
                s.boxed()
            }
            None => (0, 0),
        };
        out.words[w] = WordBox { line, x, width };
    }
    let mut next = 0u32;
    for (i, line) in out.lines.iter_mut().enumerate() {
        line.first_word = next;
        while (next as usize) < out.words.len() && out.words[next as usize].line == i as u32 {
            next += 1;
        }
        line.end_word = next;
    }
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
