//! `fake-claude`: argv parsing, configuration, the script, settings, the transcript and the
//! stream-JSON runner.

mod argv;
mod config;
mod loads;
mod observed;
mod runner;
mod script;
mod settings;
mod transcript;

/// Runs the fake with the arguments after `argv[0]`; returns the process exit code.
pub fn run(args: Vec<String>) -> i32 {
    runner::main(args)
}
