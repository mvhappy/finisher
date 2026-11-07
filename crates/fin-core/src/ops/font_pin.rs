//! `font.pin`: pins a face to an exact file (plan 4C Decision 29), optionally placing it in the
//! project's fallback order at `fallback`. Pins are plain data: the op never fetches and never
//! reads font bytes.
//!
//! - The key is the pin's face (`font_key(family, weight, italic)`), the SHA-256 64 lowercase
//!   hex digits and the size at least a byte (structural, `INVALID_OP`). An asset pin names a
//!   font asset (`NOT_FOUND`) holding that file and face (`INVALID_OP`).
//! - A key already pinned **to the same SHA-256** is a no-op (empty diff, empty inverse), so two
//!   edits pinning one face merge, unless the op places the key in the fallback order and the
//!   order lacks it: then it inserts it there, the diff records `font/<key>`'s `fallback` and
//!   the inverse is the internal `font.unpin {fallbackOnly}`, so a merge keeps the fallback
//!   place either edit asked for. Another SHA-256 is `FONT_PINNED` (policy on edits; replay
//!   refuses it too, as valid history cannot produce it): `font.repin` changes a pin's file.
//! - `fallback` past the end of the order is `INVALID_RANGE` for clients; internal and replay
//!   applies (an undo of `font.unpin` meeting a shorter order) put it at the end.
//!
//! The diff records `font/<key>` (`added`); the inverse is `font.unpin`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{AssetKind, FontPin, FontSource, Project, font_key};
use crate::ops::font_unpin::FontUnpin;
use crate::ops::util::{OpDiff, asset, invalid_range, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// `font.pin`: pins `key` to `pin`, at `fallback` in the fallback order if given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FontPinOp {
    pub key: String,
    pub pin: FontPin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub fallback: Option<u32>,
}

impl From<FontPinOp> for Op {
    fn from(op: FontPinOp) -> Op {
        Op::FontPin(op)
    }
}

/// The param key a pin's place in the fallback order is changed under.
pub(crate) const FALLBACK: &str = "fallback";

/// The footprint entity of pin `key` (Decision 20).
pub(crate) fn entity(key: &str) -> String {
    format!("font/{key}")
}

fn bad(message: String, details: serde_json::Value, fix: &str) -> CoreError {
    CoreError::new("INVALID_OP", message)
        .with_details(details)
        .with_fix(fix)
}

/// Checks `pin` for `key` (see the module docs); resolves an asset source's placeholder.
pub(crate) fn check_pin(
    project: &Project,
    key: &str,
    pin: &mut FontPin,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    let want = font_key(&pin.family, pin.weight, pin.italic);
    if key != want {
        return Err(bad(
            format!("pin key {key} does not name its face {want}"),
            json!({ "key": key, "face": want }),
            "use the key <family>|<weight>|<normal|italic> of the pinned face",
        ));
    }
    let hex = pin.sha256.len() == 64
        && pin
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !hex || pin.size == 0 {
        return Err(bad(
            format!("pin {key} needs a 64-digit lowercase SHA-256 and a size"),
            json!({ "key": key, "sha256": pin.sha256, "size": pin.size }),
            "pin a file from the font catalog or a font asset (fin fonts pin)",
        ));
    }
    if let FontSource::Asset { asset: a } = &mut pin.source {
        *a = bindings.resolve(a)?;
        let found = asset(project, a)?;
        let file = found
            .font
            .as_ref()
            .filter(|_| found.kind == AssetKind::Font);
        let face = file.and_then(|f| f.faces.iter().find(|x| x.index == pin.face));
        let matches = file.is_some_and(|f| f.sha256 == pin.sha256) && face.is_some();
        if !matches {
            return Err(bad(
                format!(
                    "asset {a} does not hold face {} of file {}",
                    pin.face, pin.sha256
                ),
                json!({ "key": key, "asset": a, "face": pin.face, "sha256": pin.sha256 }),
                "pin a face the font asset lists (fin fonts pin --asset)",
            ));
        }
    }
    Ok(())
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::FontPin(p) = op else {
        return Err(wrong_op(op));
    };
    let mut pin = p.pin.clone();
    check_pin(project, &p.key, &mut pin, bindings)?;
    if let Some(held) = project.fonts.pins.get(&p.key) {
        if held.sha256 == pin.sha256 {
            let placed = project.fonts.fallback.contains(&p.key);
            let Some(at) = p.fallback.filter(|_| !placed) else {
                return Ok(Vec::new());
            };
            insert_fallback(project, &p.key, at, bindings)?;
            diff.record_path_of(EntityKind::Font, &entity(&p.key), None, FALLBACK);
            return Ok(vec![
                FontUnpin {
                    key: p.key.clone(),
                    fallback_only: true,
                }
                .into(),
            ]);
        }
        return Err(CoreError::new(
            "FONT_PINNED",
            format!("{} is pinned to another file", p.key),
        )
        .with_details(json!({ "key": p.key, "sha256": held.sha256, "wanted": pin.sha256 }))
        .with_fix(format!(
            "change the pinned file with fin fonts repin \"{}\"",
            p.key
        )));
    }
    if let Some(at) = p.fallback {
        insert_fallback(project, &p.key, at, bindings)?;
    }
    project.fonts.pins.insert(p.key.clone(), pin);
    diff.record(EntityKind::Font, &entity(&p.key), ChangeKind::Added);
    Ok(vec![FontUnpin::new(p.key.clone()).into()])
}

/// Puts `key` at `at` in the fallback order (see the module docs for an index past the end).
fn insert_fallback(
    project: &mut Project,
    key: &str,
    at: u32,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    let order = &mut project.fonts.fallback;
    let at = usize::try_from(at).unwrap_or(usize::MAX);
    if at > order.len() && bindings.policy() && !bindings.internal() {
        return Err(invalid_range(
            format!("fallback index {at} is past the end of the order"),
            json!({ "path": "fallback", "min": 0, "max": order.len() }),
            format!("use an index from 0 to {}", order.len()),
        ));
    }
    order.insert(at.min(order.len()), key.to_string());
    Ok(())
}

/// Creates `font/<key>`, using the font asset it pins.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::FontPin(p) = op {
        f.creates.push(entity(&p.key));
        if let FontSource::Asset { asset } = &p.pin.source {
            f.uses.push(asset.clone());
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

/// One `font.unpin` (`fallbackOnly` for a held pin), or nothing for a no-op pin (it records
/// no change).
pub(crate) fn inverse_len(_op: &Op, own: &Diff) -> usize {
    usize::from(!own.changes.is_empty())
}

/// The wire name.
pub(crate) const NAME: &str = "font.pin";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// The asset of an asset pin (Decision 24), mutably.
pub(crate) fn source_ref(pin: &mut FontPin) -> Vec<&mut String> {
    match &mut pin.source {
        FontSource::Asset { asset } => vec![asset],
        FontSource::Catalog { .. } => Vec::new(),
    }
}

/// The asset of an asset pin.
pub(crate) fn source_id(pin: &FontPin) -> Vec<&str> {
    match &pin.source {
        FontSource::Asset { asset } => vec![asset],
        FontSource::Catalog { .. } => Vec::new(),
    }
}

/// Every id reference the op carries, mutably: an asset pin's asset (pins are keyed by face,
/// not by id).
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::FontPin(p) => source_ref(&mut p.pin),
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::FontPin(p) => source_id(&p.pin),
        _ => Vec::new(),
    }
}

/// Pins allocate no id (Decision 17).
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
