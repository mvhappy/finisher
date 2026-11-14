//! Checks shared by the style and caption ops (plan 4C Tasks 18–19): style values by path,
//! project styles, caption words and a caption's style reference.

use std::collections::BTreeMap;

use serde_json::json;

use crate::captions::{MAX_CAPTION_WORDS, PRESET_PREFIX, find_style};
use crate::error::CoreError;
use crate::layout::check_words;
use crate::model::{
    CaptionBlock, CaptionSource, CaptionStyle, ItemBody, ParamValue, Project, Word,
};
use crate::ops::Bindings;
use crate::ops::util::param_check::check_value;
use crate::ops::util::{TrackAt, invalid_range, item_index, not_found};
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

/// At most [`MAX_CAPTION_WORDS`] words for caption `item` in what a client sends: more is
/// `CAPTION_TOO_LONG {item, words, max}`. Internal applies (undo, redo, revert, recovery) and
/// replay only re-run words a committed record held, so they are not re-judged: an edit of a
/// caption written before the limit stays undoable.
pub(crate) fn check_word_count(
    item: &str,
    words: usize,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    if bindings.internal() || words <= MAX_CAPTION_WORDS {
        return Ok(());
    }
    Err(CoreError::new(
        "CAPTION_TOO_LONG",
        format!("caption {item} would hold {words} words, more than {MAX_CAPTION_WORDS}"),
    )
    .with_details(json!({ "item": item, "words": words, "max": MAX_CAPTION_WORDS }))
    .with_fix(format!(
        "keep a caption to {MAX_CAPTION_WORDS} words: split the text over several captions"
    )))
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

/// Where caption item `id` is: `NOT_FOUND` for no item, `INVALID_OP` for an item of another
/// kind.
pub(crate) fn caption_at(project: &Project, id: &str) -> Result<(TrackAt, usize), CoreError> {
    let (ti, i) = item_index(project, id)?;
    let it = &ti.get(project).items[i];
    if !matches!(it.body, ItemBody::Caption { .. }) {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("item {id} is a {}, not a caption", it.body.kind_name()),
        )
        .with_details(json!({ "item": id, "kind": it.body.kind_name() }))
        .with_fix("caption edits take caption items"));
    }
    Ok((ti, i))
}

/// The caption block of the item at `(ti, i)`, mutably (the caller checked it is one).
pub(crate) fn block_mut(
    project: &mut Project,
    ti: TrackAt,
    i: usize,
) -> Result<&mut CaptionBlock, CoreError> {
    let it = &mut ti.get_mut(project).items[i];
    let id = it.id.clone();
    match &mut it.body {
        ItemBody::Caption { block } => Ok(block),
        _ => Err(not_found("caption", &id)),
    }
}

/// A caption's transcript source split before word `k` of its `n` words: the left part keeps
/// the range up to the cut, the right part the rest. The cut is `first + k` while the range
/// still counts the words one to one, else the same share of the range (an edited text).
pub(crate) fn split_source(
    source: Option<&CaptionSource>,
    k: usize,
    n: usize,
) -> (Option<CaptionSource>, Option<CaptionSource>) {
    let Some(s) = source else {
        return (None, None);
    };
    let [first, end] = s.word_range;
    let len = u64::from(end.saturating_sub(first));
    let (k, n) = (k as u64, (n as u64).max(1));
    let at = if len == n { k } else { len * k / n };
    let cut = first.saturating_add(u32::try_from(at.min(len)).unwrap_or(u32::MAX));
    let part = |range: [u32; 2]| CaptionSource {
        artifact: s.artifact.clone(),
        word_range: range,
    };
    (Some(part([first, cut])), Some(part([cut, end])))
}

/// The source of `item` merged with `with`'s: one range over both when both come from one
/// transcript, else `item`'s own.
pub(crate) fn merge_source(
    item: Option<&CaptionSource>,
    with: Option<&CaptionSource>,
) -> Option<CaptionSource> {
    match (item, with) {
        (Some(a), Some(b)) if a.artifact == b.artifact => Some(CaptionSource {
            artifact: a.artifact.clone(),
            word_range: [
                a.word_range[0].min(b.word_range[0]),
                a.word_range[1].max(b.word_range[1]),
            ],
        }),
        (a, _) => a.cloned(),
    }
}
