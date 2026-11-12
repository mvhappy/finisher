//! Letter spacing, applied by fin-text rather than cosmic-text.
//!
//! cosmic-text would add the spacing to every glyph's advance: combining marks too (a base
//! with a mark gets two), the hidden direction mark, and the line's last glyph (so the line
//! ends one spacing past its ink). Instead each line is shaped and aligned without spacing,
//! then every glyph of the `k`-th shaping cluster from the left (a base with its combining
//! marks, or a ligature) moves `k × spacing` right, so spacing goes **between clusters**
//! only, and the whole line moves by the difference between where cosmic-text's alignment
//! rule puts a line of the tracked width and where it put the untracked one.
//!
//! cosmic-text's rule, for a line `w` wide in a box (`max_width`, else the line's own width):
//! the gap is `max(0, box − w)`; a left-to-right line's left edge sits at 0, ½ or all of the
//! gap (left, centre, right), a right-to-left line's right edge at the box's right less all,
//! ½ or none of it.

use std::collections::BTreeMap;

use cosmic_text::LayoutGlyph;
use fin_core::layout::Align;

/// The left edge cosmic-text gives a line `w` wide.
fn left_edge(max: Option<f64>, align: Align, rtl: bool, w: f32) -> f32 {
    let bx = max.map_or(w, |m| m as f32);
    let gap = (bx - w).max(0.0);
    let f = match align {
        Align::Left => 0.0,
        Align::Center => 0.5,
        Align::Right => 1.0,
    };
    if rtl {
        bx - (1.0 - f) * gap - w
    } else {
        f * gap
    }
}

/// The line facts [`shifts`] needs.
pub(crate) struct LineFacts {
    pub(crate) max: Option<f64>,
    pub(crate) align: Align,
    pub(crate) rtl: bool,
    /// The untracked line's advance.
    pub(crate) w: f32,
    /// Letter spacing in px.
    pub(crate) spacing: f32,
}

/// Each glyph's horizontal shift in px (in `glyphs` order). Glyphs ending at or before
/// `lead` (the direction mark) are not a cluster of their own.
pub(crate) fn shifts(glyphs: &[LayoutGlyph], lead: usize, line: &LineFacts) -> Vec<f32> {
    if line.spacing == 0.0 {
        return vec![0.0; glyphs.len()];
    }
    // Each cluster's left edge: its leftmost advancing glyph (a lone mark: itself).
    let mut clusters: BTreeMap<usize, (f32, bool)> = BTreeMap::new();
    for g in glyphs.iter().filter(|g| g.end > lead) {
        let advancing = g.w != 0.0;
        let e = clusters.entry(g.start).or_insert((g.x, advancing));
        match (e.1, advancing) {
            (false, true) => *e = (g.x, true),
            (true, false) => {}
            _ => e.0 = e.0.min(g.x),
        }
    }
    let mut order: Vec<(f32, usize)> = clusters.iter().map(|(&s, &(x, _))| (x, s)).collect();
    // Ties (a lone mark against its neighbour) go in reading order.
    order.sort_by(|a, b| {
        let by_start = if line.rtl {
            b.1.cmp(&a.1)
        } else {
            a.1.cmp(&b.1)
        };
        a.0.total_cmp(&b.0).then(by_start)
    });
    let rank: BTreeMap<usize, f32> = order
        .iter()
        .enumerate()
        .map(|(k, &(_, s))| (s, k as f32))
        .collect();
    let gaps = order.len().saturating_sub(1) as f32;
    let tracked = line.w + gaps * line.spacing;
    let moved = left_edge(line.max, line.align, line.rtl, tracked)
        - left_edge(line.max, line.align, line.rtl, line.w);
    glyphs
        .iter()
        .map(|g| moved + rank.get(&g.start).map_or(0.0, |k| k * line.spacing))
        .collect()
}
