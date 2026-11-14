//! The journal's test hooks (`test-hooks` builds only): panics, a stalled writer, keyed ids.

use std::panic::catch_unwind;

use napi_derive::napi;

use super::{JournalHandle, hook};

#[napi]
impl JournalHandle {
    /// Test hook: panics on a thread named `thread` (the hook records it); returns whether
    /// the panic was caught.
    #[napi(catch_unwind)]
    pub fn panic_on_thread_for_test(&self, thread: String) -> bool {
        std::thread::Builder::new()
            .name(thread)
            .spawn(|| {
                catch_unwind(|| panic!("fin-node test panic in {}", std::env::temp_dir().display()))
                    .is_err()
            })
            .ok()
            .and_then(|t| t.join().ok())
            .unwrap_or(false)
    }

    /// Test hook: makes the journal's writer panic at its next message.
    #[napi(catch_unwind)]
    pub fn panic_writer_for_test(&self) {
        if let Some(f) = self.journal.faults() {
            f.panic_next();
        }
    }

    /// Test hook: stalls the writer before its next message (a slow disk); true once it is.
    #[napi(catch_unwind)]
    pub fn pause_writer_for_test(&self) -> bool {
        self.journal.pause_writer()
    }

    /// Test hook: lets a stalled writer go on.
    #[napi(catch_unwind)]
    pub fn resume_writer_for_test(&self) {
        self.journal.resume_writer();
    }

    /// Test hook: `raw` as the journal stores an id (a ULID kept, anything else keyed with
    /// the install key), so a test can compute the ids it expects; `None` while off.
    #[napi(catch_unwind)]
    pub fn id_ref_for_test(&self, raw: String) -> Option<String> {
        self.journal.id_ref(&raw).map(|r| r.as_str().to_owned())
    }

    /// Test hook: how often the previous panic hook ran (it is chained after ours).
    #[napi(catch_unwind)]
    pub fn previous_hook_calls_for_test(&self) -> f64 {
        hook::PREVIOUS_CALLS.load(std::sync::atomic::Ordering::SeqCst) as f64
    }
}
