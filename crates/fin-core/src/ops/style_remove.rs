//! `style.remove`: takes a project style off the project (plan 4C Task 18). A style a caption
//! uses is `STYLE_IN_USE {items}` (structural: nothing may name a missing style); presets
//! cannot go (`INVALID_OP`). The diff records it (`removed`); the inverse is `style.add` with
//! its literal id, no `from`, and every path that differs from the default style, so it comes
//! back exactly.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{CaptionStyle, ItemBody, Project};
use crate::ops::style_add::StyleAdd;
use crate::ops::util::{OpDiff, items, not_found, project_style, wrong_op};
use crate::ops::{Bindings, Op};
use crate::params::{Scope, get_style, specs};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// `style.remove`: takes `style` off the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StyleRemove {
    pub style: String,
}

impl From<StyleRemove> for Op {
    fn from(op: StyleRemove) -> Op {
        Op::StyleRemove(op)
    }
}

/// The `set` that rebuilds `style` from the default style: every path whose value differs.
pub(crate) fn rebuild(
    style: &CaptionStyle,
) -> Result<BTreeMap<String, crate::model::ParamValue>, CoreError> {
    let base = CaptionStyle::default();
    let mut set = BTreeMap::new();
    for spec in specs()
        .iter()
        .filter(|s| s.scope == Scope::Style && s.path != "name")
    {
        let v = get_style(style, spec.path)?;
        if v != get_style(&base, spec.path)?
            && let Some(v) = v
        {
            set.insert(spec.path.to_string(), v);
        }
    }
    Ok(set)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::StyleRemove(r) = op else {
        return Err(wrong_op(op));
    };
    let id = project_style(project, &bindings.resolve(&r.style)?)?;
    let users: Vec<&str> = items(project)
        .filter(|it| matches!(&it.body, ItemBody::Caption { block } if block.style == id))
        .map(|it| it.id.as_str())
        .collect();
    if !users.is_empty() {
        return Err(
            CoreError::new("STYLE_IN_USE", format!("style {id} is in use"))
                .with_details(json!({ "style": id, "items": users }))
                .with_fix(
                    "give those captions another style (fin caption style) or delete them first",
                ),
        );
    }
    let Some(style) = project.styles.remove(&id) else {
        return Err(not_found("style", &id));
    };
    diff.record(EntityKind::Style, &id, ChangeKind::Removed);
    Ok(vec![
        StyleAdd {
            id,
            name: style.name.clone(),
            from: None,
            set: rebuild(&style)?,
        }
        .into(),
    ])
}

/// Needs the style and removes it.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::StyleRemove(r) = op {
        f.touches.push((r.style.clone(), Touch::Exists));
        f.removes.push(r.style.clone());
    }
    f
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// No item-relative values.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The inverse is one `style.add`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "style.remove";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::StyleRemove(r) => vec![&mut r.style],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::StyleRemove(r) => vec![&r.style],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
