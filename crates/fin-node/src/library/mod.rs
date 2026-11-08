//! Libraries in `fin-node` (plan 5D Task 8, Decision 3): what `finisherd` asks of a library
//! root that must not run on its JS thread or on libuv's pool.
//!
//! - [`library_canonical_root`] (one call per `library.add`): the canonical path, its lookup
//!   key, the volume identity and the detected storage kind of a root, on a thread of its own
//!   that the call abandons at its deadline;
//! - [`library_probe_root`] (every 60 s for an offline library, and before answers): the
//!   root's [`fin_library::RootState`] against its stored identity, one outstanding stat per
//!   root (`fin_library::root_state` is single-flight; [`rootprobe`]);
//! - [`library_probe_status`]: how many roots' latest probe timed out.
//!
//! Nothing here opens a file under a root, and nothing writes there. Values cross as JSON
//! strings, like the rest of this crate.

use std::path::{Path, PathBuf};
use std::time::Duration;

use fin_core::CoreError;
use fin_library::names;
use fin_library::volume::unreachable_code;
use fin_library::{VolumeIdentity, identity};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Serialize;
use serde_json::json;

use crate::{encode, reject};

mod rootprobe;

/// This module's own results (`Result` here is napi's).
type CoreResult<T> = std::result::Result<T, CoreError>;

/// The first stat of a root waits this long (spin-up tolerance, Decision 18).
const DEFAULT_TIMEOUT_MS: f64 = 60_000.0;

fn timeout_of(ms: Option<f64>) -> Duration {
    let ms = ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    Duration::from_millis(if ms.is_finite() && ms > 0.0 {
        ms as u64
    } else {
        DEFAULT_TIMEOUT_MS as u64
    })
}

/// `{root, rootKey, identity, storage}` of a root (the TypeScript `CanonicalRoot`).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CanonicalRoot {
    root: String,
    root_key: String,
    identity: VolumeIdentity,
    /// The detected storage kind (`local`, `network`, `sync:<provider>`), never an override.
    storage: String,
}

/// A root's lookup key (Decision 10's rules for a path): NFC, `/` separators, no trailing
/// separator, lower-cased on a case-insensitive volume.
pub(crate) fn root_key(root: &str, case_sensitive: bool) -> String {
    let unified = if cfg!(windows) {
        root.replace('\\', "/")
    } else {
        root.to_owned()
    };
    let trimmed = unified.trim_end_matches('/');
    let trimmed = if trimmed.is_empty() && unified.starts_with('/') {
        "/"
    } else {
        trimmed
    };
    names::fold(&names::nfc(trimmed.as_bytes()).nfc, case_sensitive)
}

/// True when `outer` is `inner` or a directory above it (both keys of [`root_key`]).
pub(crate) fn key_contains(outer: &str, inner: &str) -> bool {
    inner == outer
        || inner
            .strip_prefix(outer)
            .is_some_and(|rest| outer.ends_with('/') || rest.starts_with('/'))
}

fn invalid(reason: &str, path: &str) -> CoreError {
    CoreError::new(
        "LIBRARY_ROOT_INVALID",
        format!("the library root is not usable ({reason})"),
    )
    .with_details(json!({ "reason": reason, "path": path }))
}

fn unreadable(code: &str, path: &str) -> CoreError {
    CoreError::new(
        "LIBRARY_ROOT_UNREADABLE",
        format!("the library root cannot be read ({code})"),
    )
    .with_details(json!({ "code": code, "path": path }))
}

/// Windows' canonical form carries a `\\?\` prefix that the rest of Finisher's paths do not.
fn strip_verbatim(p: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return p;
    }
    let s = p.to_string_lossy();
    if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p,
    }
}

// The one place a library path is resolved with `std::fs`: a root is a directory the owner
// names, resolved once at `library.add` (never a file under it). Listed in
// `scripts/clippy-allow.json` by Task 6.
#[allow(clippy::disallowed_methods)]
fn real_directory(path: &str) -> CoreResult<PathBuf> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(invalid("relative", path));
    }
    let real = std::fs::canonicalize(p).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => invalid("notFound", path),
        _ => unreadable(unreachable_code(&e), path),
    })?;
    let real = strip_verbatim(real);
    let meta = std::fs::metadata(&real).map_err(|e| unreadable(unreachable_code(&e), path))?;
    if !meta.is_dir() {
        return Err(invalid("notDirectory", path));
    }
    Ok(real)
}

fn canonical_root(path: &str, server_data: &str) -> CoreResult<CanonicalRoot> {
    let real = real_directory(path)?;
    let root = real
        .to_str()
        .ok_or_else(|| invalid("notUtf8", path))?
        .to_owned();
    let identity = identity(&real).map_err(|e| unreadable(unreachable_code(&e), path))?;
    let root_key = root_key(&root, identity.case_sensitive);
    // A root that holds or sits inside the server data would index the server's own
    // databases, which change with every scan.
    let data = real_directory(server_data).unwrap_or_else(|_| PathBuf::from(server_data));
    if let Some(data) = data.to_str() {
        let data_key = self::root_key(data, identity.case_sensitive);
        if key_contains(&root_key, &data_key) || key_contains(&data_key, &root_key) {
            return Err(invalid("serverData", path));
        }
    }
    let storage = fin_store::storage_kind::detect(&real, None).to_string();
    Ok(CanonicalRoot {
        root,
        root_key,
        identity,
        storage,
    })
}

