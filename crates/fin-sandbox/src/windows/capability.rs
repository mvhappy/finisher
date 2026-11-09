//! Names and SIDs of AppContainer profiles and capabilities (plan 7C Decision 16, S9 Windows
//! point 1). Pure, so the derivations are tested on every OS.
//!
//! A capability SID is what `DeriveCapabilitySidsFromName` returns: `S-1-15-3-1024-` and the
//! eight little-endian 32-bit words of SHA-256 over the UTF-16LE of the name **upper-cased**
//! (the derivation is case-insensitive). Every name here is ASCII, so upper-casing is ASCII's.
//!
//! | Name | Who carries it | What the broker grants it |
//! |---|---|---|
//! | container `finisher-p-<hash>` | every `agent` and `hook` process of one project | nothing (the pipe's DACL names it) |
//! | `finisher-pv-<hash>` | the processes of one project that see it alike (same denied paths) | the project folder and libraries (read), enumerated |
//! | container `finisher-worker-<family>` | the workers of a family | nothing (each job has its capability) |
//! | container `finisher-plugin-<hash>` | one plugin in one project | its store directory, its models |
//! | `finisher-session-<sessionId>` | one agent session | its session directory and workspace |
//! | `finisher-toolchain` | every `agent`, `hook` and `plugin` process | the toolchain roots (read, execute), once |
//! | `finisher-signin-<harness>` | the agents of a harness with a sign-in store | the store directory |
//! | `finisher-hook-<hash>` | one hook call | its private temp |
//! | `finisher-job-<jobId>` | a cold worker | its job's `in/`, `out/`, temp and sources |
//! | `finisher-worker-<name>` | a warm worker | its own directory (plan Decision 19) |
//! | `finisher-call-<callId>` | one plugin call or job | its call directory and temp |
//!
//! The agent container is named after the project folder (`<hash>`: the first 24 hex digits of
//! SHA-256 over the folder, lower-cased, as UTF-8), not the project's UUID: the profile inputs
//! carry the folder, and the folder is what its grants name. Capabilities are never bound to a
//! container (S9: another container that carries a capability is admitted), so the launcher
//! alone decides which capability a process gets.

use std::path::Path;

use sha2::{Digest, Sha256};

/// `internetClient` (`S-1-15-3-1`): outbound IP, for the `agent` profile only (owner decision 5).
pub const INTERNET_CLIENT: &str = "S-1-15-3-1";
/// The capability every `agent`, `hook` and `plugin` process carries for the toolchain roots.
pub const TOOLCHAIN: &str = "finisher-toolchain";
/// The longest AppContainer profile name Windows accepts.
pub const MAX_CONTAINER_NAME: usize = 64;
/// The longest capability name accepted here (names are derived, never typed).
pub const MAX_CAPABILITY_NAME: usize = 128;

/// The capability SID of `name` (`DeriveCapabilitySidsFromName`'s capability, not its group).
pub fn capability_sid(name: &str) -> String {
    let utf16: Vec<u8> = name
        .to_ascii_uppercase()
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let digest = Sha256::digest(&utf16);
    let mut sid = String::from("S-1-15-3-1024");
    for word in digest.as_chunks::<4>().0 {
        let w = u32::from_le_bytes(*word);
        sid.push('-');
        sid.push_str(&w.to_string());
    }
    sid
}

/// The first `n` hex digits of SHA-256 over `bytes`.
fn short_hash(bytes: &[u8], n: usize) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s.truncate(n);
    s
}

/// The hash of a path as these names use it: lower-cased UTF-8 (Windows paths compare
/// without case).
fn path_hash(p: &Path, n: usize) -> String {
    short_hash(p.to_string_lossy().to_lowercase().as_bytes(), n)
}

/// One name component: ASCII letters, digits, `.`, `_`, `-`; not empty, not `.` or `..`.
pub fn check_component(field: &str, s: &str) -> Result<(), String> {
    let ok = !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{field} {s:?} is not a name component (ASCII letters, digits, '.', '_', '-')"
        ))
    }
}

fn bounded(name: String, max: usize) -> Result<String, String> {
    if name.len() > max {
        return Err(format!("{name:?} is longer than {max} characters"));
    }
    Ok(name)
}

/// The container of a project's agents and hooks.
pub fn project_container(project_dir: &Path) -> String {
    format!("finisher-p-{}", path_hash(project_dir, 24))
}

