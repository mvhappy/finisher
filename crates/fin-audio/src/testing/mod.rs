//! Signal generators and analysis helpers for tests and fixtures (feature `testing`; never in
//! a shipped build). Generators use `libm` and fixed seeds, so the A0 golden plans' sources are
//! bit-identical native and on wasm32.

pub mod analyse;
pub mod signals;
