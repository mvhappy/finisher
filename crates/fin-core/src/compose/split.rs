//! `compose::split_linked` (plan 4C Decision 17): `item.split` cuts one item; a cut through a
//! link group splits each member at the same time and links the right parts.

use serde_json::json;

use crate::compose::Composed;
use crate::error::CoreError;
use crate::model::Project;
use crate::ops::util::{item_index, members};
use crate::ops::{ItemLink, ItemSplit};
use crate::time::Rational;

const LINK: &str = "$link";

/// Ops cutting `item` and every member of its link group strictly containing timeline time
/// `at`: an `item.split` each (the named item first, its right part `$split`, the others'
/// `$split2`, `$split3`, …), then `item.link` of the right parts when there are two or more.
/// The named item's split is sent even when `at` is not inside it (the op refuses it).
/// `echo` is `{ splits: [{ item, offset, id }], link: "$link" | null }`.
pub fn split_linked(project: &Project, item: &str, at: Rational) -> Result<Composed, CoreError> {
    let mut ops = Vec::new();
    let mut splits = Vec::new();
    let mut rights = Vec::new();
    for (n, member) in members(project, item)?.into_iter().enumerate() {
        let (t, i) = item_index(project, &member)?;
        let it = &t.get(project).items[i];
        let inside = it.start < at && at < it.end()?;
        if n > 0 && !inside {
            continue;
        }
        let id = if n == 0 {
            "$split".to_string()
        } else {
            format!("$split{}", rights.len() + 1)
        };
        let offset = at.checked_sub(it.start)?;
        splits.push(json!({ "item": member, "offset": offset, "id": id }));
        ops.push(
            ItemSplit {
                item: member,
                offset,
                id: id.clone(),
            }
            .into(),
        );
        rights.push(id);
    }
    let link = (rights.len() > 1).then(|| {
        ops.push(
            ItemLink {
                items: rights,
                id: LINK.to_string(),
            }
            .into(),
        );
        LINK
    });
    Ok(Composed {
        ops,
        echo: json!({ "splits": splits, "link": link }),
    })
}
