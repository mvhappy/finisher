//! Deterministic test fonts, generated at test time (4C Decision 39). No font file is ever
//! committed: tests call [`build`] (or the named fonts) and get the same bytes on every machine.
//!
//! Every font has 1000 units per em, ascender 800, descender −200, no line gap, a `.notdef`
//! glyph, and one glyph per code point of [`Spec::chars`] whose advance is [`Spec::advance`].
//! Glyphs are plain rectangles (empty for the space); there is no GSUB or GPOS.

use write_fonts::FontBuilder;
use write_fonts::tables::cmap::Cmap;
use write_fonts::tables::fvar::{AxisInstanceArrays, Fvar, VariationAxisRecord};
use write_fonts::tables::glyf::{GlyfLocaBuilder, Glyph, SimpleGlyph};
use write_fonts::tables::gvar::{GlyphDelta, GlyphDeltas, GlyphVariations, Gvar, Tent};
use write_fonts::tables::head::{Head, MacStyle};
use write_fonts::tables::hhea::Hhea;
use write_fonts::tables::hmtx::{Hmtx, LongMetric};
use write_fonts::tables::maxp::Maxp;
use write_fonts::tables::name::{Name, NameRecord};
use write_fonts::tables::os2::{Os2, SelectionFlags};
use write_fonts::tables::post::Post;
use write_fonts::types::{F2Dot14, FWord, Fixed, GlyphId, LongDateTime, NameId, Tag, UfWord};

pub const UNITS_PER_EM: u16 = 1000;
pub const ASCENDER: i16 = 800;
pub const DESCENDER: i16 = -200;
/// The advance of `.notdef` (glyph 0).
pub const NOTDEF_ADVANCE: u16 = 500;

/// What a generated font contains.
pub struct Spec {
    pub family: &'static str,
    pub weight: u16,
    pub italic: bool,
    /// Every mapped code point, in glyph order (duplicates are ignored).
    pub chars: &'static str,
    pub advance: fn(char) -> u16,
}

const LATIN: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 .,!?'-/:=";
const SANS_CHARS: &str = concat!(
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 .,!?'-/:=",
    "АБВГДЕЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯабвгдежзийклмнопрстуфхцчшщъыьэюяЁё",
);
const RTL_CHARS: &str = "אבגדהוזחטיךכלםמןנסעףפץצקרשת 0123456789.,";
const HAN_CHARS: &str = "一人中日大子的国文字 abc";

fn sans_advance(c: char) -> u16 {
    match c {
        ' ' | 'i' | 'l' | 'j' | '.' | ',' | '!' | '\'' => 250,
        'm' | 'w' | 'M' | 'W' | 'Ш' | 'Щ' | 'ш' | 'щ' | 'Ж' | 'ж' | 'Ю' | 'ю' => 800,
        '0'..='9' => 550,
        c if c.is_uppercase() => 650,
        _ => 500,
    }
}

fn bold_advance(c: char) -> u16 {
    sans_advance(c) + 50
}

fn rtl_advance(c: char) -> u16 {
    match c {
        ' ' | '.' | ',' => 250,
        '0'..='9' => 550,
        _ => 600,
    }
}

fn han_advance(c: char) -> u16 {
    match c {
        ' ' => 300,
        'a'..='c' => 700,
        _ => 1000,
    }
}

/// `TestSans` Regular: Latin and Cyrillic letters, digits, a little punctuation.
pub const SANS: Spec = Spec {
    family: "TestSans",
    weight: 400,
    italic: false,
    chars: SANS_CHARS,
    advance: sans_advance,
};
/// `TestRtl` Regular: the Hebrew letters, digits, `.` and `,`.
pub const RTL: Spec = Spec {
    family: "TestRtl",
    weight: 400,
    italic: false,
    chars: RTL_CHARS,
    advance: rtl_advance,
};
/// `TestHan` Regular: ten Han ideographs (full width) and `abc` (700 units, unlike `TestSans`).
pub const HAN: Spec = Spec {
    family: "TestHan",
    weight: 400,
    italic: false,
    chars: HAN_CHARS,
    advance: han_advance,
};
/// `TestLatin` Bold: Latin only, each advance 50 units wider than `TestSans`.
pub const LATIN_BOLD: Spec = Spec {
    family: "TestLatin",
    weight: 700,
    italic: false,
    chars: LATIN,
    advance: bold_advance,
};

