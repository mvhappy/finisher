//! `font.unpin`: drops a pin and its place in the fallback order (plan 4C Decision 29).
//! A client unpin is refused while the pin is in use (`FONT_IN_USE {key, styles, items}`,
//! structural): a project style's face, or a caption's (its effective style's face, or a
//! fallback covering its text, [`crate::layout::key_users`]). The diff records `font/<key>`
//! (`removed`); the inverse is `font.pin` with the pin and its old fallback index. Besides
//! needing the pin, its footprint touches the pin's `sha256` and `fallback`: the inverse puts
//! the old file back, so an unpin after another author's repin or reorder of the key is
//! `SAME_PARAM` (spec §4.3), where any other removal of an edited entity is not a conflict.
//!
//! The use check is for client edits only. An internal unpin (undo, redo, revert, and their
//! rebased forms) and a replay skip it: an internal unpin is the inverse of a `font.pin`, and
//! `fonts.plan` prepends pins for faces that styles added before the pin already name (a style
//! may name an unpinned face, it is unmeasured). Undoing such a caption import must remove the
//! pin while that style still uses it; the state it returns to is the pre-pin one, which was
//! valid then. Users left after the unpin lose their measurement (`fontUnpinned`), never their
//! validity, and the overflow check does not see them (they are no longer the key's users).
//!
//! Internal `fallbackOnly` (the inverse of a `font.pin` that only placed a held pin in the
//! fallback order) drops the key's fallback place and keeps the pin: no use check, as the pin
//! stays; a key out of the order is a no-op. Its diff records `font/<key>`'s `fallback`; its
//! inverse is `font.pin` with the pin and the old index.
//!
//! A caption whose text needed that fallback loses it (its script is unmeasured,
//! `fontUnpinned`, or a later fallback covers it); the op only ever undoes a placement, so it
//! returns those captions to how they were measured before the placement. The overflow check
//! looks at the key's users after the op ([`key_users`]), which no longer include those
//! captions, so they are not re-measured.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::layout::key_users;
use crate::model::Project;
use crate::ops::font_pin::{FALLBACK, FontPinOp, entity};
use crate::ops::util::{OpDiff, not_found, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// `font.unpin`: drops pin `key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FontUnpin {
    pub key: String,
    /// Internal: drop only the key's fallback place.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fallback_only: bool,
}

impl FontUnpin {
    /// Drops pin `key`.
    pub fn new(key: impl Into<String>) -> FontUnpin {
        FontUnpin {
            key: key.into(),
            fallback_only: false,
        }
    }
}

impl From<FontUnpin> for Op {
    fn from(op: FontUnpin) -> Op {
        Op::FontUnpin(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::FontUnpin(u) = op else {
        return Err(wrong_op(op));
    };
    if !project.fonts.pins.contains_key(&u.key) {
        return Err(not_found("font pin", &u.key));
    }
    if u.fallback_only {
        return drop_fallback(project, u, bindings, diff);
    }
    if !bindings.internal() {
        let (styles, items) = key_users(project, &u.key);
        if !styles.is_empty() || !items.is_empty() {
            return Err(
                CoreError::new("FONT_IN_USE", format!("{} is in use", u.key))
                    .with_details(json!({ "key": u.key, "styles": styles, "items": items }))
                    .with_fix("change the styles and captions that use the face first, or repin it (fin fonts repin)"),
            );
        }
    }
    let fallback = project.fonts.fallback.iter().position(|k| *k == u.key);
    if let Some(i) = fallback {
        project.fonts.fallback.remove(i);
    }
    let Some(pin) = project.fonts.pins.remove(&u.key) else {
        return Err(not_found("font pin", &u.key));
    };
    diff.record(EntityKind::Font, &entity(&u.key), ChangeKind::Removed);
    Ok(vec![
        FontPinOp {
            key: u.key.clone(),
            pin,
            fallback: fallback.map(|i| u32::try_from(i).unwrap_or(u32::MAX)),
        }
        .into(),
    ])
}

/// The internal `fallbackOnly` unpin: the key leaves the fallback order, the pin stays.
fn drop_fallback(
    project: &mut Project,
    u: &FontUnpin,
    bindings: &Bindings,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    if !bindings.internal() {
        return Err(CoreError::new(
            "INVALID_OP",
            "dropping only a pin's fallback place is internal",
        )
        .with_details(json!({ "op": NAME, "field": "fallbackOnly" }))
        .with_fix("unpin the face (fin fonts unpin)"));
    }
    let Some(i) = project.fonts.fallback.iter().position(|k| *k == u.key) else {
        return Ok(Vec::new());
    };
    project.fonts.fallback.remove(i);
    let Some(pin) = project.fonts.pins.get(&u.key) else {
        return Err(not_found("font pin", &u.key));
    };
    diff.record_path_of(EntityKind::Font, &entity(&u.key), None, FALLBACK);
    Ok(vec![
        FontPinOp {
            key: u.key.clone(),
            pin: pin.clone(),
            fallback: Some(u32::try_from(i).unwrap_or(u32::MAX)),
        }
        .into(),
    ])
}

/// Needs `font/<key>` and removes it; `fallbackOnly` sets its `fallback` instead.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::FontUnpin(u) = op {
        if u.fallback_only {
            f.touches
                .push((entity(&u.key), Touch::Param(FALLBACK.into())));
        } else {
            f.touches.push((entity(&u.key), Touch::Exists));
            // Its inverse pins the file it took off at its old place in the fallback order: a
            // snapshot. `font.pin` cannot declare that (a client pin of the same file must
            // merge), so the unpin meets a later repin or reorder of the key here instead
            // (`SAME_PARAM`), and an unpin-and-undo cannot put the old file back over it.
            f.touches
                .push((entity(&u.key), Touch::Param("sha256".into())));
            f.touches
                .push((entity(&u.key), Touch::Param(FALLBACK.into())));
            f.removes.push(entity(&u.key));
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

/// The inverse is one `font.pin`, or nothing for a `fallbackOnly` no-op.
pub(crate) fn inverse_len(_op: &Op, own: &Diff) -> usize {
    usize::from(!own.changes.is_empty())
}

/// The wire name.
pub(crate) const NAME: &str = "font.unpin";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// No id references: pins are keyed by face.
pub(crate) fn refs_mut(_op: &mut Op) -> Vec<&mut String> {
    Vec::new()
}

/// No ids.
pub(crate) fn ids(_op: &Op) -> Vec<&str> {
    Vec::new()
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
