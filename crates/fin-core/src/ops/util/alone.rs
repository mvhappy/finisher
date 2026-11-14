//! The post-op link pass (4C Tasks 9–10 review, B2). An op that takes items out of the project
//! (a carve, a removal, a track or timeline removal) can leave a link group with one member,
//! and a one-member group would block linking that item again (`ALREADY_LINKED`). So after
//! such an op each group it left with one member is dissolved: the member's `link` is cleared,
//! the diff records the group `removed` and the member's `link` parameter, and the op's inverse
//! ends with an internal `item.link` of that member to the group's literal id: the op's own
//! inverse restores the removed members into the group first, and the member joins them. Its
//! inverse in turn (a redo's) is the member leaving the group (`item.unlink` with `items`), so
//! the redo's re-run removal finds no member left alone and dissolves nothing twice.
//!
//! The candidates are the groups of the items the op's inverse restores (`item.restore`,
//! `track.restore`, `timeline.restore`) that are no longer in the project: what a move restores
//! is still there, so a move never dissolves anything.

use std::collections::BTreeSet;

use crate::error::CoreError;
use crate::model::{Item, Project};
use crate::ops::item_link::LINK;
use crate::ops::util::{OpDiff, find_item, group_items};
use crate::ops::{ItemLink, Op};
use crate::tx::{ChangeKind, Diff, EntityKind};

/// The items `op` (an inverse op) brings back.
fn restored(op: &Op) -> Vec<&Item> {
    match op {
        Op::ItemRestore(r) => vec![&r.item],
        Op::TrackRestore(r) => r.track.items.iter().collect(),
        Op::TimelineRestore(r) => r
            .timeline
            .tracks
            .iter()
            .flat_map(|t| t.items.iter())
            .collect(),
        _ => Vec::new(),
    }
}

/// Runs the pass after an op that recorded into `d` and returned `inverse`; returns the whole
/// inverse: `inverse`, then an `item.link` joining each dissolved group's member back.
pub(crate) fn dissolve_alone(
    project: &mut Project,
    d: &mut OpDiff<'_>,
    inverse: Vec<Op>,
) -> Result<Vec<Op>, CoreError> {
    let groups: BTreeSet<String> = inverse
        .iter()
        .flat_map(restored)
        .filter(|item| find_item(project, &item.id).is_none())
        .filter_map(|item| item.common.link.clone())
        .collect();
    let mut out = inverse;
    for group in groups {
        let members = group_items(project, &group);
        let [member] = members.as_slice() else {
            continue;
        };
        let Some((t, i)) = find_item(project, member) else {
            continue;
        };
        let track = t.get_mut(project);
        track.items[i].common.link = None;
        d.record(EntityKind::Link, &group, ChangeKind::Removed);
        d.record_path(&track.id, member, LINK);
        out.push(
            ItemLink {
                items: vec![member.clone()],
                id: group,
            }
            .into(),
        );
    }
    Ok(out)
}

/// How many groups the pass dissolved, read off the op's own diff (each recorded one `link`
/// change), and that diff without what the pass recorded (each group's `link` change and its
/// member's `link` parameter, which follows it).
pub(crate) fn split_dissolved(own: &Diff) -> (usize, Diff) {
    let mut rest = own.clone();
    let mut n = 0;
    let mut skip_member = false;
    rest.changes.retain(|c| {
        if skip_member && c.kind == EntityKind::Item && c.path.as_deref() == Some(LINK) {
            skip_member = false;
            return false;
        }
        if c.kind == EntityKind::Link && c.change == ChangeKind::Removed {
            n += 1;
            skip_member = true;
            return false;
        }
        true
    });
    (n, rest)
}
