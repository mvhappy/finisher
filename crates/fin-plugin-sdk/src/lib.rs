//! The Rust SDK for Finisher code plugins (sub-project 8 spec §3, §7.1; plan 8B Task 12).
//!
//! A plugin process is a worker of 5C's `plugin` family: it speaks the `fin-jobs` envelope on
//! stdin and stdout and nothing else (plan 8B Decision 2). This crate implements the plugin side
//! of that envelope: `hello` with `v` 1 and the SDK's minor, the job loop, `ready` after each job
//! when started with a trailing `--warm`, cancel, and typed host calls (`project.snapshot`,
//! `project.view`, `net.fetch`, `log`) over `call`/`reply`. Plugins propose transactions of
//! fin-core ops; the host validates and commits them under the invoker.
//!
//! Built in Task 12; this is the skeleton of Task 1.
