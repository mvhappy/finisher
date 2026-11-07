//! `asset.remove`: takes an unused asset off the project (a client op since plan 4C Task 13,
//! and the inverse of `asset.add`). An asset in use is `ASSET_IN_USE` (structural: nothing may
//! name a missing asset): by an item showing it (a clip, graphic or image), an item's effect
//! naming it (a `lut` effect's `asset` param), or a font pin of it (`source: asset`). The
//! details list them: `items` (and `clips`, as 4B named it) and `pins` (their keys). The diff
//! records it (`removed`); the inverse is `asset.add` with its literal id and every field.

use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{FontSource, Item, ParamValue, Project};
use crate::ops::util::{OpDiff, asset, items, wrong_op};
use crate::ops::{AssetAdd, Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// Whether `item` uses asset `id`: it shows it, or one of its effects names it. The ids an
/// item lists ([`Item::ids`]) cover what it shows (and, with the settings registry, its
/// effects' asset references); a `lut` effect's `asset` param held as text is read too.
fn uses(item: &Item, id: &str) -> bool {
    item.ids().contains(&id)
        || item.common.effects.iter().any(|e| {
            e.kind == "lut" && matches!(e.params.get("asset"), Some(ParamValue::Text(a)) if a == id)
        })
}

pub(crate) fn validate(
    project: &Project,
    op: &Op,
    bindings: &Bindings,
) -> Result<String, CoreError> {
    let Op::AssetRemove { id } = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(id)?;
    asset(project, &id)?;
    let users: Vec<&str> = items(project)
        .filter(|c| uses(c, &id))
        .map(|c| c.id.as_str())
        .collect();
    let pins: Vec<&str> = project
        .fonts
        .pins
        .iter()
        .filter(|(_, p)| matches!(&p.source, FontSource::Asset { asset } if *asset == id))
        .map(|(k, _)| k.as_str())
        .collect();
    if !users.is_empty() || !pins.is_empty() {
        let fix = if pins.is_empty() {
            "delete the items that use the asset first"
        } else {
            "delete the items that use the asset and unpin its fonts (fin fonts unpin) first"
        };
        return Err(
            CoreError::new("ASSET_IN_USE", format!("asset {id} is in use"))
                .with_details(json!({ "id": id, "clips": users, "items": users, "pins": pins }))
                .with_fix(fix),
        );
    }
    Ok(id)
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let id = validate(project, op, bindings)?;
    let Some(a) = project.assets.remove(&id) else {
        return Err(crate::ops::util::not_found("asset", &id));
    };
    diff.record(EntityKind::Asset, &id, ChangeKind::Removed);
    Ok(vec![
        AssetAdd {
            id: a.id,
            kind: a.kind,
            path: a.path,
            fingerprint: a.fingerprint,
            duration: a.duration,
            video: a.video,
            audio: a.audio,
            image: a.image,
            lottie: a.lottie,
            font: a.font,
            lut: a.lut,
            bin: a.bin,
        }
        .into(),
    ])
}

/// Removes the asset; binds no track.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::AssetRemove { id } = op {
        f.touches.push((id.clone(), Touch::Exists));
        f.removes.push(id.clone());
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

/// The op's inverse is one op.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "asset.remove";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::AssetRemove { id } => vec![id],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::AssetRemove { id } => vec![id],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
