//! What `fin` tells `finisherd` about its own run (plan D, Decisions 14b, 23, 23a and 26; spec
//! §21.1): the actor headers on every request, one `cli.command` event at the end of a run that
//! talked to the server, and `error.panic` from the panic hook.
//!
//! The event is a fire-and-forget `diagnostics.record` on the connection the run already has
//! (the online backend parks it here when it is dropped): `fin` writes the frame within
//! [`BUDGET`] and never reads the reply, so a slow or broken server can neither delay the exit
//! nor change the exit code or stdout. A run that never reached the server records nothing.
//!
//! Only names reach the wire: the command and flag names come from the registry's parse and are
//! looked up in `fin-core`'s `CLI_COMMANDS`/`CLI_FLAGS` tables (anything else reads as `other`).
//! Values, positional arguments and stdin are never sent; `argsHash` is an unkeyed BLAKE3 the
//! server keys before it stores it.
//!
//! Who decides: `fin` sends nothing when its own environment says `DO_NOT_TRACK` or
//! `FIN_DIAGNOSTICS=off`, nor when the `finisherd.json` it found says recording is off. With
//! `FIN_SERVER` set that file is never read, so `fin` sends and the server's gate decides.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::PanicHookInfo;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use clap::parser::ValueSource;
use clap::{ArgMatches, Command};
use fin_core::CoreError;
use fin_core::events::names::{CLI_COMMANDS, CLI_FLAGS};
use fin_core::events::{
    Blake3Hex, CliCommandIn, Code, ErrorPanicIn, MAX_RAW_ID, RawId, ScrubCtx, Scrubbed, SrcLoc,
    payloads::{PanicThread, PanicWhere},
};
use serde_json::{Value, json};

use crate::client::Client;

/// The write of an event (and the connect of a panic report) takes at most this long.
pub const BUDGET: Duration = Duration::from_millis(50);

/// What the discovery file said about recording: unknown (not read, or an older server), on, off.
const UNKNOWN: u8 = 0;
const ENABLED: u8 = 1;
const DISABLED: u8 = 2;
static DISCOVERY: AtomicU8 = AtomicU8::new(UNKNOWN);

static STARTED: OnceLock<Instant> = OnceLock::new();

#[derive(Default)]
struct State {
    name: Option<String>,
    flags: BTreeSet<String>,
    dry_run: bool,
    /// The command input of each write step planned (`commands::input`), in order.
    inputs: Vec<Value>,
    /// The canonical hash of the parse (command, flags with their values, positionals).
    parse_hash: Option<String>,
    project: Option<String>,
    tx_ids: Vec<String>,
    /// The connection of the run, parked by the online backend when it is dropped.
    conn: Option<Client>,
}

