//! `marker.add`: a marker on a timeline (plan 4C Task 13). `id` is a `$name` placeholder bound
//! to a new `m<n>` id (a literal id restores one: internal only, the inverse of
//! `marker.remove`). `at` and `duration` are times of at least 0 and `color` a lowercase
//! `#rrggbb[aa]` (structural); the label's length is policy. **Markers never ripple** (Decision
//! 26): no edit moves them and a rebase never maps their `at`. A timeline keeps its markers
//! ordered by `at`, then id, so a restore puts one back where it was. The diff records the
//! marker (`added`); the inverse is `marker.remove`.

use std::cmp::Ordering;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{Marker, ParamValue, Project, Timeline};
use crate::ops::marker_remove::MarkerRemove;
use crate::ops::track_add::timeline_index;
use crate::ops::util::settings::{Scope, check};
use crate::ops::util::{OpDiff, not_found, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::Footprint;
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff, EntityKind};

/// `marker.add`: a marker on `timeline` at `at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MarkerAdd {
    pub id: String,
    pub timeline: String,
    // Timeline position in seconds.
    pub at: Rational,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Rational")]
    pub duration: Option<Rational>,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub color: Option<String>,
}

impl From<MarkerAdd> for Op {
    fn from(op: MarkerAdd) -> Op {
        Op::MarkerAdd(op)
    }
}

/// Where marker `id` is: its timeline's index and its index there.
pub(crate) fn marker_index(project: &Project, id: &str) -> Result<(usize, usize), CoreError> {
    for (t, tl) in project.timelines.iter().enumerate() {
        if let Some(m) = tl.markers.iter().position(|m| m.id == id) {
            return Ok((t, m));
        }
    }
    Err(not_found("marker", id))
}

/// Orders a timeline's markers by `at`, then by id (`m2` before `m10`).
pub(crate) fn sort_markers(timeline: &mut Timeline) {
    timeline.markers.sort_by(|x, y| {
        x.at.cmp(&y.at)
            .then_with(|| match x.id.len().cmp(&y.id.len()) {
                Ordering::Equal => x.id.cmp(&y.id),
                other => other,
            })
    });
}

/// Checks a marker's fields through the marker settings.
pub(crate) fn check_marker(
    at: Rational,
    duration: Option<Rational>,
    label: &str,
    color: Option<&str>,
    policy: bool,
) -> Result<(), CoreError> {
    let mut fields = vec![
        ("at", ParamValue::Text(at.to_string())),
        ("label", ParamValue::Text(label.to_string())),
    ];
    fields.extend(duration.map(|d| ("duration", ParamValue::Text(d.to_string()))));
    fields.extend(color.map(|c| ("color", ParamValue::Text(c.to_string()))));
    for (path, mut v) in fields {
        check(Scope::Marker, path, Some(&mut v), policy)?;
    }
    Ok(())
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::MarkerAdd(a) = op else {
        return Err(wrong_op(op));
    };
    let tl = timeline_index(project, &bindings.resolve(&a.timeline)?)?;
    check_marker(
        a.at,
        a.duration,
        &a.label,
        a.color.as_deref(),
        bindings.policy(),
    )?;
    let id = bindings.create(project, &a.id, IdKind::Marker, ids)?;
    let timeline = &mut project.timelines[tl];
    timeline.markers.push(Marker {
        id: id.clone(),
        at: a.at,
        duration: a.duration,
        label: a.label.clone(),
        color: a.color.clone(),
    });
    sort_markers(timeline);
    diff.record(EntityKind::Marker, &id, ChangeKind::Added);
    Ok(vec![MarkerRemove { marker: id }.into()])
}

/// Creates the marker on its timeline.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::MarkerAdd(a) = op {
        f.creates.push(a.id.clone());
        // A removal's inverse (a literal id) puts the entity back with every field.
        f.puts_back(&a.id);
        f.uses.push(a.timeline.clone());
    }
    f
}

/// No timeline positions to map: markers never ripple (Decision 26).
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// No item-relative values.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The inverse is one `marker.remove`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "marker.add";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the new id and the timeline.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::MarkerAdd(a) => vec![&mut a.id, &mut a.timeline],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::MarkerAdd(a) => vec![&a.id, &a.timeline],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::MarkerAdd(a) => Some(a.id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
