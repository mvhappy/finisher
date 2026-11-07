//! Captions (plan 4C Task 18, spec §5.5): splitting words into blocks by a style's rule
//! ([`blocks`]), SubRip import ([`parse_srt`]), re-timing edited text ([`align_text`]), the
//! built-in styles and safe zones ([`presets`], [`safe_zones`]), and a caption's effective
//! style ([`resolve_style`]).
//!
//! Evaluation-pure (Global Constraints, Determinism): exact rationals, and `f64` arithmetic
//! only.

use crate::error::CoreError;
use crate::model::{CaptionBlock, CaptionStyle, Project};
use crate::ops::util::not_found;
use crate::params::set_style;

pub(crate) mod align;
mod blocks;
mod presets;
mod srt;

pub use align::align_text;
pub use blocks::{BlockDraft, blocks};
pub use presets::{SafeZone, preset, presets, safe_box, safe_zone, safe_zones};
pub use srt::parse_srt;

/// The prefix of the built-in styles' ids.
pub const PRESET_PREFIX: &str = "preset:";

/// The style `reference` names: a preset (`preset:<name>`) or a project style (`st<n>`).
pub fn style_of<'a>(project: &'a Project, reference: &str) -> Option<&'a CaptionStyle> {
    if reference.starts_with(PRESET_PREFIX) {
        preset(reference)
    } else {
        project.styles.get(reference)
    }
}

/// [`style_of`], or `NOT_FOUND` (kind `style`).
pub fn find_style<'a>(
    project: &'a Project,
    reference: &str,
) -> Result<&'a CaptionStyle, CoreError> {
    style_of(project, reference).ok_or_else(|| not_found("style", reference))
}

/// The style a caption is laid out with: its style with its `overrides` applied, in path
/// order. `NOT_FOUND` for a missing style; `PARAM_UNKNOWN`/`PARAM_TYPE` for an override the
/// style scope does not take (the ops refuse those before they are stored).
pub fn resolve_style(project: &Project, block: &CaptionBlock) -> Result<CaptionStyle, CoreError> {
    let mut style = find_style(project, &block.style)?.clone();
    for (path, value) in &block.overrides {
        set_style(&mut style, path, Some(value.clone()))?;
    }
    Ok(style)
}
