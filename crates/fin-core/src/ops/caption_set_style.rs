//! `caption.setStyle`: gives a caption another style (a preset or a project style), and with
//! `overrides` replaces its per-item overrides (plan 4C Task 19). The style must exist
//! (structural `NOT_FOUND`); overrides are style paths other than `name` with values of their
//! shape (structural), in range (policy); a locked track is `TRACK_LOCKED` (policy). The diff
//! records the item's `caption.style`; the inverse sets the old style and overrides.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::captions::{PRESET_PREFIX, find_style};
use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{ParamValue, Project};
use crate::ops::util::{OpDiff, block_mut, caption_at, check_overrides, check_unlocked, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::Diff;

/// The param key a caption's style is changed under (Decision 20).
const STYLE: &str = "caption.style";

/// `caption.setStyle`: `item` takes `style`; `overrides`, when given, replace its overrides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CaptionSetStyle {
    pub item: String,
    pub style: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "BTreeMap<String, ParamValue>")]
    pub overrides: Option<BTreeMap<String, ParamValue>>,
}

impl From<CaptionSetStyle> for Op {
    fn from(op: CaptionSetStyle) -> Op {
        Op::CaptionSetStyle(op)
    }
}

fn project_ref(style: &str) -> bool {
    !style.starts_with(PRESET_PREFIX)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::CaptionSetStyle(s) = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(&s.item)?;
    let style = bindings.resolve(&s.style)?;
    let (ti, i) = caption_at(project, &id)?;
    find_style(project, &style)?;
    let mut overrides = s.overrides.clone();
    if let Some(o) = &mut overrides {
        check_overrides(project, o, bindings)?;
    }
    check_unlocked(ti.get(project), Some(&id), bindings)?;
    let track = ti.get(project).id.clone();
    let block = block_mut(project, ti, i)?;
    let old_style = std::mem::replace(&mut block.style, style);
    let old_overrides = block.overrides.clone();
    if let Some(o) = overrides {
        block.overrides = o;
    }
    diff.record_path(&track, &id, STYLE);
    Ok(vec![
        CaptionSetStyle {
            item: id,
            style: old_style,
            overrides: Some(old_overrides),
        }
        .into(),
    ])
}

/// Sets the item's `caption.style`, using the project style it names.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::CaptionSetStyle(s) = op {
        f.touches.push((s.item.clone(), Touch::Param(STYLE.into())));
        if project_ref(&s.style) {
            f.uses.push(s.style.clone());
        }
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

/// The inverse is one `caption.setStyle`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "caption.setStyle";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the item and a project style.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::CaptionSetStyle(s) => {
            let mut out = vec![&mut s.item];
            if project_ref(&s.style) {
                out.push(&mut s.style);
            }
            out
        }
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::CaptionSetStyle(s) => {
            let mut out = vec![s.item.as_str()];
            if project_ref(&s.style) {
                out.push(&s.style);
            }
            out
        }
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
