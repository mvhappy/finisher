//! `fin-workers`: the one worker binary (spec §6.2). Arguments only; the families and helpers
//! are in `families.rs`.
//!
//! ```text
//! fin-workers <family> [--warm]        run a family's worker (stdin/stdout envelope); the engine
//!                                       appends `--warm` for a warm spawn
//! fin-workers media --replay <jobdir> [--params <file>]
//!                                       runs a media job directory without a host
//! fin-workers --helper <name>:<variant> run a helper (test builds: test:ok|exitEarly|hang|spawn|call)
//! fin-workers watchdog <host pid>       the Unix host watchdog (`watchdog.rs`)
//! fin-workers --spawn-broker            the Windows host's spawn broker image: created
//!                                       suspended and never resumed; exits 0 at once if it is
//!                                       (first thing in `main`)
//! fin-workers --version                 prints the envelope id
//! ```
//!
//! Exit 2 for usage errors and unknown families or helpers. No self-sandbox function is
//! registered yet: sub-project 7's `fin-sandbox` will call `fin_jobs::sandbox::register` here,
//! before any runtime starts, so until then a requested self-sandbox exits 78 (fail closed).

mod families;
mod library;
mod media;
#[cfg(feature = "test-hooks")]
mod test_family;
#[cfg(unix)]
mod watchdog;

const USAGE: i32 = 2;

fn usage(msg: &str) -> ! {
    eprintln!("fin-workers: {msg}");
    eprintln!(
        "usage: fin-workers <family> [--warm] | media --replay <jobdir> [--params <file>] | --helper <name>:<variant> | watchdog <host pid> | --version"
    );
    std::process::exit(USAGE)
}

fn main() {
    // The spawn broker (Windows) never runs; should something resume it, it does nothing.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "--spawn-broker")
    {
        std::process::exit(0);
    }
    #[cfg(feature = "test-hooks")]
    test_family::capture_start();
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(feature = "test-hooks")]
    if let Some(code) = test_family::hidden_main(&args) {
        std::process::exit(code);
    }
    let first = args.first().map(String::as_str);
    match first {
        None => usage("no family"),
        Some("--version") => println!("{}", fin_jobs::envelope_id()),
        #[cfg(unix)]
        Some("watchdog") => std::process::exit(watchdog::run(&args[1..])),
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
        Some("media") if args.get(1).is_some_and(|a| a == "--replay") => {
            std::process::exit(media::replay::main(&args[2..]))
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
