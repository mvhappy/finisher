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

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, PanicHookInfo};
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
    args_hash: Option<String>,
    argv_hash: Option<String>,
    project: Option<String>,
    tx_ids: Vec<String>,
    /// The connection of the run, parked by the online backend when it is dropped.
    conn: Option<Client>,
}

static STATE: Mutex<State> = Mutex::new(State {
    name: None,
    flags: BTreeSet::new(),
    dry_run: false,
    args_hash: None,
    argv_hash: None,
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

/// `fin-actor-*` headers: `{kind: agent, session, call}` when `FIN_SESSION` is set, else
/// `{kind: user}`. An empty or over-long id is left out (the server drops it too).
pub fn actor_headers_from(session: Option<&str>, call: Option<&str>) -> Vec<[String; 2]> {
    let Some(session) = session.filter(|s| id_ok(s)) else {
        return vec![header("fin-actor-kind", "user")];
    };
    let mut out = vec![
        header("fin-actor-kind", "agent"),
        header("fin-actor-session", session),
    ];
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

fn flags_of<'a>(cmd: &'a Command, matches: &'a ArgMatches) -> (Vec<&'a str>, BTreeSet<String>) {
    let (mut c, mut m) = (cmd, matches);
    let (mut names, mut flags) = (Vec::new(), BTreeSet::new());
    loop {
        for arg in c.get_arguments() {
            let Some(long) = arg.get_long() else { continue };
            let id = arg.get_id().as_str();
            if m.try_contains_id(id).unwrap_or(false)
                && m.value_source(id) == Some(ValueSource::CommandLine)
            {
                flags.insert(long.to_string());
            }
        }
        let Some((name, sub)) = m.subcommand() else {
            break;
        };
        let Some(sub_cmd) = c.find_subcommand(name) else {
            break;
        };
        names.push(sub_cmd.get_name());
        (c, m) = (sub_cmd, sub);
    }
    (names, flags)
}

/// The dotted command name and the long flags given (by name only). `cmd` is the built command
/// `matches` came from.
fn describe(cmd: &Command, matches: &ArgMatches) -> (String, BTreeSet<String>) {
    let (names, flags) = flags_of(cmd, matches);
    let name = match names.as_slice() {
        [] => "help".to_string(),
        ["schema"]
            if matches
                .subcommand_matches("schema")
                .and_then(|m| m.get_many::<String>("command"))
                .is_some_and(|mut c| c.next().is_some_and(|w| w == "param")) =>
        {
            "schema.param".to_string()
        }
        parts => parts.join("."),
    };
    (name, flags)
}

/// Records which command ran and which flags it was given (by name only).
pub fn note_matches(cmd: &Command, matches: &ArgMatches) {
    let (name, flags) = describe(cmd, matches);
    let mut s = state();
    s.dry_run = flags.contains("dry-run");
    s.name = Some(name);
    s.flags = flags;
}

/// The hash of the arguments as typed, for a run that plans no transaction.
pub fn note_argv(args: &[std::ffi::OsString]) {
    let mut h = blake3::Hasher::new();
    for a in args.iter().skip(1) {
        h.update(a.as_encoded_bytes());
        h.update(&[0]);
    }
    state().argv_hash = Some(format!("blake3:{}", h.finalize().to_hex()));
}

/// The command input a write planned (`commands::args_hash`): the same bytes an intent keys.
pub fn note_inputs(inputs: &[Value]) {
    state().args_hash = Some(crate::commands::args_hash(inputs));
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
/// error's details.
fn answer_tx_ids(result: &Result<Value, CoreError>) -> Vec<String> {
    let id = match result {
        Ok(doc) => doc.get("txId"),
        Err(e) => e.details.get("txId"),
    };
    id.and_then(Value::as_str)
        .map(str::to_string)
        .into_iter()
        .collect()
}

/// The wire form of this run's `cli.command`.
fn command_event(
    s: &State,
    result: &Result<Value, CoreError>,
    exit: i32,
    micros: u64,
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
        args_hash: s
            .args_hash
            .as_ref()
            .or(s.argv_hash.as_ref())
            .and_then(|h| Blake3Hex::parse(h)),
        micros,
        exit: u8::try_from(exit).unwrap_or(u8::MAX),
        code: result.as_ref().err().and_then(|e| Code::parse(&e.code)),
        online: true,
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

/// Sends this run's `cli.command` on the run's connection, once, at the end (after the answer
/// is printed). Never fails, never waits for a reply, never takes longer than [`BUDGET`].
pub fn finish(result: &Result<Value, CoreError>, exit: i32) {
    if !allowed() {
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
    let event = command_event(&s, result, exit, micros);
    let Some(mut conn) = s.conn.take() else {
        return;
    };
    let project = s.project.clone();
    drop(s);
    if let Some(req) = record("cli.command", project.as_deref(), &event) {
        let _ = conn.send_only("diagnostics.record", &req, BUDGET);
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

/// Reports a panic of `fin` as `error.panic` (best effort, within [`BUDGET`]: a connect, then a
/// write; no reply is read). Called by the panic hook before it prints the `INTERNAL` document.
pub fn report_panic(info: &PanicHookInfo<'_>) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if env_opt_out(|k| std::env::var(k).ok()) {
            return;
        }
        let Ok(endpoint) = crate::endpoint::resolve() else {
            return;
        };
        if !allowed() {
            return;
        }
        let Ok(mut client) = Client::connect(&endpoint, BUDGET) else {
            return;
        };
        if let Some(req) = record("error.panic", None, &panic_event(info)) {
            let _ = client.send_only("diagnostics.record", &req, BUDGET);
        }
    }));
}

/// Test hook: `FIN_TEST_PANIC` set makes `fin` panic right after it starts, so a test can see
/// the chained panic hook report and then print the `INTERNAL` document.
pub const TEST_PANIC_ENV: &str = "FIN_TEST_PANIC";

pub fn test_panic() {
    if std::env::var_os(TEST_PANIC_ENV).is_some_and(|v| !v.is_empty()) {
        panic!("{TEST_PANIC_ENV} asked for a panic");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn the_env_opt_out_follows_the_server_s_rules() {
        assert!(!env_opt_out(env(&[])));
        assert!(env_opt_out(env(&[("DO_NOT_TRACK", "1")])));
        assert!(env_opt_out(env(&[("DO_NOT_TRACK", "yes")])));
        for off in ["", "0", "false", " FALSE "] {
            assert!(!env_opt_out(env(&[("DO_NOT_TRACK", off)])), "{off:?}");
        }
        assert!(env_opt_out(env(&[("FIN_DIAGNOSTICS", "off")])));
        assert!(env_opt_out(env(&[("FIN_DIAGNOSTICS", " OFF")])));
        assert!(!env_opt_out(env(&[("FIN_DIAGNOSTICS", "on")])));
    }

    #[test]
    fn the_actor_is_an_agent_only_with_a_session() {
        let get = |h: &[[String; 2]], k: &str| h.iter().find(|p| p[0] == k).map(|p| p[1].clone());
        let user = actor_headers_from(None, Some("call-1"));
        assert_eq!(get(&user, "fin-actor-kind").as_deref(), Some("user"));
        assert_eq!(user.len(), 1);
        let agent = actor_headers_from(Some("s1"), Some("c1"));
        assert_eq!(get(&agent, "fin-actor-kind").as_deref(), Some("agent"));
        assert_eq!(get(&agent, "fin-actor-session").as_deref(), Some("s1"));
        assert_eq!(get(&agent, "fin-actor-call").as_deref(), Some("c1"));
        let no_call = actor_headers_from(Some("s1"), None);
        assert_eq!(no_call.len(), 2);
        // Over-long and empty ids are dropped; a session that is dropped is no agent.
        let long = "x".repeat(MAX_RAW_ID + 1);
        assert_eq!(actor_headers_from(Some("s1"), Some(&long)).len(), 2);
        assert_eq!(actor_headers_from(Some(&long), Some("c")).len(), 1);
        assert_eq!(actor_headers_from(Some(""), None).len(), 1);
    }

    #[test]
    fn the_discovery_file_gates_only_when_it_says_so() {
        note_discovery(&json!({ "diagnostics": { "enabled": false } }));
        assert_eq!(DISCOVERY.load(Ordering::Relaxed), DISABLED);
        note_discovery(&json!({ "diagnostics": { "enabled": true } }));
        assert_eq!(DISCOVERY.load(Ordering::Relaxed), ENABLED);
        // An older server wrote no `diagnostics`: send, and let the server's gate decide.
        note_discovery(&json!({ "socket": "s" }));
        assert_eq!(DISCOVERY.load(Ordering::Relaxed), UNKNOWN);
    }

    #[test]
    fn a_command_event_holds_names_and_hashes_only() {
        let s = State {
            name: Some("item.trim".into()),
            flags: ["project".to_string(), "typed-by-a-user".to_string()].into(),
            args_hash: Some(format!("blake3:{}", "ab".repeat(32))),
            tx_ids: vec!["tx-1".into()],
            ..State::default()
        };
        let err = CoreError::new("CONFLICT", "no").with_details(json!({ "txId": "tx-2" }));
        let e = command_event(&s, &Err(err), 4, 1234);
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["name"], "item.trim");
        assert_eq!(v["flags"], json!(["project", "other"]));
        assert_eq!(v["exit"], 4);
        assert_eq!(v["code"], "CONFLICT");
        assert_eq!(v["txIds"], json!(["tx-1", "tx-2"]));
        let unknown = State {
            name: Some("not.a.command".into()),
            ..State::default()
        };
        let e = command_event(&unknown, &Ok(json!({})), 0, 1);
        assert_eq!(e.name.as_str(), "other");
        assert!(e.args_hash.is_none());
    }

    fn parsed(line: &str) -> (String, Vec<String>) {
        let mut cmd = crate::configure(<crate::Cli as clap::CommandFactory>::command());
        let args = std::iter::once("fin").chain(line.split_whitespace());
        let m = cmd.try_get_matches_from_mut(args).unwrap();
        let (name, flags) = describe(&cmd, &m);
        (name, flags.into_iter().collect())
    }

    #[test]
    fn registry_names_are_in_core_tables() {
        for c in crate::registry::commands() {
            assert!(
                CLI_COMMANDS.get(c.name).is_some(),
                "{} is not in CLI_COMMANDS",
                c.name
            );
        }
        fn walk(c: &Command, out: &mut BTreeSet<String>) {
            out.extend(
                c.get_arguments()
                    .filter_map(|a| a.get_long())
                    .map(str::to_string),
            );
            c.get_subcommands().for_each(|s| walk(s, out));
        }
        let mut cmd = crate::configure(<crate::Cli as clap::CommandFactory>::command());
        cmd.build();
        let mut longs = BTreeSet::new();
        walk(&cmd, &mut longs);
        for long in longs {
            assert!(
                CLI_FLAGS.get(&long).is_some(),
                "--{long} is not in CLI_FLAGS"
            );
        }
        // Every name the parse yields for a sample of the tree is in the table.
        for line in [
            "status",
            "try keep",
            "cherry-pick tx1 --into b",
            "caption set-text c1 Hello",
        ] {
            let (name, _) = parsed(line);
            assert!(CLI_COMMANDS.get(&name).is_some(), "{line}: {name}");
        }
    }

    #[test]
    fn a_parse_yields_a_name_and_flag_names_never_values() {
        let (name, flags) = parsed("clip insert --track t1 --at end --asset a1 --dur 2s --dry-run");
        assert_eq!(name, "clip.insert");
        assert_eq!(flags, ["asset", "at", "dry-run", "dur", "track"]);
        assert_eq!(
            parsed("--project d --idem-key k status"),
            ("status".into(), vec!["idem-key".into(), "project".into()])
        );
        assert_eq!(parsed("try keep").0, "try.keep");
        assert_eq!(parsed("schema param --scope style").0, "schema.param");
        assert_eq!(parsed("schema clip.insert").0, "schema");
        assert_eq!(parsed("").0, "help");
        assert_eq!(parsed("help clip").0, "help");
        let (name, flags) =
            parsed("diagnostics export --since 2026-11-01 --kind cli.command --as-upload training");
        assert_eq!(name, "diagnostics.export");
        assert_eq!(flags, ["as-upload", "kind", "since"]);
    }
}