static STATE: Mutex<State> = Mutex::new(State {
    name: None,
    flags: BTreeSet::new(),
    dry_run: false,
    inputs: Vec::new(),
    parse_hash: None,
    project: None,
    tx_ids: Vec::new(),
    conn: None,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Starts the clock of the run. Called first thing in `main`.
pub fn begin() {
    STARTED.get_or_init(Instant::now);
}

// ---- the actor (Decision 23a) -------------------------------------------------------------

fn header(name: &str, value: &str) -> [String; 2] {
    [name.to_string(), value.to_string()]
}

fn id_ok(v: &str) -> bool {
    !v.is_empty() && v.len() <= MAX_RAW_ID
}

/// `fin-actor-*` headers: `{kind: agent, session, call}` whenever `FIN_SESSION` is set (even
/// empty or too long: the run is still an agent's), else `{kind: user}`. Only a bad id (empty or
/// over-long) is left out; the server drops those too.
pub fn actor_headers_from(session: Option<&str>, call: Option<&str>) -> Vec<[String; 2]> {
    let Some(session) = session else {
        return vec![header("fin-actor-kind", "user")];
    };
    let mut out = vec![header("fin-actor-kind", "agent")];
    if id_ok(session) {
        out.push(header("fin-actor-session", session));
    }
    if let Some(call) = call.filter(|c| id_ok(c)) {
        out.push(header("fin-actor-call", call));
    }
    out
}

/// The actor of this run, from `FIN_SESSION` and `FIN_CALL`.
pub fn actor_headers() -> Vec<[String; 2]> {
    let env = |k: &str| std::env::var(k).ok();
    actor_headers_from(env("FIN_SESSION").as_deref(), env("FIN_CALL").as_deref())
}

// ---- the gate -------------------------------------------------------------------------------

/// `DO_NOT_TRACK` set to anything but empty, `0` or `false`, or `FIN_DIAGNOSTICS=off`.
pub fn env_opt_out(var: impl Fn(&str) -> Option<String>) -> bool {
    let dnt = var("DO_NOT_TRACK").is_some_and(|v| {
        let v = v.trim().to_ascii_lowercase();
        !(v.is_empty() || v == "0" || v == "false")
    });
    dnt || var("FIN_DIAGNOSTICS").is_some_and(|v| v.trim().eq_ignore_ascii_case("off"))
}

/// Whether this run may tell the server anything.
fn allowed() -> bool {
    !env_opt_out(|k| std::env::var(k).ok()) && DISCOVERY.load(Ordering::Relaxed) != DISABLED
}

/// The discovery file's `diagnostics.enabled` (`endpoint.rs` calls this for a file it read).
pub fn note_discovery(info: &Value) {
    let v = match info
        .pointer("/diagnostics/enabled")
        .and_then(Value::as_bool)
    {
        Some(true) => ENABLED,
        Some(false) => DISABLED,
        None => UNKNOWN,
    };
    DISCOVERY.store(v, Ordering::Relaxed);
}

// ---- what the run did -----------------------------------------------------------------------

/// What a parse holds, by name: the subcommand chain, each long flag given with its values, and
/// the positionals in declaration order.
#[derive(Default)]
struct Parsed {
    names: Vec<String>,
    flags: BTreeMap<String, Vec<String>>,
    positionals: Vec<String>,
}

fn parsed_of(cmd: &Command, matches: &ArgMatches) -> Parsed {
    let (mut c, mut m) = (cmd, matches);
    let mut out = Parsed::default();
    loop {
        for arg in c.get_arguments() {
            let id = arg.get_id().as_str();
            if !m.try_contains_id(id).unwrap_or(false)
                || m.value_source(id) != Some(ValueSource::CommandLine)
            {
                continue;
            }
            let values: Vec<String> = m
                .try_get_raw(id)
                .ok()
                .flatten()
                .map(|v| v.map(|s| s.to_string_lossy().into_owned()).collect())
                .unwrap_or_default();
            match arg.get_long() {
                Some(long) => {
                    out.flags.insert(long.to_string(), values);
                }
                None => out.positionals.extend(values),
            }
        }
        let Some((name, sub)) = m.subcommand() else {
            break;
        };
        let Some(sub_cmd) = c.find_subcommand(name) else {
            break;
        };
        out.names.push(sub_cmd.get_name().to_string());
        (c, m) = (sub_cmd, sub);
    }
    out
}

/// The dotted command name, the long flags given and the canonical hash of the parse: the
/// command name, the flags sorted by name with their values, then the positionals. Values are
/// hashed, never sent (the server keys the hash). `cmd` is the built command `matches` came from.
fn describe(cmd: &Command, matches: &ArgMatches) -> (String, BTreeSet<String>, String) {
    let p = parsed_of(cmd, matches);
    let name = match p.names.as_slice() {
        [] => "help".to_string(),
        [n] if n == "schema" && p.positionals.first().is_some_and(|w| w == "param") => {
            "schema.param".to_string()
        }
        parts => parts.join("."),
    };
    let doc = json!({ "cmd": name, "flags": p.flags, "args": p.positionals });
    let bytes = serde_json::to_vec(&doc).unwrap_or_default();
    let hash = format!("blake3:{}", blake3::hash(&bytes).to_hex());
    (name, p.flags.keys().cloned().collect(), hash)
}

/// Records which command ran, which flags it was given (by name only) and the hash of its parse.
pub fn note_matches(cmd: &Command, matches: &ArgMatches) {
    let (name, flags, hash) = describe(cmd, matches);
    let mut s = state();
    s.dry_run = flags.contains("dry-run");
    s.name = Some(name);
    s.flags = flags;
    s.parse_hash = Some(hash);
}

/// One write step's command input (`commands::input`, with `--timeline`). A run that plans a
/// write hashes these as `Tx.argsHash` does ([`crate::commands::args_hash`]), so its
/// `cli.command` and the commit's intent carry the same hash.
pub fn note_input(input: &Value) {
    state().inputs.push(input.clone());
}

pub fn note_project(id: &str) {
    state().project = Some(id.to_string());
}

/// A transaction this run sent (a retry's idempotency key included).
pub fn note_tx(tx_id: &str) {
    let mut s = state();
    if !s.tx_ids.iter().any(|t| t == tx_id) {
        s.tx_ids.push(tx_id.to_string());
    }
}

/// Keeps the run's connection for [`finish`].
pub fn park(client: Client) {
    state().conn = Some(client);
}

// ---- `cli.command` ----------------------------------------------------------------------------

/// The transaction ids a run's answer names: a `txId` at the top of a success document, or in an
/// error's details. A merge (`merge`, `try keep`) answers `merge.mergeId`: its records are
/// `<mergeId>-1`, `-2`, …, and its `log.commit` names the first (plan D5), so this does too.
fn answer_tx_ids(result: &Result<Value, CoreError>) -> Vec<String> {
    let id = match result {
        Ok(doc) => doc.get("txId"),
        Err(e) => e.details.get("txId"),
    };
    let merge = match result {
        Ok(doc) => doc.pointer("/merge/mergeId").and_then(Value::as_str),
        Err(_) => None,
    };
    id.and_then(Value::as_str)
        .map(str::to_string)
        .into_iter()
        .chain(merge.map(|m| format!("{m}-1")))
        .collect()
}

/// `argsHash`: the write's input hash when the run planned a write (what its commit's intent
/// keys), else the canonical hash of the parse. The two never mix for one command: a command
/// either writes or it does not, and only a write has an intent to join.
fn args_hash(s: &State) -> String {
    if s.inputs.is_empty() {
        s.parse_hash.clone().unwrap_or_default()
    } else {
        crate::commands::args_hash(&s.inputs)
    }
}

/// The wire form of this run's `cli.command`.
fn command_event(
    s: &State,
    result: &Result<Value, CoreError>,
    exit: i32,
    micros: u64,
    online: bool,
) -> CliCommandIn {
    let mut tx_ids = s.tx_ids.clone();
    for id in answer_tx_ids(result) {
        if !tx_ids.contains(&id) {
            tx_ids.push(id);
        }
    }
    CliCommandIn {
        name: CLI_COMMANDS.get_or_other(s.name.as_deref().unwrap_or("other")),
        flags: s.flags.iter().map(|f| CLI_FLAGS.get_or_other(f)).collect(),
        args_hash: Blake3Hex::parse(&args_hash(s)),
        micros,
        exit: u8::try_from(exit).unwrap_or(u8::MAX),
        code: result.as_ref().err().and_then(|e| Code::parse(&e.code)),
        online,
        dry_run: s.dry_run,
        tx_ids: tx_ids.iter().filter_map(|id| RawId::new(id)).collect(),
    }
}

/// A `diagnostics.record` payload for `kind` v1.
fn record(kind: &str, project: Option<&str>, payload: &impl serde::Serialize) -> Option<Value> {
    let mut req = json!({ "kind": kind, "kv": 1, "payload": serde_json::to_value(payload).ok()? });
    if let Some(p) = project {
        req["ctx"] = json!({ "project": p });
    }
    Some(req)
}

/// A connection to the discoverable server, when recording is allowed, within `deadline`.
fn connect_by(deadline: Instant) -> Option<Client> {
    let endpoint = crate::endpoint::resolve().ok()?;
    if !allowed() {
        return None;
    }
    let left = deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())?;
    // Outside fin's overall deadline: a timed-out command is still recorded, within BUDGET.
    Client::connect_unbounded(&endpoint, left).ok()
}

/// Writes `req` as a `diagnostics.record` within what is left of `deadline`.
fn send_by(client: &mut Client, req: &Value, deadline: Instant) {
    if let Some(left) = deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
    {
        let _ = client.send_only("diagnostics.record", req, left);
    }
}

/// Sends this run's `cli.command` once, at the end (after the answer is printed): on the run's
/// own connection when it had one (`online`: this command itself talked to the server), else
/// on a new one to the server `finisherd.json` names. Connect and write share [`BUDGET`]; a
/// server that cannot be reached in time means nothing is recorded. Never fails, never waits
/// for a reply.
pub fn finish(result: &Result<Value, CoreError>, exit: i32) {
    let deadline = Instant::now() + BUDGET;
    if !allowed() {
        return;
    }
    // A server that did not prove itself gets nothing more from this run (5A Decision 10).
    if result.as_ref().is_err_and(|e| e.code == "SERVER_UNTRUSTED") {
        return;
    }
    let mut s = state();
    if s.name.as_deref() == Some("diagnostics.purge") {
        // A purge is not recorded: `purge --all` leaves nothing behind, not even its own entry.
        return;
    }
    let micros = STARTED.get().map_or(0, |t| {
        u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX)
    });
    let parked = s.conn.take();
    let event = command_event(&s, result, exit, micros, parked.is_some());
    let project = s.project.clone();
    drop(s);
    let Some(mut conn) = parked.or_else(|| connect_by(deadline)) else {
        return;
    };
    if let Some(req) = record("cli.command", project.as_deref(), &event) {
        send_by(&mut conn, &req, deadline);
    }
}

