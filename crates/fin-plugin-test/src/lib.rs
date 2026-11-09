//! The plugin test harness (sub-project 8 spec §7.4; plan 8B Task 14).
//!
//! A fake host that speaks the host side of the `fin-jobs` envelope, serves `project.snapshot`
//! and `project.view` from fixture snapshots and `net.fetch` from a fake HTTPS API, records every
//! call, and runs a plugin through a conformance profile (`command` in 8B; `analyzer`,
//! `provider`, `codec` and `matte` in their batches). `fin-plugin test` is its command line.
//!
//! Built in Task 14; this is the skeleton of Task 1.
