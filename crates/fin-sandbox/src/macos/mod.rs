//! macOS Seatbelt (spec §8.2; plan 7C Decision 14, Task 8). [`sbpl`] generates the profile from
//! a policy (pure, built on every OS for its goldens), [`resolve`] maps its paths as the kernel
//! matches them, and [`apply`] applies it to the calling process with
//! `sandbox_init_with_parameters` (the launcher, before it `exec`s an agent or a hook, and a
//! worker's self-sandbox) or rewrites a plugin's program to `sandbox-exec -p`.

#[cfg(target_os = "macos")]
pub mod apply;
pub mod resolve;
pub mod sbpl;
