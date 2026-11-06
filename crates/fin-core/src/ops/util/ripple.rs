//! Ripple, placement and diff-recording helpers.

use serde_json::json;

use crate::error::CoreError;
use crate::model::{Clip, Track};
use crate::ops::Edge;
use crate::ops::util::end;
use crate::time::Rational;
use crate::tx::{Change, ChangeKind, Diff, EntityKind, Shift};

/// The clip on `track` strictly containing `at` (`start < at < end`), if any.
pub(crate) fn clip_containing(track: &Track, at: Rational) -> Result<Option<&Clip>, CoreError> {
    for clip in &track.items {
        if clip.start < at && at < end(clip)? {
            return Ok(Some(clip));
        }
    }
    Ok(None)
}

pub(crate) fn insert_inside(clip: &Clip, at: Rational) -> Result<CoreError, CoreError> {
    let (start, end) = (clip.start, end(clip)?);
    let nearest = if at.checked_sub(start)? <= end.checked_sub(at)? {
        start
    } else {
        end
    };
    Ok(CoreError::new(
        "INSERT_INSIDE_CLIP",
        format!("{at} is inside clip {}", clip.id),
    )
    .with_details(
        json!({ "clip": clip.id, "at": at, "start": start, "end": end, "nearest": nearest }),
    )
    .with_fix(format!(
        "insert at the clip's start ({start}) or end ({end}); nearest is {nearest}"
    )))
}

/// Ripple: moves every item on `track` starting at or after `from` by `by`.
pub(crate) fn ripple(track: &mut Track, from: Rational, by: Rational) -> Result<(), CoreError> {
    if by.is_zero() {
        return Ok(());
    }
    for clip in track.items.iter_mut().filter(|c| c.start >= from) {
        clip.start = clip.start.checked_add(by)?;
    }
    Ok(())
}

/// Inserts `clip` keeping `track.items` sorted by `start` (after any item with the same start).
pub(crate) fn place(track: &mut Track, clip: Clip) {
    let i = track.items.partition_point(|c| c.start <= clip.start);
    track.items.insert(i, clip);
}

/// The diff sink of one op: everything it records carries the op's index (plan 4C
/// Decision 4).
pub(crate) struct OpDiff<'a> {
    diff: &'a mut Diff,
    op: u32,
}

impl<'a> OpDiff<'a> {
    /// Records into `diff` as op `op` of the transaction.
    pub(crate) fn new(diff: &'a mut Diff, op: u32) -> OpDiff<'a> {
        OpDiff { diff, op }
    }

    fn push(&mut self, kind: EntityKind, id: &str, change: ChangeKind) -> &mut Change {
        self.diff.changes.push(Change {
            kind,
            id: id.to_string(),
            change,
            op: self.op,
            edge: None,
            path: None,
            by: None,
        });
        let last = self.diff.changes.len() - 1;
        &mut self.diff.changes[last]
    }

    /// The entity `id` was added, removed or changed as a whole.
    pub(crate) fn record(&mut self, kind: EntityKind, id: &str, change: ChangeKind) {
        self.push(kind, id, change);
    }

    /// One edge of `id` moved; `by` is the content offset of a start-edge move (Decision 13).
    pub(crate) fn record_edge(
        &mut self,
        kind: EntityKind,
        id: &str,
        edge: Edge,
        by: Option<Rational>,
    ) {
        let c = self.push(kind, id, ChangeKind::Updated);
        c.edge = Some(edge);
        c.by = by;
    }

    /// One parameter of `id` (its registry path) was set.
    #[allow(dead_code, reason = "param.set (plan 4C Task 10) records paths")]
    pub(crate) fn record_path(&mut self, kind: EntityKind, id: &str, path: &str) {
        self.push(kind, id, ChangeKind::Updated).path = Some(path.to_string());
    }

    /// Items on `track` at or after `from` moved by `by`.
    pub(crate) fn shift(&mut self, track: &str, from: Rational, by: Rational) {
        self.diff.shifts.push(Shift {
            track: track.to_string(),
            from,
            by,
            op: self.op,
        });
    }
}
