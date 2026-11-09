//! The family and helper tables. Each entry runs its runtime and never returns; an unknown
//! name answers `None`. The `test` families and helper exist only with `test-hooks`; 5D adds
//! `library`, sub-project 6 `media` (plan 6A Task 11).

use std::convert::Infallible;

/// Runs the worker of `family` (`warm`: send `ready` after each job), or `None`.
#[allow(unused_variables)]
pub fn run_family(family: &str, warm: bool) -> Option<Infallible> {
    match family {
        #[cfg(feature = "test-hooks")]
        "test" | "testbad" | "testplugin" => {
            fin_jobs::worker::run(crate::test_family::TestFamily::new(family, warm))
        }
        "library" => {
            // The worker profile grants no `/proc`: the volume check reads the mount table
            // through a handle opened now, before the self-sandbox entry (plan 7C Task 2).
            #[cfg(target_os = "linux")]
            if let Err(e) = fin_library::volume::linux::hold_mountinfo() {
                eprintln!("fin-workers: /proc/self/mountinfo: {e}");
            }
            fin_jobs::worker::run(crate::library::LibraryFamily::new(warm))
        }
        "media" => fin_jobs::worker::run(crate::media::MediaFamily::new(warm)),
        _ => None,
    }
}

/// Runs the helper `<name>:<variant>`, or `None`.
pub fn run_helper(spec: &str) -> Option<Infallible> {
    let (name, variant) = spec.split_once(':')?;
    #[cfg(not(feature = "test-hooks"))]
    let _ = variant;
    match name {
        #[cfg(feature = "test-hooks")]
        "test" => fin_jobs::helper::run(crate::test_family::helper::TestHelper::parse(variant)?),
        _ => None,
    }
}
