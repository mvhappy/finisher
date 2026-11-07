//! The compositor (sub-project 6 spec §5, §18): geometry, coverage, mip levels (shared by both
//! compositors), the CPU reference compositor (`reference`, the oracle, feature `reference`)
//! and, from 6A-2, the wgpu render graph. Linked only by `fin-workers` (and `fin-wasm` later).

pub mod coverage;
pub mod geometry;
pub mod mips;
mod output;
#[cfg(feature = "reference")]
pub mod reference;
mod source;

pub use coverage::coverage;
pub use geometry::{RectF64, base_box, crop_quad, invert, layer_matrix, transform_point};
pub use mips::mip_level;
pub use output::{RenderOutput, Skipped};
pub use source::{LayerIssue, SourceFrame, SourceProvider};
