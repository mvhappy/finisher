//! A process lowers its own QoS after `hello` (5C-1 Decision 21, macOS).
//!
//! The background QoS class starves a process under load (`taskpolicy -b echo` was measured
//! above 100 s), so a healthy worker spawned at `background` would miss `hello_ms` and
//! `hung_ms`. The host therefore spawns it at `utility` with `FIN_QOS=background`, and the
//! worker and helper runtimes call [`lower_after_hello`] once their `hello` is out: a loaded
//! machine delays the jobs, never the handshake. Elsewhere the host sets the nice value at
//! spawn (CFS weights do not starve) and the variable is never set.

/// The variable naming the class to lower to (`background`).
pub const ENV: &str = "FIN_QOS";

/// Lowers this process to the class `FIN_QOS` names, if any (best effort).
pub fn lower_after_hello() {
    if std::env::var_os(ENV).is_some_and(|v| v == "background") {
        lower_to_background();
    }
}

#[cfg(target_os = "macos")]
fn lower_to_background() {
    use std::ffi::{c_int, c_uint};
    unsafe extern "C" {
        // <sys/resource.h> (libSystem).
        fn setpriority(which: c_int, who: c_uint, prio: c_int) -> c_int;
    }
    const PRIO_DARWIN_PROCESS: c_int = 4;
    const PRIO_DARWIN_BG: c_int = 0x1000;
    // SAFETY: changes this process's own scheduling policy; no memory is passed.
    unsafe { setpriority(PRIO_DARWIN_PROCESS, 0, PRIO_DARWIN_BG) };
}

#[cfg(not(target_os = "macos"))]
fn lower_to_background() {}
