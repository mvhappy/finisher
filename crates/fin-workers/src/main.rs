//! `fin-workers`: the one worker binary (spec §6.2). Arguments only; the families and helpers
//! are in `families.rs`.
//!
//! ```text
//! fin-workers <family> [--warm]        run a family's worker (stdin/stdout envelope); the engine
//!                                       appends `--warm` for a warm spawn
//! fin-workers --helper <name>:<variant> run a helper (test builds: test:ok|exitEarly|hang|spawn|call)
//! fin-workers watchdog                  the Unix host watchdog (Task 4)
//! fin-workers --version                 prints the envelope id
//! ```
//!
//! Exit 2 for usage errors and unknown families or helpers. No self-sandbox function is
//! registered yet: sub-project 7's `fin-sandbox` will call `fin_jobs::sandbox::register` here,
//! before any runtime starts, so until then a requested self-sandbox exits 78 (fail closed).

mod families;
#[cfg(feature = "test-hooks")]
mod test_family;

const USAGE: i32 = 2;

fn usage(msg: &str) -> ! {
    eprintln!("fin-workers: {msg}");
    eprintln!("usage: fin-workers <family> [--warm] | --helper <name>:<variant> | --version");
    std::process::exit(USAGE)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(feature = "test-hooks")]
    if let Some(code) = test_family::hidden_main(&args) {
        std::process::exit(code);
    }
    let first = args.first().map(String::as_str);
    match first {
        None => usage("no family"),
        Some("--version") => println!("{}", fin_jobs::envelope_id()),
        Some("--helper") => {
            let spec = args
                .get(1)
                .map(String::as_str)
                .unwrap_or_else(|| usage("--helper needs <name>:<variant>"));
            if args.len() > 2 {
                usage("too many arguments");
            }
            if families::run_helper(spec).is_none() {
                usage(&format!("unknown helper {spec:?}"));
            }
        }
        Some(family) => {
            let warm = match args.get(1).map(String::as_str) {
                None => false,
                Some("--warm") if args.len() == 2 => true,
                Some(other) => usage(&format!("unknown argument {other:?}")),
            };
            if families::run_family(family, warm).is_none() {
                usage(&format!("unknown family {family:?}"));
            }
        }
    }
}
