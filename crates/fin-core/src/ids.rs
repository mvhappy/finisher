//! Entity id allocation. The store allocates durable ids at commit time; `MemIds` is the
//! in-memory allocator for tests, WASM and dry runs.

use crate::model::{ItemBody, Project};

/// The kinds of allocated ids, one prefix each (plan 4C Decision 10, owner): items have one
/// kind per item kind. No prefix is another one followed by digits, so an id names at most one
/// kind ([`suffix`]). Font pins are keyed by face, not by id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdKind {
    Asset,
    Track,
    /// Clip items.
    Clip,
    Timeline,
    /// Branch ids (`br<n>`, sub-project 4B). The store keeps the counter from 4A on so that
    /// importing another machine's branches raises it too (spec §3.3).
    Branch,
    /// Caption items.
    Caption,
    /// Graphic (Lottie template) items.
    Graphic,
    /// Image items.
    Image,
    /// Adjustment items.
    Adjustment,
    /// Nested-timeline items.
    Nested,
    Marker,
    Transition,
    Bin,
    /// Effects, from one project-wide counter, addressed as `(item, effect)`.
    Effect,
    /// Link groups.
    Link,
    /// Project caption styles.
    Style,
    /// Keyframes, from one project-wide counter, addressed as `(item, path, key)` (spec
    /// §5.12): a split copies keys with their ids.
    Keyframe,
}

impl IdKind {
    pub const fn prefix(self) -> &'static str {
        match self {
            IdKind::Asset => "a",
            IdKind::Track => "t",
            IdKind::Clip => "c",
            IdKind::Timeline => "tl",
            IdKind::Branch => "br",
            IdKind::Caption => "cap",
            IdKind::Graphic => "g",
            IdKind::Image => "img",
            IdKind::Adjustment => "adj",
            IdKind::Nested => "nst",
            IdKind::Marker => "m",
            IdKind::Transition => "tr",
            IdKind::Bin => "b",
            IdKind::Effect => "fx",
            IdKind::Link => "lk",
            IdKind::Style => "st",
            IdKind::Keyframe => "kf",
        }
    }

    /// The id kind of an item body.
    pub const fn of_item(body: &ItemBody) -> IdKind {
        match body {
            ItemBody::Clip { .. } => IdKind::Clip,
            ItemBody::Caption { .. } => IdKind::Caption,
            ItemBody::Graphic { .. } => IdKind::Graphic,
            ItemBody::Image { .. } => IdKind::Image,
            ItemBody::Adjustment {} => IdKind::Adjustment,
            ItemBody::Nested { .. } => IdKind::Nested,
        }
    }

    /// Whether ids of this kind name items.
    pub const fn is_item(self) -> bool {
        matches!(
            self,
            IdKind::Clip
                | IdKind::Caption
                | IdKind::Graphic
                | IdKind::Image
                | IdKind::Adjustment
                | IdKind::Nested
        )
    }

    const fn slot(self) -> usize {
        self as usize
    }
}

pub trait IdAlloc {
    fn next(&mut self, kind: IdKind) -> String;
}

/// Every kind, in declaration order.
pub const ID_KINDS: [IdKind; 17] = [
    IdKind::Asset,
    IdKind::Track,
    IdKind::Clip,
    IdKind::Timeline,
    IdKind::Branch,
    IdKind::Caption,
    IdKind::Graphic,
    IdKind::Image,
    IdKind::Adjustment,
    IdKind::Nested,
    IdKind::Marker,
    IdKind::Transition,
    IdKind::Bin,
    IdKind::Effect,
    IdKind::Link,
    IdKind::Style,
    IdKind::Keyframe,
];

/// In-memory counters, one per kind; the next id is one past the last allocated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemIds {
    next: [u64; ID_KINDS.len()],
}

impl Default for MemIds {
    fn default() -> Self {
        MemIds {
            next: [1; ID_KINDS.len()],
        }
    }
}

/// Numeric suffix of `id` when it is exactly `prefix` followed by digits.
pub(crate) fn suffix(id: &str, prefix: &str) -> Option<u64> {
    let digits = id.strip_prefix(prefix)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

impl MemIds {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seeds each counter past the highest numeric suffix present in `project`, over every
    /// id it holds ([`Project::entity_ids`]). Each id raises the one kind whose prefix it
    /// carries, whichever entity holds it, so an id is never handed out twice. A `Project`
    /// holds no branch ids, so the branch counter starts at 1.
    pub fn after(project: &Project) -> Self {
        let mut ids = MemIds::new();
        let mut seen = |id: &str| {
            for kind in ID_KINDS {
                if let Some(n) = suffix(id, kind.prefix()) {
                    let slot = &mut ids.next[kind.slot()];
                    *slot = (*slot).max(n.saturating_add(1));
                }
            }
        };
        for id in project.entity_ids() {
            seen(id);
        }
        ids
    }
}

impl IdAlloc for MemIds {
    fn next(&mut self, kind: IdKind) -> String {
        let n = &mut self.next[kind.slot()];
        let id = format!("{}{}", kind.prefix(), n);
        *n += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_ids_use_br_and_do_not_collide_with_other_prefixes() {
        let mut ids = MemIds::new();
        assert_eq!(ids.next(IdKind::Branch), "br1");
        assert_eq!(ids.next(IdKind::Branch), "br2");
        // Each kind's ids are only its own: `br1` is no other kind's, `tl1` is not a track's.
        for kind in ID_KINDS {
            let own = format!("{}7", kind.prefix());
            for other in ID_KINDS {
                assert_eq!(
                    suffix(&own, other.prefix()).is_some(),
                    kind == other,
                    "{own}"
                );
            }
        }
    }
}
