//! Link groups (plan 4C Decision 19): move, trim and delete act on every member of the named
//! item's group; slip, slide and split act on the named item alone.

use crate::error::CoreError;
use crate::model::Project;
use crate::ops::util::{item_index, tracks};

/// The ids of the items an op on `item` acts on: `item` first, then the other members of
/// its link group in track order (and item order on a track). An unlinked item is its own
/// group. `NOT_FOUND` when no item has the id.
pub(crate) fn members(project: &Project, item: &str) -> Result<Vec<String>, CoreError> {
    let (ti, ci) = item_index(project, item)?;
    let mut out = vec![item.to_string()];
    let Some(link) = ti.get(project).items[ci].common.link.as_deref() else {
        return Ok(out);
    };
    out.extend(
        tracks(project)
            .flat_map(|t| &t.items)
            .filter(|it| it.common.link.as_deref() == Some(link) && it.id != item)
            .map(|it| it.id.clone()),
    );
    Ok(out)
}

/// The ids of the items in link group `link`, in track order (empty when there is none).
pub(crate) fn group_items(project: &Project, link: &str) -> Vec<String> {
    tracks(project)
        .flat_map(|t| &t.items)
        .filter(|it| it.common.link.as_deref() == Some(link))
        .map(|it| it.id.clone())
        .collect()
}