/// The capability that reads a project folder as one policy sees it (review B2): named after
/// the folder and the denied paths inside it, so a Claude Code session (its project files
/// denied, spec §4.5) and a Codex session or a hook (none denied) carry different ones, and a
/// grant made for one view never opens what another view denies. Processes of one view share
/// the grant, which is static.
pub fn project_view(project_dir: &Path, denied: &[std::path::PathBuf]) -> String {
    let mut key = project_dir.to_string_lossy().to_lowercase();
    let mut inside: Vec<String> = denied
        .iter()
        .filter(|d| d.starts_with(project_dir))
        .map(|d| d.to_string_lossy().to_lowercase())
        .collect();
    inside.sort();
    for d in inside {
        key.push('\0');
        key.push_str(&d);
    }
    format!("finisher-pv-{}", short_hash(key.as_bytes(), 24))
}

/// The container of a worker family's processes.
pub fn worker_container(family: &str) -> Result<String, String> {
    check_component("family", family)?;
    bounded(format!("finisher-worker-{family}"), MAX_CONTAINER_NAME)
}

/// The container of one plugin in one project: its store directory and its per-project data
/// directory (absent: the plugin keeps no project data, and one container serves it).
pub fn plugin_container(store_dir: &Path, plugin_data: Option<&Path>) -> String {
    let mut key = store_dir.to_string_lossy().to_lowercase();
    key.push('\0');
    if let Some(d) = plugin_data {
        key.push_str(&d.to_string_lossy().to_lowercase());
    }
    format!("finisher-plugin-{}", short_hash(key.as_bytes(), 24))
}

/// The capability of one agent session.
pub fn session(session_id: &str) -> Result<String, String> {
    check_component("sessionId", session_id)?;
    bounded(
        format!("finisher-session-{session_id}"),
        MAX_CAPABILITY_NAME,
    )
}

/// The capability of a harness's sign-in store (S9 point 12: one static capability per
/// harness, granted on the store alone).
pub fn sign_in(harness: &str) -> Result<String, String> {
    check_component("harness", harness)?;
    bounded(format!("finisher-signin-{harness}"), MAX_CAPABILITY_NAME)
}

/// The capability of one hook call, named after its private temp.
pub fn hook(tmp: &Path) -> String {
    format!("finisher-hook-{}", path_hash(tmp, 24))
}

/// The capability of a cold worker's job.
pub fn job(job_id: &str) -> Result<String, String> {
    check_component("jobId", job_id)?;
    bounded(format!("finisher-job-{job_id}"), MAX_CAPABILITY_NAME)
}

/// The capability of a warm worker (its directory's name).
pub fn warm_worker(name: &str) -> Result<String, String> {
    check_component("worker", name)?;
    bounded(format!("finisher-worker-{name}"), MAX_CAPABILITY_NAME)
}

/// The capability of one plugin call or job (its call directory's name).
pub fn call(call_id: &str) -> Result<String, String> {
    check_component("callId", call_id)?;
    bounded(format!("finisher-call-{call_id}"), MAX_CAPABILITY_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_sids_match_windows() {
        // `DeriveCapabilitySidsFromName` on the S9 host (Windows 11 build 26300).
        assert_eq!(
            capability_sid("internetClient"),
            capability_sid("INTERNETCLIENT"),
            "case-insensitive"
        );
        let s = capability_sid("finisher-session-s1");
        assert!(s.starts_with("S-1-15-3-1024-"), "{s}");
        assert_eq!(s.split('-').count(), 3 + 1 + 1 + 8, "{s}");
        assert_ne!(s, capability_sid("finisher-session-s2"));
        assert_eq!(s, capability_sid("finisher-session-s1"), "stable");
    }

    #[test]
    fn names_are_components_and_bounded() {
        assert!(session("01J0ABC").is_ok());
        for bad in ["", ".", "..", "a/b", "a\\b", "a b", "é"] {
            assert!(session(bad).is_err(), "{bad:?}");
        }
        assert!(worker_container(&"x".repeat(60)).is_err());
        assert!(project_container(Path::new("C:\\p")).len() <= MAX_CONTAINER_NAME);
        assert_eq!(
            project_container(Path::new("C:\\Projects\\A")),
            project_container(Path::new("c:\\projects\\a")),
            "paths compare without case"
        );
        assert_ne!(
            plugin_container(Path::new("C:\\s\\p"), None),
            plugin_container(Path::new("C:\\s\\p"), Some(Path::new("C:\\d")))
        );
    }
}
