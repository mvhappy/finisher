//! `fake-codex`: argv parsing, configuration, the script, threads and the JSON-RPC runner.
//!
//! A stand-in for `codex app-server --listen stdio://` (codex-cli 0.167.0), shaped after what
//! plan 7E's Task 0 saw on the real binary. See the crate documentation for the script and the
//! observed file.

mod argv;
mod config;
mod observed;
mod runner;
mod script;
mod threads;
mod toml_value;

/// Runs the fake with the arguments after `argv[0]`; returns the process exit code.
pub fn run(args: Vec<String>) -> i32 {
    runner::main(args)
}