pub fn test_sans() -> Vec<u8> {
    build(&SANS)
}
pub fn test_rtl() -> Vec<u8> {
    build(&RTL)
}
pub fn test_han() -> Vec<u8> {
    build(&HAN)
}

/// `TestVar`: Latin like `TestLatin` but Regular, variable on `wght` 100–900 (default 400);
/// every advance grows linearly by 200 units from `wght` 400 to 900 (none below 400).
pub fn test_variable() -> Vec<u8> {
    let spec = Spec {
        family: "TestVar",
        weight: 400,
        ..LATIN_BOLD
    };
    assemble_with(&spec, None, Some(VARIABLE_DELTA))
}

/// The advance gained at `wght` 900 by every `test_variable` glyph.
pub const VARIABLE_DELTA: i16 = 200;

/// `TestSans` padded with a private `zpad` table to at least `size` bytes (and fewer than
/// `size + 4`), for size limits. Still a valid font.
pub fn test_big(size: usize) -> Vec<u8> {
    let base = assemble(&SANS, Some(0)).len();
    let pad = size.saturating_sub(base);
    assemble(&SANS, Some(pad))
}

/// A deterministic TrueType font (`glyf`, `loca`, `cmap`, `hmtx`, `hhea`, `head`, `maxp`,
/// `OS/2`, `name`, `post`). Timestamps are fixed at zero.
pub fn build(spec: &Spec) -> Vec<u8> {
    assemble(spec, None)
}

