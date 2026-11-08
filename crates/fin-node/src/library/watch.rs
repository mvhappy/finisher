//! `libraryWatchStart` (Linux; plan 5D Decision 30, the Tasks 9–11 review): the per-directory
//! inotify watch of a local root, [`fin_library::watch::DirWatch`], on a thread of its own.
//! Its messages reach `callback` as JSON strings on the calling JS thread (the indexer
//! thread): `{ready}`, `{dirs: [RawRel]}`, `{all: true}`, `{failed: code}`. The callback does
//! not keep the thread's event loop alive.

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;

type WatchFn = ThreadsafeFunction<String, (), String, Status, false, true>;

/// A running watch of one library root; `close()` stops it (idempotent).
#[napi]
pub struct LibraryWatch {
    #[cfg(target_os = "linux")]
    inner: Option<fin_library::watch::DirWatch>,
}

#[napi]
impl LibraryWatch {
    /// Stops the watch: no message is delivered after it returns.
    #[napi(catch_unwind)]
    pub fn close(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(w) = self.inner.take() {
            w.close();
        }
    }
}

/// Starts watching `root` (a library's canonical root): at most `maxWatches` directories.
/// Fails with `WATCH_UNSUPPORTED` on other systems (they use Node's native recursive watch).
#[napi(catch_unwind)]
pub fn library_watch_start(
    root: String,
    max_watches: u32,
    #[napi(ts_arg_type = "(json: string) => void")] callback: Function<String, ()>,
) -> Result<LibraryWatch> {
    let tsfn: WatchFn = callback
        .build_threadsafe_function::<String>()
        .weak::<true>()
        .build()?;
    start(root, max_watches, tsfn)
}

#[cfg(target_os = "linux")]
fn start(root: String, max_watches: u32, tsfn: WatchFn) -> Result<LibraryWatch> {
    let sink = Box::new(move |m: fin_library::watch::WatchMsg| {
        tsfn.call(m.to_json(), ThreadsafeFunctionCallMode::NonBlocking);
    });
    let w = fin_library::watch::DirWatch::start(root.into(), max_watches as usize, sink)
        .map_err(|e| Error::from_reason(format!("WATCH_ERROR: {e}")))?;
    Ok(LibraryWatch { inner: Some(w) })
}

#[cfg(not(target_os = "linux"))]
fn start(_root: String, _max_watches: u32, _tsfn: WatchFn) -> Result<LibraryWatch> {
    let _ = ThreadsafeFunctionCallMode::NonBlocking;
    Err(Error::from_reason("WATCH_UNSUPPORTED"))
}
