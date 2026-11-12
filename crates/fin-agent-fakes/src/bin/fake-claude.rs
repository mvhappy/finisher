//! The `fake-claude` binary; see `fin_agent_fakes` for the script format.

fn main() {
    std::process::exit(fin_agent_fakes::claude::run(
        std::env::args().skip(1).collect(),
    ));
}