fn chars_of(spec: &Spec) -> Vec<char> {
    let mut out: Vec<char> = Vec::new();
    for c in spec.chars.chars() {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

fn rect(x0: i16, x1: i16) -> Glyph {
    let path = format!("M{x0},0 L{x1},0 L{x1},700 L{x0},700 Z");
    let path = write_fonts_path(&path);
    let mut g = SimpleGlyph::from_bezpath(&path).expect("a rectangle is a valid glyph");
    g.recompute_bounding_box();
    Glyph::Simple(g)
}

fn write_fonts_path(svg: &str) -> kurbo::BezPath {
    kurbo::BezPath::from_svg(svg).expect("valid path")
}

fn assemble(spec: &Spec, pad: Option<usize>) -> Vec<u8> {
    assemble_with(spec, pad, None)
}

/// `wght`: the advance delta (font units) every glyph gains at `wght` 900, making a variable
/// font with one axis `wght` 100–900, default 400 (deltas on the right phantom point only).
fn assemble_with(spec: &Spec, pad: Option<usize>, wght: Option<i16>) -> Vec<u8> {
    let chars = chars_of(spec);
    let mut advances = vec![NOTDEF_ADVANCE];
    let mut glyf = GlyfLocaBuilder::new();
    let notdef = rect(50, NOTDEF_ADVANCE as i16 - 50);
    let mut points = vec![points_of(&notdef)];
    glyf.add_glyph(&notdef).unwrap();
    for &c in &chars {
        let adv = (spec.advance)(c);
        advances.push(adv);
        let glyph = if c == ' ' || adv <= 100 {
            Glyph::Empty
        } else {
            rect(50, adv as i16 - 50)
        };
        points.push(points_of(&glyph));
        glyf.add_glyph(&glyph).unwrap();
    }
    let (glyf, loca, loca_format) = glyf.build();
    let num_glyphs = advances.len() as u16;
    let max_adv = *advances.iter().max().unwrap();

    let bold = spec.weight >= 700;
    let mut mac_style = MacStyle::empty();
    if bold {
        mac_style |= MacStyle::BOLD;
    }
    if spec.italic {
        mac_style |= MacStyle::ITALIC;
    }
    let head = Head::new(
        Fixed::from_f64(1.0),
        0,
        Default::default(),
        UNITS_PER_EM,
        LongDateTime::new(0),
        LongDateTime::new(0),
        0,
        DESCENDER,
        max_adv as i16,
        ASCENDER,
        mac_style,
        8,
        loca_format as i16,
    );
    let hhea = Hhea::new(
        FWord::new(ASCENDER),
        FWord::new(DESCENDER),
        FWord::new(0),
        UfWord::new(max_adv),
        FWord::new(0),
        FWord::new(0),
        FWord::new(max_adv as i16),
        1,
        0,
        0,
        num_glyphs,
    );
    let mut maxp = Maxp::new(num_glyphs);
    maxp.max_points = Some(4);
    maxp.max_contours = Some(1);
    maxp.max_composite_points = Some(0);
    maxp.max_composite_contours = Some(0);
    maxp.max_zones = Some(1);
    maxp.max_twilight_points = Some(0);
    maxp.max_storage = Some(0);
    maxp.max_function_defs = Some(0);
    maxp.max_instruction_defs = Some(0);
    maxp.max_stack_elements = Some(0);
    maxp.max_size_of_instructions = Some(0);
    maxp.max_component_elements = Some(0);
    maxp.max_component_depth = Some(0);

    let metrics = advances.iter().map(|&a| LongMetric::new(a, 50)).collect();
    let hmtx = Hmtx::new(metrics, vec![]);
    let cmap = Cmap::from_mappings(
        chars
            .iter()
            .enumerate()
            .map(|(i, &c)| (c, GlyphId::new(i as u32 + 1))),
    )
    .expect("no conflicting mappings");

    let style = match (bold, spec.italic) {
        (false, false) => "Regular",
        (true, false) => "Bold",
        (false, true) => "Italic",
        (true, true) => "Bold Italic",
    };
    let mut fs_selection = SelectionFlags::empty();
    if spec.italic {
        fs_selection |= SelectionFlags::ITALIC;
    }
    if bold {
        fs_selection |= SelectionFlags::BOLD;
    }
    if !spec.italic && !bold {
        fs_selection |= SelectionFlags::REGULAR;
    }
    let first = chars
        .iter()
        .map(|&c| c as u32)
        .min()
        .unwrap_or(0)
        .min(0xFFFF) as u16;
    let last = chars
        .iter()
        .map(|&c| c as u32)
        .max()
        .unwrap_or(0)
        .min(0xFFFF) as u16;
    let os2 = Os2 {
        x_avg_char_width: 500,
        us_weight_class: spec.weight,
        us_width_class: 5,
        ach_vend_id: Tag::new(b"TEST"),
        fs_selection,
        us_first_char_index: first,
        us_last_char_index: last,
        s_typo_ascender: ASCENDER,
        s_typo_descender: DESCENDER,
        s_typo_line_gap: 0,
        us_win_ascent: ASCENDER as u16,
        us_win_descent: (-DESCENDER) as u16,
        ul_code_page_range_1: Some(0),
        ul_code_page_range_2: Some(0),
        sx_height: Some(500),
        s_cap_height: Some(700),
        us_default_char: Some(0),
        us_break_char: Some(32),
        us_max_context: Some(0),
        ..Default::default()
    };

    let ps = format!(
        "{}-{}",
        spec.family.replace(' ', ""),
        style.replace(' ', "")
    );
    let full = format!("{} {}", spec.family, style);
    let name_record = |id: NameId, s: &str| NameRecord::new(3, 1, 0x409, id, s.to_string().into());
    let mut records = vec![
        name_record(NameId::FAMILY_NAME, spec.family),
        name_record(NameId::SUBFAMILY_NAME, style),
        name_record(NameId::FULL_NAME, &full),
        name_record(NameId::POSTSCRIPT_NAME, &ps),
        name_record(NameId::TYPOGRAPHIC_FAMILY_NAME, spec.family),
        name_record(NameId::TYPOGRAPHIC_SUBFAMILY_NAME, style),
    ];
    if wght.is_some() {
        records.push(name_record(NameId::new(256), "Weight"));
    }
    records.sort_by_key(|r| r.name_id);
    let name = Name::new(records);
    let mut post = Post::new(
        Fixed::from_f64(if spec.italic { -12.0 } else { 0.0 }),
        FWord::new(-100),
        FWord::new(50),
        0,
        0,
        0,
        0,
        0,
    );
    post.version = write_fonts::types::Version16Dot16::VERSION_3_0;

    let mut builder = FontBuilder::new();
    builder
        .add_table(&head)
        .unwrap()
        .add_table(&hhea)
        .unwrap()
        .add_table(&maxp)
        .unwrap()
        .add_table(&os2)
        .unwrap()
        .add_table(&hmtx)
        .unwrap()
        .add_table(&cmap)
        .unwrap()
        .add_table(&glyf)
        .unwrap()
        .add_table(&loca)
        .unwrap()
        .add_table(&name)
        .unwrap()
        .add_table(&post)
        .unwrap();
    if let Some(pad) = pad {
        builder.add_raw(Tag::new(b"zpad"), vec![0u8; pad]);
    }
    let fvar_gvar = wght.map(|delta| variations(delta, &points));
    if let Some((fvar, gvar)) = &fvar_gvar {
        builder.add_table(fvar).unwrap().add_table(gvar).unwrap();
    }
    builder.build()
}

fn points_of(glyph: &Glyph) -> usize {
    match glyph {
        Glyph::Simple(g) => g.contours.iter().map(|c| c.len()).sum(),
        _ => 0,
    }
}

/// `fvar` (one `wght` axis) and `gvar` (each glyph's right phantom point moves by `delta`).
fn variations(delta: i16, points: &[usize]) -> (Fvar, Gvar) {
    let axis = VariationAxisRecord::new(
        Tag::new(b"wght"),
        Fixed::from_f64(100.0),
        Fixed::from_f64(400.0),
        Fixed::from_f64(900.0),
        0,
        NameId::new(256),
    );
    let fvar = Fvar::new(AxisInstanceArrays::new(vec![axis], vec![]));
    let zero = GlyphDelta::new(0, 0, true);
    let glyphs = points
        .iter()
        .enumerate()
        .map(|(gid, &n)| {
            let mut deltas = vec![zero; n];
            // Phantom points: left side bearing, advance, top, bottom.
            deltas.extend([zero, GlyphDelta::new(delta, 0, true), zero, zero]);
            let peak = Tent::new(F2Dot14::from_f32(1.0), None);
            GlyphVariations::new(
                GlyphId::new(gid as u32),
                vec![GlyphDeltas::new(vec![peak], deltas)],
            )
        })
        .collect();
    (fvar, Gvar::new(glyphs, 1).expect("valid glyph variations"))
}

/// A TrueType Collection (`ttcf` 1.0) holding `fonts` in order. Each font's tables are copied
/// after its own table directory, so faces never share table data.
pub fn collection(fonts: &[Vec<u8>]) -> Vec<u8> {
    let be16 = |b: &[u8], at: usize| u16::from_be_bytes([b[at], b[at + 1]]);
    let be32 = |b: &[u8], at: usize| u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let header = 12 + 4 * fonts.len();
    let mut out = vec![0u8; header];
    out[..4].copy_from_slice(b"ttcf");
    out[4..8].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    out[8..12].copy_from_slice(&(fonts.len() as u32).to_be_bytes());
    for (i, font) in fonts.iter().enumerate() {
        let start = out.len();
        out[12 + 4 * i..16 + 4 * i].copy_from_slice(&(start as u32).to_be_bytes());
        let num_tables = be16(font, 4) as usize;
        let dir_len = 12 + 16 * num_tables;
        out.extend_from_slice(&font[..dir_len]);
        for t in 0..num_tables {
            let rec = 12 + 16 * t;
            let (offset, length) = (be32(font, rec + 8) as usize, be32(font, rec + 12) as usize);
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
            let new_offset = out.len() as u32;
            out.extend_from_slice(&font[offset..offset + length]);
            out[start + rec + 8..start + rec + 12].copy_from_slice(&new_offset.to_be_bytes());
        }
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_are_deterministic() {
        assert_eq!(test_sans(), test_sans());
        assert_ne!(test_sans(), test_rtl());
        let big = test_big(100_000);
        assert!(big.len() >= 100_000 && big.len() < 100_004, "{}", big.len());
        let ttc = collection(&[test_sans(), build(&LATIN_BOLD)]);
        assert_eq!(&ttc[..4], b"ttcf");
    }
}
