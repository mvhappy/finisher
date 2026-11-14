//! `style.set`: sets one path of a project style, or resets it with no `value` (plan 4C Task
//! 18). Presets are read only (`INVALID_OP`). An unknown path and a value of the wrong shape
//! are structural; the range is policy. The diff records the style's parameter; the inverse
//! sets the old value.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{ParamValue, Project};
use crate::ops::util::{OpDiff, check_name, check_style_value, project_style, wrong_op};
use crate::ops::{Bindings, Op};
use crate::params::set_style;
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{Diff, EntityKind};

/// `style.set`: sets `path` of `style` to `value`; no `value` resets it to the default
/// style's (or removes an optional part).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StyleSet {
    pub style: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ParamValue")]
    pub value: Option<ParamValue>,
}

impl From<StyleSet> for Op {
    fn from(op: StyleSet) -> Op {
        Op::StyleSet(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::StyleSet(s) = op else {
        return Err(wrong_op(op));
    };
    let id = project_style(project, &bindings.resolve(&s.style)?)?;
    let mut value = s.value.clone();
    match &mut value {
        Some(ParamValue::Text(name)) if s.path == "name" => check_name(name, bindings)?,
        Some(v) => check_style_value(project, &s.path, v, bindings)?,
        None if s.path == "name" => {
            return Err(
                CoreError::new("INVALID_OP", "a style's name cannot be reset")
                    .with_details(serde_json::json!({ "path": "name" }))
                    .with_fix("give the style a name"),
            );
        }
        None => {
            crate::params::find(&crate::params::ScopeOf::Style, &s.path)?;
        }
    }
    let Some(style) = project.styles.get_mut(&id) else {
        return Err(crate::ops::util::not_found("style", &id));
    };
    let old = set_style(style, &s.path, value)?;
    diff.record_path_of(EntityKind::Style, &id, None, &s.path);
    Ok(vec![
        StyleSet {
            style: id,
            path: s.path.clone(),
            value: old,
        }
        .into(),
    ])
}

/// Sets the style's parameter `path`.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::StyleSet(s) = op {
        f.touches
            .push((s.style.clone(), Touch::Param(s.path.clone())));
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

/// The inverse is one `style.set`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "style.set";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::StyleSet(s) => vec![&mut s.style],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::StyleSet(s) => vec![&s.style],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
