//! The compositor (sub-project 6 spec §5, §18): geometry, coverage, mip levels (shared by both
//! compositors), the CPU reference compositor (`reference`, the oracle, feature `reference`)
//! and the wgpu render graph (`Gpu`, `Compositor`), checked against the reference within T1
//! (`compare`). Linked only by `fin-workers` (and `fin-wasm` later). The wgpu half is not built
//! for WASI, where only the reference runs.

#[cfg(not(target_os = "wasi"))]
mod blend;
pub mod compare;
pub mod coverage;
#[cfg(not(target_os = "wasi"))]
mod device;
pub mod geometry;
#[cfg(not(target_os = "wasi"))]
mod gpu;
#[cfg(not(target_os = "wasi"))]
mod ingest;
#[cfg(not(target_os = "wasi"))]
mod layer;
#[cfg(not(target_os = "wasi"))]
mod mipgen;
pub mod mips;
mod output;
#[cfg(not(target_os = "wasi"))]
mod pipelines;
pub mod placement;
#[cfg(not(target_os = "wasi"))]
mod pool;
#[cfg(not(target_os = "wasi"))]
mod readback;
#[cfg(feature = "reference")]
pub mod reference;
mod source;

#[cfg(not(target_os = "wasi"))]
pub use device::{AdapterChoice, AdapterSummary, Gpu, driver_major, map_error};
#[cfg(not(target_os = "wasi"))]
pub use gpu::{Compositor, Stage};
#[cfg(not(target_os = "wasi"))]
pub use pool::PoolStats;

pub use coverage::coverage;
pub use geometry::{RectF64, base_box, crop_quad, invert, layer_matrix, transform_point};
pub use mips::mip_level;
pub use output::{RenderOutput, Skipped};
pub use source::{LayerIssue, SourceFrame, SourceProvider};
