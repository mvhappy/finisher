//! The server's diagnostics journal (plan D, Task D4; spec §21.1): one `fin_journal::Journal`
//! per server data, like the font manager, opened by `finisherd` through the napi `Journal`
//! class after it read `settings.json`, and found by the engines opened on the same server
//! data ([`for_server`]).
//!
//! - Calls that only queue or read memory (`record`, `observe`, `applySettings`, `consent`,
//!   `statusNow`, the project policies) run on the JS thread and never wait on I/O.
//! - Calls that flush or read the files (`status`, `export`, `purge`, `endSession`, `close`)
//!   run on a thread of their own and settle a promise.
//! - `recordSync` waits for the writer for at most its timeout (an uncaught error, the
//!   process about to exit).
//! - A chained panic hook ([`install_panic_hook`]) records `error.panic` for panics in
//!   fin-node, except on the journal's writer thread (it would wait on itself; the writer
//!   suspends instead).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use fin_core::CoreError;
use fin_core::events::names::{EVAL_KEYS, RPC_METHODS};
use fin_core::events::payloads::SessionStart;
use fin_core::events::{Code, CollectPolicy, Ctx, Purposes, Source};
use fin_journal::{EnvOverrides, Filter, JournalOptions, Metric, PurgeSelector, server_scrub};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Deserialize;
use serde_json::{Value, json};

mod ctx;
mod hook;

use ctx::{RawCtx, ctx_of, event_invalid};
pub(crate) use hook::install_panic_hook;

use crate::engine::lock;
use crate::{encode, guarded, reject};

/// Every open journal, by canonical server data.
pub(super) static JOURNALS: Mutex<Vec<(PathBuf, fin_journal::Journal)>> = Mutex::new(Vec::new());

fn key_of(server_data: &Path) -> PathBuf {
    std::fs::canonicalize(server_data).unwrap_or_else(|_| server_data.to_path_buf())
}

/// The journal open on `server_data`, or a disabled one (records nothing).
pub(crate) fn for_server(server_data: &Path) -> fin_journal::Journal {
    let key = key_of(server_data);
    lock(&JOURNALS)
        .iter()
        .find(|(p, _)| *p == key)
        .map(|(_, j)| j.clone())
        .unwrap_or_else(fin_journal::Journal::disabled)
}

/// An open engine's project in the journal (`fin_journal::ProjectScope`, ref-counted per id):
/// its flag while the engine is open, unknown again when the last scope drops. Unknown
/// projects record nothing, so every engine takes one at open.
pub(crate) type ProjectGuard = fin_journal::ProjectScope;

fn invalid(what: &str, e: impl std::fmt::Display) -> Error {
    reject(
        CoreError::new("INVALID_ARGS", format!("{what} does not decode: {e}"))
            .with_details(json!({ "reason": e.to_string() })),
    )
}

fn parse<T: serde::de::DeserializeOwned>(what: &str, json: &str) -> Result<T> {
    serde_json::from_str(json).map_err(|e| invalid(what, e))
}

/// The raw `privacy` section: `None` absent, a value as given, or (text that is not JSON)
/// a value `resolve` refuses, so an unusable `settings.json` keeps the last good settings.
fn privacy_value(json: Option<&str>) -> (Option<Value>, bool) {
    match json {
        None => (None, false),
        Some(s) => match serde_json::from_str::<Value>(s) {
            Ok(v) => (Some(v), false),
            Err(_) => (Some(Value::String("unusable".into())), true),
        },
    }
}

fn warning_of(w: Option<String>, unparsable: bool) -> Option<String> {
    let w = w?;
    Some(match unparsable {
        true => w.replacen(
            "privacy is not an object",
            "settings.json does not parse",
            1,
        ),
        false => w,
    })
}

fn source_of(s: &str) -> Result<Source> {
    serde_json::from_value(Value::String(s.to_owned())).map_err(|_| {
        reject(CoreError::new(
            "INVALID_ARGS",
            format!("unknown source {s:?}"),
        ))
    })
}

fn purposes_of(s: &str) -> Result<Purposes> {
    match s {
        "diagnostics" => Ok(Purposes::DIAGNOSTICS),
        "training" => Ok(Purposes::ALL),
        other => Err(reject(CoreError::new(
            "INVALID_ARGS",
            format!("asUpload is diagnostics or training, not {other:?}"),
        ))),
    }
}

/// Runs `f` on its own thread (it may wait on the writer for seconds) and settles the
/// promise with its JSON.
fn off_thread<'e>(
    env: &'e Env,
    f: impl FnOnce() -> Result<String> + Send + 'static,
) -> Result<Object<'e>> {
    let (deferred, promise) = env.create_deferred::<String, _>()?;
    std::thread::Builder::new()
        .name("fin-node-journal-call".to_string())
        .spawn(move || match guarded(f, || {}) {
            Ok(s) => deferred.resolve(move |_env| Ok(s)),
            Err(e) => deferred.reject(e),
        })
        .map_err(|e| {
            reject(CoreError::new(
                "INTERNAL",
                format!("cannot start a journal thread: {e}"),
            ))
        })?;
    Ok(promise)
}

