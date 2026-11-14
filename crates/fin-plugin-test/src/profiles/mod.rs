//! Conformance profiles (spec §7.4): `command` in 8B, `hook` in 7F; `analyzer` (8C),
//! `provider` (8G), `codec` and `matte` (8H) arrive with their batches.

pub mod command;
pub mod hook;
mod judge;
