//! Checks shared by the style and caption ops (plan 4C Tasks 18–19): style values by path,
//! project styles, caption words and a caption's style reference.

use std::collections::BTreeMap;

use serde_json::json;

use crate::captions::{PRESET_PREFIX, find_style};
use crate::error::CoreError;
use crate::layout::check_words;
use crate::model::{CaptionBlock, CaptionStyle, ParamValue, Project, Word};
use crate::ops::Bindings;
use crate::ops::util::param_check::check_value;
use crate::ops::util::{invalid_range, not_found};
use crate::params::{self, ScopeOf};
use crate::time::Rational;

/// Checks `value` for style path `path`: the path exists and the value has its shape
/// (structural), numbers are canonical, and, when policy runs, it is in range.
pub(crate) fn check_style_value(
    project: &Project,
    path: &str,
    value: &mut ParamValue,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    let spec = params::find(&ScopeOf::Style, path)?;
    check_value(project, spec, path, value, bindings)
}

/// A style's name: 1 to 64 characters (policy).
pub(crate) fn check_name(name: &str, bindings: &Bindings) -> Result<(), CoreError> {
    let v = ParamValue::Text(name.to_string());
    let spec = params::find(&ScopeOf::Style, "name")?;
    params::check_shape(spec, "name", &v)?;
    if bindings.policy() {
        if name.is_empty() {
            return Err(invalid_range(
                "a style name must not be empty",
                json!({ "path": "name", "min": 1 }),
                "give the style a name",
            ));
        }
        params::check_range(spec, "name", &v)?;
    }
    Ok(())
}

/// The id of project style `reference` (resolved): `INVALID_OP` for a preset (read only),
/// `NOT_FOUND` for no such style.
pub(crate) fn project_style(project: &Project, reference: &str) -> Result<String, CoreError> {
    if reference.starts_with(PRESET_PREFIX) {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("{reference} is a built-in style and cannot change"),
        )
        .with_details(json!({ "style": reference }))
        .with_fix(format!(
            "copy it first: fin style add <name> --from {reference}"
        )));
    }
    if !project.styles.contains_key(reference) {
        return Err(not_found("style", reference));
    }
    Ok(reference.to_string())
}

/// Checks a caption's style overrides (style paths, not `name`) and makes them canonical.
pub(crate) fn check_overrides(
    project: &Project,
    overrides: &mut BTreeMap<String, ParamValue>,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    for (path, v) in overrides.iter_mut() {
        if path == "name" {
            return Err(CoreError::new(
                "PARAM_UNKNOWN",
                "a caption cannot override its style's name",
            )
            .with_details(json!({ "path": path, "scope": "style", "alternatives": [] }))
            .with_fix("override how the caption looks: text.size, color, position.anchor, …"));
        }
        check_style_value(project, path, v, bindings)?;
    }
    Ok(())
}

/// A caption's words, structurally: at least one, no control characters, each ending at or
/// after it starts and none starting before the previous one ends. With `within`, policy
/// also wants them inside `[0, within]` (out-of-range times are otherwise kept, Decision 13).
#[allow(dead_code)] // Task 19's caption ops.
pub(crate) fn check_caption_words(
    words: &[Word],
    within: Option<Rational>,
) -> Result<(), CoreError> {
    if words.is_empty() {
        return Err(
            CoreError::new("INVALID_OP", "a caption needs at least one word")
                .with_details(json!({ "path": "words" }))
                .with_fix("give the caption some text, or delete it"),
        );
    }
    let texts: Vec<String> = words.iter().map(|w| w.text.clone()).collect();
    check_words(&texts)?;
    for (i, w) in words.iter().enumerate() {
        let after_prev = i == 0 || words[i - 1].end <= w.start;
        if w.end < w.start || !after_prev {
            return Err(CoreError::new(
                "INVALID_OP",
                format!("word {i} overlaps or runs backwards"),
            )
            .with_details(json!({ "path": "words", "word": i, "start": w.start, "end": w.end }))
            .with_fix(
                "order the words in time; each ends at or after it starts, before the next",
            ));
        }
    }
    if let Some(max) = within {
        let (start, end) = (words[0].start, words[words.len() - 1].end);
        if start < Rational::zero() || end > max {
            return Err(invalid_range(
                format!("the words run from {start} to {end}, outside the caption's 0 to {max}"),
                json!({ "path": "words", "min": Rational::zero(), "max": max }),
                "keep word times inside the caption, or make the caption longer",
            ));
        }
    }
    Ok(())
}

/// A caption body: its style exists (structural `NOT_FOUND`; `block.style` resolved through
/// `bindings`), its overrides are style values and its words are well formed
/// ([`check_caption_words`]; inside `[0, within]` when given and policy runs).
pub(crate) fn check_caption(
    project: &Project,
    block: &CaptionBlock,
    within: Option<Rational>,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    find_style(project, &bindings.resolve(&block.style)?)?;
    let mut overrides = block.overrides.clone();
    check_overrides(project, &mut overrides, bindings)?;
    check_caption_words(&block.words, within.filter(|_| bindings.policy()))
}

/// The style a project style was made from, for `style.add`: `from`'s fields, or the default.
pub(crate) fn base_style(project: &Project, from: Option<&str>) -> Result<CaptionStyle, CoreError> {
    match from {
        Some(f) => Ok(find_style(project, f)?.clone()),
        None => Ok(CaptionStyle::default()),
    }
}