/// Test-only options (`test-hooks` builds): a short stats window and flush interval.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TestOptions {
    stats_every_ms: Option<u64>,
    flush_every_ms: Option<u64>,
}

fn options(server_data: &Path, hostname: String, test: Option<&str>) -> Result<JournalOptions> {
    let mut opts = JournalOptions {
        scrub: server_scrub(server_data, Some(hostname)),
        ..JournalOptions::default()
    };
    if let Some(t) = test {
        let t: TestOptions = parse("test options", t)?;
        if cfg!(feature = "test-hooks") {
            if let Some(ms) = t.stats_every_ms {
                opts.stats_every = Duration::from_millis(ms);
            }
            if let Some(ms) = t.flush_every_ms {
                opts.flush_every = Duration::from_millis(ms);
            }
        }
    }
    Ok(opts)
}

/// The diagnostics journal of one server data (`<server-data>/events`).
#[napi(js_name = "Journal")]
pub struct JournalHandle {
    journal: fin_journal::Journal,
    key: PathBuf,
    warning: Option<String>,
}

#[napi]
impl JournalHandle {
    /// Opens the journal of `server_data` under the consent resolved from `privacy_json`
    /// (`settings.json`'s raw `privacy` section; `null` when absent, text that is not JSON
    /// when the file is unusable) and this process's `DO_NOT_TRACK`/`FIN_DIAGNOSTICS`/
    /// `FIN_TRAINING`. `hostname` (Node's `os.hostname()`) is scrubbed from received text.
    /// Replaces a journal already open on the same server data. Installs the panic hook.
    #[napi(factory, catch_unwind)]
    pub fn open(
        server_data: String,
        privacy_json: Option<String>,
        hostname: String,
        test_options_json: Option<String>,
    ) -> Result<JournalHandle> {
        let dir = PathBuf::from(&server_data);
        let opts = options(&dir, hostname, test_options_json.as_deref())?;
        let (privacy, unparsable) = privacy_value(privacy_json.as_deref());
        let (journal, w) = fin_journal::Journal::open_with_settings(
            &dir.join("events"),
            privacy.as_ref(),
            EnvOverrides::from_env(),
            opts,
        );
        let key = key_of(&dir);
        {
            let mut all = lock(&JOURNALS);
            all.retain(|(p, _)| *p != key);
            all.push((key.clone(), journal.clone()));
        }
        install_panic_hook();
        Ok(JournalHandle {
            journal,
            key,
            warning: warning_of(w, unparsable),
        })
    }

    /// The warning of an unusable `privacy` section at open (recording is then off).
    #[napi(getter, catch_unwind)]
    pub fn open_warning(&self) -> Option<String> {
        self.warning.clone()
    }

    /// This process run's id (a ULID): every entry of this journal carries it.
    #[napi(getter, catch_unwind)]
    pub fn session(&self) -> Option<String> {
        self.journal.session().map(str::to_owned)
    }

    /// Whether any purpose is on (`finisherd.json` `diagnostics.enabled`).
    #[napi(getter, catch_unwind)]
    pub fn enabled(&self) -> bool {
        self.journal.gate().is_some()
    }

    /// Applies a new raw `privacy` section (the settings watch); returns the warning of an
    /// unusable one, which keeps the last good settings.
    #[napi(catch_unwind)]
    pub fn apply_settings(&self, privacy_json: Option<String>) -> Option<String> {
        let (privacy, unparsable) = privacy_value(privacy_json.as_deref());
        warning_of(self.journal.apply_settings(privacy.as_ref()), unparsable)
    }

    /// The `ConsentState` JSON in force.
    #[napi(catch_unwind)]
    pub fn consent(&self) -> Result<String> {
        encode(&self.journal.consent())
    }

    /// Announces this run (`SessionStart` JSON): `session.start` now if on, else when a
    /// purpose turns on; `session.unclean` first when the previous run never ended. Waits at
    /// most `timeout_ms` (≤ 1 s) for the writer: true when they are on disk (or nothing was
    /// due), false on a timeout (the start is still made).
    #[napi(catch_unwind)]
    pub fn start_session(&self, start_json: String, timeout_ms: u32) -> Result<bool> {
        let start: SessionStart = parse("session start", &start_json)?;
        let timeout = Duration::from_millis(u64::from(timeout_ms.min(1_000)));
        Ok(self.journal.start_session(start, timeout))
    }

