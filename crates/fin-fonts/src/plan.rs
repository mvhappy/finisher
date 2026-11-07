//! Pin planning (4C Decision 35): the `font.pin` ops a client prepends to its own tx so every
//! face and script its captions need has a pin. The server never alters the client's tx.
//!
//! The client's ops are applied to a copy of the head without a measurer (a tx that cannot
//! apply fails here as it would at commit), then [`fin_core::layout::font_needs`] says what
//! the result needs:
//! - each face `(family, weight, italic)` of a style in use with no pin is pinned to the
//!   catalog entry of that family's style of that weight and slope;
//! - each script a caption's shown text needs that no chosen pin covers ([`faces_for`]: the
//!   style's face, then the fallback order) is covered by the first entry of the catalog's
//!   `fallback` order whose scripts hold it, not placed in the order yet, and whose pin covers
//!   it, pinned at the end of the fallback order (its regular style, else its first). A face
//!   already pinned but out of the order is placed with a same-hash pin; a face this plan
//!   pins for a style and then places moves after the placements before it, so the ops place
//!   fallback indices in order.
//!
//! What the catalog cannot pin is a `FONT_NOT_IN_CATALOG` note, never an error: that text is
//! unmeasured (`FONT_UNPINNED`). Pins name the catalog's version; nothing reads font bytes.

use std::collections::BTreeSet;

use fin_core::layout::{faces_for, font_needs};
use fin_core::{
    CoreError, FontPin, FontPinOp, FontSource, ItemBody, MemIds, Op, Project, SCHEMA_VERSION, Tx,
    font_key,
};
use serde_json::json;

use crate::catalog::{CatalogEntry, CatalogStyle, FontCatalog};
use crate::wire::FontsPlan;

/// The pin of `style` of `entry` (its default face) at `catalog`'s version.
fn pin_of(catalog: &FontCatalog, entry: &CatalogEntry, style: &CatalogStyle) -> FontPin {
    let mut scripts = entry.scripts.clone();
    scripts.sort();
    FontPin {
        family: entry.family.clone(),
        weight: style.weight,
        italic: style.italic,
        face: style.default_face,
        sha256: style.file.sha256.clone(),
        size: style.file.size,
        format: style.file.format,
        scripts,
        source: FontSource::Catalog {
            entry: entry.id.clone(),
            style: style.id.clone(),
            catalog: catalog.catalog_version.clone(),
        },
    }
}

fn not_in_catalog(message: String, details: serde_json::Value, fix: &str) -> CoreError {
    CoreError::new("FONT_NOT_IN_CATALOG", message)
        .with_details(details)
        .with_fix(fix)
}

/// The ops `ops` need prepended on `head` (module docs), with notes for what the catalog
/// lacks. Errors are the ops' own at the head.
pub fn plan_pins(
    head: &Project,
    ops: &[Op],
    catalog: &FontCatalog,
) -> Result<FontsPlan, CoreError> {
    let tx = fin_core::normalize(Tx {
        tx_id: "fonts.plan".to_string(),
        author: "fonts.plan".to_string(),
        branch: "main".to_string(),
        base_rev: 0,
        schema_version: SCHEMA_VERSION,
        ops: ops.to_vec(),
        args_hash: None,
    })?;
    let mut after = fin_core::apply_tx(head, &tx, &mut MemIds::after(head))?.project;
    let (mut planned, mut notes) = (Vec::new(), Vec::new());
    for (family, weight, italic) in font_needs(&after).faces {
        let key = font_key(&family, weight, italic);
        if after.fonts.pins.contains_key(&key) {
            continue;
        }
        let found = catalog.entries.iter().find_map(|e| {
            let s = e
                .styles
                .iter()
                .find(|s| s.weight == weight && s.italic == italic);
            s.filter(|_| e.family == family).map(|s| (e, s))
        });
        let Some((entry, style)) = found else {
            notes.push(not_in_catalog(
                format!("the font catalog has no {key}"),
                json!({ "key": key, "family": family, "weight": weight, "italic": italic }),
                "pick a family and weight the catalog has (fin fonts ls --catalog), or import the font (fin asset add, then fin fonts pin --asset)",
            ));
            continue;
        };
        let pin = pin_of(catalog, entry, style);
        after.fonts.pins.insert(key.clone(), pin.clone());
        planned.push(FontPinOp {
            key,
            pin,
            fallback: None,
        });
    }
    let mut at = head.fonts.fallback.len();
    for script in uncovered(&after) {
        // An earlier fallback pin of this plan may cover it now.
        if !uncovered(&after).contains(&script) {
            continue;
        }
        // The first entry of the fallback order offered for the script whose face is not
        // placed yet and whose pin (the project's own, if it has one) covers it: a face
        // already placed that does not cover it cannot help, so the next entry is tried.
        let found = catalog
            .fallback
            .iter()
            .filter_map(|id| catalog.entries.iter().find(|e| e.id == *id))
            .filter(|e| e.scripts.contains(&script))
            .filter_map(|e| {
                let s = e
                    .styles
                    .iter()
                    .find(|s| s.weight == 400 && !s.italic)
                    .or(e.styles.first())?;
                let key = font_key(&e.family, s.weight, s.italic);
                let pin = after
                    .fonts
                    .pins
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| pin_of(catalog, e, s));
                Some((key, pin))
            })
            .find(|(key, pin)| {
                !after.fonts.fallback.contains(key) && pin.scripts.contains(&script)
            });
        let Some((key, pin)) = found else {
            notes.push(not_in_catalog(
                format!("the font catalog has no face for script {script} that is not already in the fallback order"),
                json!({ "script": script }),
                "import a font that covers it (fin asset add, then fin fonts pin --asset --fallback)",
            ));
            continue;
        };
        // Fallback indices must be placed in order: a face pin this plan already made moves
        // after the pins placed before it, carrying its index.
        let index = u32::try_from(at).unwrap_or(u32::MAX);
        let op = match planned.iter().position(|p| p.key == key) {
            Some(i) => FontPinOp {
                fallback: Some(index),
                ..planned.remove(i)
            },
            None => FontPinOp {
                key: key.clone(),
                pin: pin.clone(),
                fallback: Some(index),
            },
        };
        planned.push(op);
        after.fonts.pins.insert(key.clone(), pin);
        after.fonts.fallback.push(key);
        at += 1;
    }
    Ok(FontsPlan {
        ops: planned.into_iter().map(Op::from).collect(),
        notes,
    })
}

/// The scripts some caption of `project` needs that no pin it is measured with covers. A
/// caption whose style's face has no pin stays unmeasured whatever the fallbacks, so it asks
/// for none.
fn uncovered(project: &Project) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for tl in &project.timelines {
        for it in tl.tracks.iter().flat_map(|t| t.items.iter()) {
            let ItemBody::Caption { block } = &it.body else {
                continue;
            };
            let Ok(style) = fin_core::captions::resolve_style(project, block) else {
                continue;
            };
            let shown = fin_core::captions::shown_words(&block.words, it.duration);
            if let Err(missing) = faces_for(project, &style, &shown)
                && missing.iter().all(|m| m.starts_with("script:"))
            {
                out.extend(
                    missing
                        .iter()
                        .filter_map(|m| m.strip_prefix("script:"))
                        .map(str::to_string),
                );
            }
        }
    }
    out
}
