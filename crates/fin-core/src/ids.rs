//! Entity id allocation. The store allocates durable ids at commit time; `MemIds` is the
//! in-memory allocator for tests, WASM and dry runs.

use crate::model::Project;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdKind {
    Asset,
    Track,
    Clip,
    Timeline,
}

impl IdKind {
    pub const fn prefix(self) -> &'static str {
        match self {
            IdKind::Asset => "a",
            IdKind::Track => "t",
            IdKind::Clip => "c",
            IdKind::Timeline => "tl",
        }
    }

    const fn slot(self) -> usize {
        self as usize
    }
}

pub trait IdAlloc {
    fn next(&mut self, kind: IdKind) -> String;
}

/// In-memory counters, one per kind; the next id is one past the last allocated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemIds {
    next: [u64; 4],
}

impl Default for MemIds {
    fn default() -> Self {
        MemIds { next: [1; 4] }
    }
}

/// Numeric suffix of `id` when it is exactly `prefix` followed by digits.
fn suffix(id: &str, prefix: &str) -> Option<u64> {
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

    /// Seeds each counter past the highest numeric suffix present in `project`.
    pub fn after(project: &Project) -> Self {
        let mut ids = MemIds::new();
        let mut seen = |kind: IdKind, id: &str| {
            if let Some(n) = suffix(id, kind.prefix()) {
                let slot = &mut ids.next[kind.slot()];
                *slot = (*slot).max(n.saturating_add(1));
            }
        };
        seen(IdKind::Timeline, &project.timeline.id);
        for id in project.assets.keys() {
            seen(IdKind::Asset, id);
        }
        for track in &project.timeline.tracks {
            seen(IdKind::Track, &track.id);
            for clip in &track.items {
                seen(IdKind::Clip, &clip.id);
            }
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
