//! The chained panic hook (Decision 26): `error.panic` into every open journal, metadata only
//! under diagnostics (a repo-relative location or none, the message's keyed hash, the
//! thread), the scrubbed message as content; never on the journal's writer thread.

use std::panic::{AssertUnwindSafe, PanicHookInfo, catch_unwind};
use std::sync::Once;
use std::time::Duration;

use fin_core::events::{Source, SrcLoc};
use serde_json::{Value, json};

use super::JOURNALS;

#[cfg(feature = "test-hooks")]
pub(super) static PREVIOUS_CALLS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

static HOOK: Once = Once::new();

/// Chains a hook that records `error.panic` into every open journal, then runs the hook
/// that was installed before (the default one prints the message). Never on the journal's
/// writer thread (`Journal::on_writer_thread`): `record_sync` there would wait on itself.
pub(crate) fn install_panic_hook() {
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !fin_journal::Journal::on_writer_thread() {
                let _ = catch_unwind(AssertUnwindSafe(|| report_panic(info)));
            }
            #[cfg(feature = "test-hooks")]
            PREVIOUS_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            previous(info);
        }));
    });
}

/// The panicking thread, by the names fin-node gives its threads.
fn thread_kind(name: Option<&str>) -> &'static str {
    match name {
        Some("main") => "main",
        Some("fin-node-worker") => "worker",
        Some("fin-node-maintenance") => "tick",
        Some("fin-journal") => "writer",
        _ => "other",
    }
}

/// The `error.panic` wire form of `info` (Decision 26): a repo-relative location or none
/// (`thirdParty`), the thread, and the message (scrubbed by the journal, content).
pub(crate) fn panic_payload(info: &PanicHookInfo<'_>) -> Value {
    let location = info
        .location()
        .and_then(|l| SrcLoc::from_file_line(l.file(), l.line()));
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".to_string());
    let mut v = json!({
        "where": "finNode",
        "thirdParty": location.is_none(),
        "thread": thread_kind(std::thread::current().name()),
        "message": message,
    });
    if let Some(l) = location {
        v["location"] = json!(l);
    }
    v
}

fn report_panic(info: &PanicHookInfo<'_>) {
    // `try_lock`: a panic while the registry is held must not deadlock on it.
    let journals: Vec<fin_journal::Journal> = match JOURNALS.try_lock() {
        Ok(all) => all.iter().map(|(_, j)| j.clone()).collect(),
        Err(_) => return,
    };
    if journals.iter().all(|j| j.gate().is_none()) {
        return;
    }
    let payload = panic_payload(info);
    for j in journals {
        let Some(session) = j.session().map(str::to_owned) else {
            continue;
        };
        let _ = j.record_value_sync(
            Source::FinNode,
            &session,
            "error.panic",
            1,
            None,
            payload.clone(),
            Duration::from_millis(250),
        );
    }
}
