//! `style.add`: a project caption style (plan 4C Task 18, Decision 28). `id` is a `$name`
//! placeholder bound to a new `st<n>` id (a literal id restores one: internal only, the
//! inverse of `style.remove`). The style starts as `from` (a preset `preset:<name>` or a
//! project style; the default style without it), takes `name`, then each `set` path in path
//! order. A set path must be a style path with a value of its shape (structural); ranges and
//! the name's length are policy. The diff records the style (`added`); the inverse is
//! `style.remove`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::captions::PRESET_PREFIX;
use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{ParamValue, Project};
use crate::ops::style_remove::StyleRemove;
use crate::ops::util::{OpDiff, base_style, check_name, check_style_value, wrong_op};
use crate::ops::{Bindings, Op};
use crate::params::set_style;
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// `style.add`: a project style named `name`, from `from`, with `set` applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StyleAdd {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub set: BTreeMap<String, ParamValue>,
}

impl From<StyleAdd> for Op {
    fn from(op: StyleAdd) -> Op {
        Op::StyleAdd(op)
    }
}

/// Whether `from` names a project style (an id), not a preset.
fn project_ref(from: &str) -> bool {
    !from.starts_with(PRESET_PREFIX)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::StyleAdd(a) = op else {
        return Err(wrong_op(op));
    };
    let from = a.from.as_deref().map(|f| bindings.resolve(f)).transpose()?;
    let mut style = base_style(project, from.as_deref())?;
    check_name(&a.name, bindings)?;
    style.name.clone_from(&a.name);
    for (path, value) in &a.set {
        if path == "name" {
            continue;
        }
        let mut v = value.clone();
        check_style_value(project, path, &mut v, bindings)?;
        set_style(&mut style, path, Some(v))?;
    }
    let id = bindings.create(project, &a.id, IdKind::Style, ids)?;
    style.id.clone_from(&id);
    project.styles.insert(id.clone(), style);
    diff.record(EntityKind::Style, &id, ChangeKind::Added);
    Ok(vec![StyleRemove { style: id }.into()])
}

/// Creates the style, using the project style it copies.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::StyleAdd(a) = op {
        f.creates.push(a.id.clone());
        f.uses
            .extend(a.from.iter().filter(|x| project_ref(x)).cloned());
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

/// The inverse is one `style.remove`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "style.add";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the new id and the project style it copies.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::StyleAdd(a) => {
            let mut out = vec![&mut a.id];
            out.extend(a.from.as_mut().filter(|x| project_ref(x)));
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::StyleAdd(a) => {
            let mut out = vec![a.id.as_str()];
            out.extend(a.from.as_deref().filter(|x| project_ref(x)));
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::StyleAdd(a) => Some(a.id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