    /// A clean shutdown: `session.end` and `openSession` cleared, waiting at most 1 s.
    /// Resolves to whether `session.end` is on disk.
    #[napi(catch_unwind, ts_return_type = "Promise<boolean>")]
    pub fn end_session<'e>(&self, env: &'e Env, uptime_s: f64) -> Result<Object<'e>> {
        let j = self.journal.clone();
        let uptime = uptime_s.max(0.0) as u64;
        off_thread(env, move || {
            encode(&j.end_session(uptime, Duration::from_secs(1)))
        })
    }

    /// Queues an entry received from a client or built by the server (`payload_json` in the
    /// kind's wire form when it has one). `ctx_json`: raw ids, keyed here. `session`: the
    /// sender's run (a ULID; this journal's own when absent). Throws `EVENT_KIND_UNKNOWN`,
    /// `EVENT_INVALID`, `EVENT_TOO_LARGE`. Off, it builds nothing.
    #[napi(catch_unwind)]
    pub fn record(
        &self,
        kind: String,
        kv: u32,
        ctx_json: Option<String>,
        payload_json: String,
        source: String,
        session: Option<String>,
    ) -> Result<()> {
        if self.journal.gate().is_none() {
            return Ok(());
        }
        let (source, ctx, payload, session) =
            self.prepare(ctx_json, &payload_json, &source, session)?;
        let kv = u16::try_from(kv).map_err(|_| event_invalid("kv is out of range"))?;
        self.journal
            .record_value(source, &session, &kind, kv, ctx, payload)
            .map_err(reject)
    }

    /// [`JournalHandle::record`], written and flushed before it returns, waiting at most
    /// `timeout_ms` (an uncaught error just before the process exits). True when on disk.
    #[napi(catch_unwind)]
    pub fn record_sync(
        &self,
        kind: String,
        kv: u32,
        ctx_json: Option<String>,
        payload_json: String,
        source: String,
        timeout_ms: u32,
    ) -> Result<bool> {
        if self.journal.gate().is_none() {
            return Ok(false);
        }
        let (source, ctx, payload, session) =
            self.prepare(ctx_json, &payload_json, &source, None)?;
        let kv = u16::try_from(kv).map_err(|_| event_invalid("kv is out of range"))?;
        let timeout = Duration::from_millis(u64::from(timeout_ms.min(5_000)));
        self.journal
            .record_value_sync(source, &session, &kind, kv, ctx, payload, timeout)
            .map_err(reject)
    }

    /// One timing (`metric`: `rpc`, keyed by `RPC_METHODS`, or `eval`, by `EVAL_KEYS`; an
    /// unknown key counts as `other`). `code`: the error code of a failed call.
    #[napi(catch_unwind)]
    pub fn observe(&self, metric: String, key: String, micros: f64, code: Option<String>) {
        if self.journal.gate().is_none() {
            return;
        }
        let (metric, key) = match metric.as_str() {
            "rpc" => (Metric::Rpc, RPC_METHODS.get_or_other(&key)),
            "eval" => (Metric::Eval, EVAL_KEYS.get_or_other(&key)),
            _ => return,
        };
        let code = code
            .as_deref()
            .map(|c| Code::parse(c).unwrap_or_else(|| Code::parse("OTHER").expect("a valid code")));
        self.journal
            .observe(metric, key, micros.max(0.0) as u64, code);
    }

    /// Resolves to the `DiagnosticsReport` JSON (after a flush; reads every file).
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn status<'e>(&self, env: &'e Env) -> Result<Object<'e>> {
        let j = self.journal.clone();
        off_thread(env, move || encode(&j.report()))
    }

    /// The `JournalStatus` JSON as the writer last published it, without waiting.
    #[napi(catch_unwind)]
    pub fn status_now(&self) -> Result<String> {
        encode(&self.journal.status_now())
    }

    /// One page of the export (`Filter` JSON; `cursor`: a page's `next`; `limit` 1–1000;
    /// `as_upload`: `diagnostics` or `training` for the upload view). Resolves to the
    /// `ExportPage` JSON.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn export<'e>(
        &self,
        env: &'e Env,
        filter_json: String,
        cursor: Option<String>,
        limit: u32,
        as_upload: Option<String>,
    ) -> Result<Object<'e>> {
        let filter: Filter = parse("export filter", &filter_json)?;
        let as_upload = as_upload.as_deref().map(purposes_of).transpose()?;
        let j = self.journal.clone();
        off_thread(env, move || {
            let page = j
                .export_page(&filter, as_upload, cursor.as_deref(), limit as usize)
                .map_err(reject)?;
            encode(&page)
        })
    }

    /// Purges (`PurgeSelector` JSON: `"all"`, `{"project": id}`, `{"before": ms}`); resolves
    /// to the `PurgeReport` JSON.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn purge<'e>(&self, env: &'e Env, selector_json: String) -> Result<Object<'e>> {
        let sel: PurgeSelector = parse("purge selector", &selector_json)?;
        let j = self.journal.clone();
        off_thread(env, move || encode(&j.purge(sel).map_err(reject)?))
    }

    /// Sets what this server knows of a project's flag (`default`, `never`, or `null`:
    /// unknown). Engines do this themselves at open, on `collect` events and at close.
    #[napi(catch_unwind)]
    pub fn set_project_policy(&self, project_id: String, policy: Option<String>) -> Result<()> {
        let policy = policy
            .map(|p| parse::<CollectPolicy>("collect policy", &json!(p).to_string()))
            .transpose()?;
        self.journal.set_project_policy(&project_id, policy);
        Ok(())
    }

    /// Every project the journal was told about, with its flag: `{id: default|never|unknown}`.
    #[napi(catch_unwind)]
    pub fn project_policies(&self) -> Result<String> {
        encode(&self.journal.project_policies())
    }

    /// Drains, flushes, fsyncs and stops the writer; engines record nothing afterwards.
    #[napi(catch_unwind, ts_return_type = "Promise<void>")]
    pub fn close<'e>(&self, env: &'e Env) -> Result<Object<'e>> {
        let j = self.journal.clone();
        let key = self.key.clone();
        lock(&JOURNALS).retain(|(p, other)| !(*p == key && other.session() == j.session()));
        let (deferred, promise) = env.create_deferred::<(), _>()?;
        std::thread::Builder::new()
            .name("fin-node-journal-call".to_string())
            .spawn(move || {
                let _ = catch_unwind(AssertUnwindSafe(move || j.close()));
                deferred.resolve(|_env| Ok(()));
            })
            .map_err(|e| {
                reject(CoreError::new(
                    "INTERNAL",
                    format!("cannot start a journal thread: {e}"),
                ))
            })?;
        Ok(promise)
    }
}