/// Resolves a root for `library.add`: the canonical absolute path (links resolved), its
/// lookup key, its volume identity and its detected storage kind, as `CanonicalRoot` JSON.
/// Rejects `LIBRARY_ROOT_INVALID` (`details.reason`: `relative`, `notFound`, `notDirectory`,
/// `notUtf8`, `serverData`) or `LIBRARY_ROOT_UNREADABLE` (`details.code`, `TIMEOUT` past
/// `timeoutMs`, default 60 s). The measurement runs on its own thread.
#[napi(catch_unwind, ts_return_type = "Promise<string>")]
pub fn library_canonical_root<'e>(
    env: &'e Env,
    path: String,
    server_data: String,
    timeout_ms: Option<f64>,
) -> Result<Object<'e>> {
    let (deferred, promise) = env.create_deferred::<String, _>()?;
    let timeout = timeout_of(timeout_ms);
    let (tx, rx) = std::sync::mpsc::channel();
    let shown = path.clone();
    let spawned = std::thread::Builder::new()
        .name("fin-node-library-root".into())
        .spawn(move || {
            let r = std::panic::catch_unwind(|| canonical_root(&path, &server_data))
                .unwrap_or_else(|_| Err(CoreError::new("INTERNAL", "root resolution panicked")));
            let _ = tx.send(r);
        });
    if spawned.is_err() {
        deferred.reject(reject(unreadable("THREAD", &shown)));
        return Ok(promise);
    }
    // The wait is a thread of its own too: this call must not block the JS thread.
    let waited = std::thread::Builder::new()
        .name("fin-node-library-root-wait".into())
        .spawn(move || {
            let outcome = rx
                .recv_timeout(timeout)
                .unwrap_or_else(|_| Err(unreadable("TIMEOUT", &shown)));
            match outcome.and_then(|c| {
                serde_json::to_string(&c)
                    .map_err(|e| CoreError::new("INTERNAL", format!("cannot encode result: {e}")))
            }) {
                Ok(json) => deferred.resolve(move |_| Ok(json)),
                Err(e) => deferred.reject(reject(e)),
            }
        });
    // A failed spawn drops the closure with `deferred`, which napi rejects.
    drop(waited);
    Ok(promise)
}

/// A root's state now (`RootState` JSON: `{state: "online", …identity} | {state:
/// "unreachable", code} | unmounted | volumeChanged | empty | timeout`) against the stored
/// `VolumeIdentity` JSON. One stat per root is ever outstanding; a call past `timeoutMs`
/// (default 60 s) answers `timeout` and the stat thread is left to finish (counted by
/// `libraryProbeStatus().stuckProbes`).
#[napi(catch_unwind, ts_return_type = "Promise<string>")]
pub fn library_probe_root<'e>(
    env: &'e Env,
    root: String,
    identity_json: String,
    timeout_ms: Option<f64>,
) -> Result<Object<'e>> {
    let stored: VolumeIdentity = serde_json::from_str(&identity_json).map_err(|e| {
        reject(CoreError::new(
            "INTERNAL",
            format!("the stored volume identity is not valid: {e}"),
        ))
    })?;
    let (deferred, promise) = env.create_deferred::<String, _>()?;
    rootprobe::probe(
        root,
        stored,
        timeout_of(timeout_ms),
        move |state| match encode(&state) {
            Ok(json) => deferred.resolve(move |_| Ok(json)),
            Err(e) => deferred.reject(e),
        },
    );
    Ok(promise)
}

/// What the root probes are doing now.
#[napi(object)]
pub struct LibraryProbeStatus {
    /// Roots whose latest probe timed out: their stat thread may still be blocked (a dead
    /// mount). A root leaves the count when a later probe of it answers anything else.
    pub stuck_probes: u32,
}

#[napi(catch_unwind)]
pub fn library_probe_status() -> LibraryProbeStatus {
    LibraryProbeStatus {
        stuck_probes: rootprobe::stuck_roots(),
    }
}

/// Test builds only: while `hang` is true, the stat of `root` blocks, as on a dead hard mount.
#[cfg(feature = "test-hooks")]
#[napi(catch_unwind)]
pub fn library_probe_hang_for_test(root: String, hang: bool) {
    rootprobe::hooks::set_hung(&root, hang);
}

/// Test builds only: how many stats of `root` have started.
#[cfg(feature = "test-hooks")]
#[napi(catch_unwind)]
pub fn library_probe_started_for_test(root: String) -> u32 {
    rootprobe::hooks::started(&root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_nfc_and_unify_separators() {
        assert_eq!(root_key("/mnt/media/", true), "/mnt/media");
        assert_eq!(root_key("/", true), "/");
        assert_eq!(root_key("/Mnt/É", false), "/mnt/é");
        // NFD input folds to NFC.
        assert_eq!(root_key("/m/e\u{301}", true), "/m/\u{e9}");
    }

    #[test]
    fn containment_is_by_component() {
        assert!(key_contains("/a/b", "/a/b"));
        assert!(key_contains("/a/b", "/a/b/c"));
        assert!(!key_contains("/a/b", "/a/bc"));
        assert!(key_contains("/", "/a"));
    }
}
