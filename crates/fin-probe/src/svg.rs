//! SVG: the root `<svg>`'s `width`, `height` and `viewBox`, in px. Sanitising is
//! sub-project 6; nothing here renders or follows references.

use std::path::Path;

use fin_core::{AssetKind, CoreError, ImageInfo};

use crate::{Probe, unsupported};

const SVG_NS: &str = "http://www.w3.org/2000/svg";
/// Element and attribute nodes a probe parses at most.
const NODES_LIMIT: u32 = 1_000_000;

/// A length in px: a non-negative number, unitless or `px`. `Err` names what is wrong.
fn length(v: &str) -> Result<f64, String> {
    let v = v.trim();
    let num = v.strip_suffix("px").unwrap_or(v).trim_end();
    match num.parse::<f64>() {
        Ok(n) if n.is_finite() && n >= 0.0 => Ok(n),
        _ => Err(format!("size {v:?} is not in px")),
    }
}

/// `min-x min-y width height`, separated by spaces and/or commas; width and height > 0.
fn view_box(v: &str) -> Result<(f64, f64), String> {
    let parts: Vec<&str> = v
        .split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter(|p| !p.is_empty())
        .collect();
    let nums: Option<Vec<f64>> = parts
        .iter()
        .map(|p| p.parse::<f64>().ok().filter(|n| n.is_finite()))
        .collect();
    match nums.as_deref() {
        Some(&[_, _, w, h]) if w > 0.0 && h > 0.0 => Ok((w, h)),
        _ => Err(format!(
            "viewBox {v:?} is not four numbers with a positive size"
        )),
    }
}

fn px(v: f64) -> Option<u32> {
    let r = (v + 0.5).floor();
    (r >= 1.0 && r <= f64::from(u32::MAX)).then_some(r as u32)
}

/// The size `width`/`height` give, else what `viewBox` gives (a single one of `width` and
/// `height` scales the `viewBox`'s aspect ratio, as browsers do).
fn size(
    width: Option<f64>,
    height: Option<f64>,
    vb: Option<(f64, f64)>,
) -> Result<(f64, f64), &'static str> {
    match (width, height, vb) {
        (Some(w), Some(h), _) => Ok((w, h)),
        (Some(w), None, Some((vw, vh))) => Ok((w, w * vh / vw)),
        (None, Some(h), Some((vw, vh))) => Ok((h * vw / vh, h)),
        (None, None, Some(vb)) => Ok(vb),
        _ => Err("no width and height or viewBox"),
    }
}

pub(crate) fn probe(text: &str, path: &Path) -> Result<Probe, CoreError> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: NODES_LIMIT,
        ..roxmltree::ParsingOptions::default()
    };
    let doc = roxmltree::Document::parse_with_options(text, opts)
        .map_err(|e| unsupported(path, &format!("malformed XML: {e}")))?;
    let root = doc.root_element();
    let tag = root.tag_name();
    if tag.name() != "svg" || tag.namespace().is_some_and(|ns| ns != SVG_NS) {
        return Err(unsupported(path, "the XML root is not <svg>"));
    }
    let attr = |name: &str| root.attribute(name).map(length).transpose();
    let bad = |why: String| unsupported(path, &why);
    let width = attr("width").map_err(bad)?;
    let height = attr("height").map_err(bad)?;
    let vb = root
        .attribute("viewBox")
        .map(view_box)
        .transpose()
        .map_err(bad)?;
    let (w, h) = size(width, height, vb).map_err(|why| unsupported(path, why))?;
    let (Some(width), Some(height)) = (px(w), px(h)) else {
        return Err(unsupported(path, "the SVG has no pixels"));
    };
    Ok(Probe {
        image: Some(ImageInfo { width, height }),
        ..Probe::of(AssetKind::Svg)
    })
}