impl JournalHandle {
    fn prepare(
        &self,
        ctx_json: Option<String>,
        payload_json: &str,
        source: &str,
        session: Option<String>,
    ) -> Result<(Source, Option<Ctx>, Value, String)> {
        let source = source_of(source)?;
        let ctx = match ctx_json {
            None => None,
            Some(c) => {
                let raw: RawCtx =
                    serde_json::from_str(&c).map_err(|e| event_invalid(format!("ctx: {e}")))?;
                Some(ctx_of(&self.journal, raw)?)
            }
        };
        let payload: Value = serde_json::from_str(payload_json)
            .map_err(|e| event_invalid(format!("payload: {e}")))?;
        let session = session
            .or_else(|| self.journal.session().map(str::to_owned))
            .unwrap_or_default();
        Ok((source, ctx, payload, session))
    }
}

#[cfg(feature = "test-hooks")]
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

    /// Test hook: how often the previous panic hook ran (it is chained after ours).
    #[napi(catch_unwind)]
    pub fn previous_hook_calls_for_test(&self) -> f64 {
        hook::PREVIOUS_CALLS.load(std::sync::atomic::Ordering::SeqCst) as f64
    }
}

/// The `RPC_METHODS` table (the `rpc.stats` keys, `other` last): finisherd checks it against
/// its RPCs.
#[napi(catch_unwind)]
pub fn rpc_methods() -> Vec<String> {
    RPC_METHODS
        .entries()
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Every kind name of fin-core's registry (`diagnostics.record` tells an unknown kind,
/// `EVENT_KIND_UNKNOWN`, from one a source may not send, `EVENT_INVALID`).
#[napi(catch_unwind)]
pub fn kind_names() -> Vec<String> {
    let mut names: Vec<String> = fin_core::events::kinds()
        .iter()
        .map(|k| k.name.to_owned())
        .collect();
    names.dedup();
    names
}

/// Whether `privacy_json` (a `privacy` section, `null` for none) is one the journal accepts:
/// `null` when it is, else why not, naming the first bad key (`privacy.retentionDays: …`).
/// The settings writer checks the merged section with it before it replaces the file.
#[napi(catch_unwind)]
pub fn validate_privacy(privacy_json: String) -> Option<String> {
    let v: Value = match serde_json::from_str(&privacy_json) {
        Ok(v) => v,
        Err(e) => return Some(format!("privacy: not JSON ({e})")),
    };
    let obj = match &v {
        Value::Null => return None,
        Value::Object(o) => o,
        _ => return Some("privacy: not an object".to_owned()),
    };
    for (key, value) in obj {
        let one = Value::Object([(key.clone(), value.clone())].into_iter().collect());
        if let Err(e) = fin_journal::PrivacySettings::parse(Some(&one)) {
            let e = e.strip_prefix("privacy: ").unwrap_or(&e).to_owned();
            return Some(match e.starts_with(&format!("privacy.{key}")) {
                true => e,
                false => format!("privacy.{key}: {e}"),
            });
        }
    }
    None
}
