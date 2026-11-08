//! Linux (spec §8.3, plan 7C Decisions 8 and 10–12): Landlock and the worker's base GPU rows;
//! the namespace that hides Unix sockets from agents and hooks (Decision 26); seccomp joins in
//! Task 4.

pub mod apply;
pub mod gpu;
pub mod landlock;
pub mod namespace;
