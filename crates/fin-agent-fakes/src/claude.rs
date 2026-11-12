//! `fake-claude`: argv parsing, the script, the transcript and the stream-JSON runner.

mod argv;
mod observed;
mod runner;
mod script;
mod transcript;

/// Runs the fake with the arguments after `argv[0]`; returns the process exit code.
pub fn run(args: Vec<String>) -> i32 {
    runner::main(args)
}
