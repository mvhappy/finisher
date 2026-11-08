//! What the three kinds share: params, the root, the cancel bridge and the offline check.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fin_jobs::worker::{CancelToken as JobToken, JobCtx};
use fin_jobs::{JobError, StartJob};
use fin_library::params::LIBRARY_OFFLINE;
use fin_library::process::{DatalessPolicy, dataless_policy};
use fin_library::{CancelToken, RootState, Source, Timing, VolumeIdentity, root_state};
use serde::de::DeserializeOwned;

pub fn params_invalid(msg: impl Into<String>) -> JobError {
    JobError::new("JOB_PARAMS_INVALID", msg)
}

/// `job.params` as `T`.
pub fn decode<T: DeserializeOwned>(job: &StartJob) -> Result<T, JobError> {
    serde_json::from_value(job.params.clone())
        .map_err(|e| params_invalid(format!("{}: {e}", job.kind)))
}

/// The root as a path: absolute, or `JOB_PARAMS_INVALID` (the service passes the canonical
/// root; the worker canonicalises nothing).
pub fn root_path(root: &str) -> Result<PathBuf, JobError> {
    let p = PathBuf::from(root);
    if p.is_absolute() {
        Ok(p)
    } else {
        Err(params_invalid("params.root: not an absolute path"))
    }
}

/// The path of a granted file (`name`, `out/` for the directory), or a contract error.
pub fn grant(ctx: &JobCtx<'_>, name: &str) -> Result<PathBuf, JobError> {
    ctx.file(name).map(|g| g.path.clone()).ok_or_else(|| {
        JobError::new(
            "JOB_FILE_MISSING",
            format!("the job was started without the {name} grant"),
        )
    })
}

/// `LIBRARY_OFFLINE {state}` with `retry: never` (Decision 33).
pub fn offline(state: &RootState) -> JobError {
    JobError::new(LIBRARY_OFFLINE, "the library root is not usable")
        .with_details(serde_json::json!({ "state": state }))
}

/// The OS's dataless policy as a name (`off` on macOS once `init` ran).
pub fn policy_name() -> String {
    match dataless_policy() {
        Ok(DatalessPolicy::Off) => "off",
        Ok(DatalessPolicy::On) => "on",
        Ok(DatalessPolicy::Default) => "default",
        Ok(DatalessPolicy::NotApplicable) => "notApplicable",
        Err(_) => "unknown",
    }
    .to_owned()
}

/// The source a scan reads: the native one (Linux, macOS, Windows), behind the test overlay
/// in test builds.
pub fn source(root: &Path) -> Result<Arc<dyn Source>, JobError> {
    let native: Arc<dyn Source> = Arc::new(fin_library::source::NativeSource::new(root));
    #[cfg(feature = "test-hooks")]
    {
        fin_library::overlay::wrap(root, native)
            .map_err(|e| params_invalid(format!("the test overlay: {e}")))
    }
    #[cfg(not(feature = "test-hooks"))]
    {
        Ok(native)
    }
}

/// Sets a `fin_library` cancel flag when the job's token is set, for as long as it lives.
pub struct CancelBridge {
    flag: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl CancelBridge {
    pub fn new(token: JobToken) -> Self {
        let flag = Arc::new(AtomicBool::new(token.is_cancelled()));
        let stop = Arc::new(AtomicBool::new(false));
        let (f, s) = (Arc::clone(&flag), Arc::clone(&stop));
        let thread = std::thread::Builder::new()
            .name("library-cancel".into())
            .spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    if token.is_cancelled() {
                        f.store(true, Ordering::Relaxed);
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
            .ok();
        Self { flag, stop, thread }
    }

    pub fn token(&self) -> CancelToken {
        CancelToken::from_flag(Arc::clone(&self.flag))
    }
}

impl Drop for CancelBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Judges the root against the library's stored identity: before the job, and again when a
/// file fails (a dead mount shows as missing and unreadable files; Decision 18). A passing
/// check is trusted for [`RootWatch::TRUST`].
pub struct RootWatch<'a> {
    root: &'a Path,
    stored: &'a VolumeIdentity,
    last_ok: Option<Instant>,
}

impl<'a> RootWatch<'a> {
    const TRUST: Duration = Duration::from_secs(2);

    pub fn new(root: &'a Path, stored: &'a VolumeIdentity) -> Self {
        Self {
            root,
            stored,
            last_ok: None,
        }
    }

    /// `Err(LIBRARY_OFFLINE)` unless the root is online.
    pub fn check(&mut self, timeout: Duration) -> Result<(), JobError> {
        if self.last_ok.is_some_and(|t| t.elapsed() < Self::TRUST) {
            return Ok(());
        }
        match root_state(self.root, self.stored, timeout) {
            RootState::Online(_) => {
                self.last_ok = Some(Instant::now());
                Ok(())
            }
            state => Err(offline(&state)),
        }
    }

    pub fn start(&mut self) -> Result<(), JobError> {
        self.check(Timing::default().first_stat)
    }

    pub fn recheck(&mut self) -> Result<(), JobError> {
        self.check(Timing::default().entry)
    }
}

/// Whether a file name's extension (the text after its last dot, lower-cased) is in
/// `extensions`; `None` lists everything (`kinds: all`, Decision 37).
pub fn extension_listed(name: &[u8], extensions: Option<&[String]>) -> bool {
    let Some(list) = extensions else {
        return true;
    };
    let name = String::from_utf8_lossy(name);
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && list
            .iter()
            .any(|e| e.trim_start_matches('.').to_lowercase() == ext.to_lowercase())
}