// ---- `error.panic` ----------------------------------------------------------------------------

fn scrub_ctx() -> ScrubCtx {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    ScrubCtx {
        home: crate::paths::home().map(|h| h.display().to_string()),
        server_data: None,
        tmp: Some(std::env::temp_dir().display().to_string()),
        user: env("USER").or_else(|| env("USERNAME")),
        host: env("HOSTNAME").or_else(|| env("COMPUTERNAME")),
    }
}

/// The panic's message as text.
pub fn panic_message(info: &PanicHookInfo<'_>) -> String {
    match info.payload().downcast_ref::<&str>() {
        Some(s) => (*s).to_string(),
        None => info
            .payload()
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_else(|| "panic".to_string()),
    }
}

fn panic_event(info: &PanicHookInfo<'_>) -> ErrorPanicIn {
    let location = info
        .location()
        .and_then(|l| SrcLoc::from_file_line(l.file(), l.line()));
    let on_main = std::thread::current().name() == Some("main");
    ErrorPanicIn {
        at: PanicWhere::Fin,
        third_party: location.is_none(),
        location,
        thread: if on_main {
            PanicThread::Main
        } else {
            PanicThread::Other
        },
        message: Scrubbed::new(&panic_message(info), &scrub_ctx()),
    }
}

/// Reports a panic of `fin` as `error.panic` (best effort: one connect and one write sharing
/// [`BUDGET`]; no reply is read). Called by the panic hook before it prints the `INTERNAL`
/// document. A panic inside a panic hook aborts the process and nothing can catch it, so this
/// is written not to panic: no unwrap, no indexing, and the one thread it needs (the
/// connection's) is spawned with `std::thread::Builder`, whose failure is an error.
pub fn report_panic(info: &PanicHookInfo<'_>) {
    let deadline = Instant::now() + BUDGET;
    if env_opt_out(|k| std::env::var(k).ok()) {
        return;
    }
    let Some(mut client) = connect_by(deadline) else {
        return;
    };
    if let Some(req) = record("error.panic", None, &panic_event(info)) {
        send_by(&mut client, &req, deadline);
    }
}

/// Test hook: `FIN_TEST_PANIC` set makes `fin` panic right after it starts, so a test can see
/// the chained panic hook report and then print the `INTERNAL` document.
pub const TEST_PANIC_ENV: &str = "FIN_TEST_PANIC";

pub fn test_panic() {
    if crate::backend::test_hook(TEST_PANIC_ENV).is_some() {
        panic!("{TEST_PANIC_ENV} asked for a panic");
    }
}

#[cfg(test)]
mod tests;
