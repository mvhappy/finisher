//! `font.repin`: points a pinned face at another file (plan 4C Decision 29), e.g. a newer
//! catalog version's. Only this op moves a pin: a catalog update never changes a project's
//! layout. The key must be pinned (`NOT_FOUND`) and `pin` checked as `font.pin` checks it (its
//! face stays the key's). The fallback place is kept. An identical pin records nothing. The
//! diff records `font/<key>`'s `sha256`; the inverse repins the old pin.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{FontPin, Project};
use crate::ops::font_pin::{check_pin, entity, source_id, source_ref};
use crate::ops::util::{OpDiff, not_found, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{Diff, EntityKind};

/// `font.repin`: pins `key` to `pin` instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FontRepin {
    pub key: String,
    pub pin: FontPin,
}

impl From<FontRepin> for Op {
    fn from(op: FontRepin) -> Op {
        Op::FontRepin(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::FontRepin(r) = op else {
        return Err(wrong_op(op));
    };
    if !project.fonts.pins.contains_key(&r.key) {
        return Err(not_found("font pin", &r.key));
    }
    let mut pin = r.pin.clone();
    check_pin(project, &r.key, &mut pin, bindings)?;
    let Some(held) = project.fonts.pins.get_mut(&r.key) else {
        return Err(not_found("font pin", &r.key));
    };
    if *held == pin {
        return Ok(Vec::new());
    }
    let old = std::mem::replace(held, pin);
    diff.record_path_of(EntityKind::Font, &entity(&r.key), None, "sha256");
    Ok(vec![
        FontRepin {
            key: r.key.clone(),
            pin: old,
        }
        .into(),
    ])
}

/// Sets `font/<key>`'s `sha256` (Decision 20); uses the font asset it pins.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::FontRepin(r) = op {
        f.touches
            .push((entity(&r.key), Touch::Param("sha256".into())));
        f.uses
            .extend(source_id(&r.pin).into_iter().map(str::to_string));
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

/// One `font.repin`, or nothing for an identical pin.
pub(crate) fn inverse_len(_op: &Op, own: &Diff) -> usize {
    usize::from(!own.changes.is_empty())
}

/// The wire name.
pub(crate) const NAME: &str = "font.repin";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: an asset pin's asset.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::FontRepin(r) => source_ref(&mut r.pin),
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::FontRepin(r) => source_id(&r.pin),
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
