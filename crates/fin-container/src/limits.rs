//! What a hostile file may make `fin-container` do (spec §6.1): every box size is checked
//! against the bytes left and `max_box_bytes`, and every declared count against its box's
//! size, before anything is allocated or handed to `mp4-atom`.
//!
//! WebM/Matroska reads the same limits: an EBML element is a box (its size checked against
//! the bytes its parent has left, nesting against `max_depth`, a body read whole only within
//! `max_box_bytes`), every element outside clusters counts against `max_boxes`, and the blocks
//! inside clusters count as samples instead: `max_samples` frames per track (laced audio
//! frames each), and `maxPresentedFrames` as MP4. Unknown sizes are accepted only for
//! `Segment` and `Cluster`.

/// Container limits; exceeding one is `MEDIA_LIMIT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Boxes visited in the whole file.
    pub max_boxes: u32,
    /// Samples in one track (all fragments together).
    pub max_samples: u32,
    /// The largest box read into memory (`mdat` is never read whole).
    pub max_box_bytes: u64,
    /// Box nesting depth.
    pub max_depth: u8,
}

impl Default for Limits {
    /// 1 M boxes, 10 M samples, 64 MiB, depth 16.
    fn default() -> Self {
        Limits {
            max_boxes: 1 << 20,
            max_samples: 10_000_000,
            max_box_bytes: 64 << 20,
            max_depth: 16,
        }
    }
}
