//! The `fake-codex` binary; see `fin_agent_fakes` for the script format.

fn main() {
    std::process::exit(fin_agent_fakes::codex::run(
        std::env::args().skip(1).collect(),
    ));
}
