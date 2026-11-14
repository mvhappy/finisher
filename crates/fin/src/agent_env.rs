//! `FIN_CALL` and `FIN_CALL_IDEM` (sub-project 7 §3.4, §12.3; plan 7A Decision 14).
//!
//! `FIN_CALL` is the call id the agent runtime sets for the processes it starts per tool call;
//! `fin` always sends it in its actor header (`diag.rs`), which is the join. It becomes the
//! **idempotency key** only when the runtime also sets `FIN_CALL_IDEM=1`, its promise that this
//! `fin` is the call's only command: the built-in `fin` tool runs exactly one `fin` per call and
//! sets it. A shell tool call may run several (`fin undo; fin undo`), and keying them all on one
//! call id would dedupe the second undo silently or refuse different arguments as
//! `IDEM_KEY_REUSED`, so a shell never gets `FIN_CALL_IDEM`.
//!
//! With both set, a write without `--idem-key` commits as txId = `FIN_CALL` (an exact join, and
//! a call retried after a crash is a dedupe, not a second edit). One `fin` write is one
//! transaction (a `batch` too; a merge's group is keyed `<txId>-<n>` by the server), so no
//! per-line suffix is needed. Without `FIN_CALL_IDEM=1`, nothing changes.

use clap::ArgMatches;
use fin_core::CoreError;
use serde_json::json;

use crate::commands::Globals;
use crate::output::usage;
use crate::registry;

pub const FIN_CALL: &str = "FIN_CALL";
/// Set to `1` by the runtime for a single-shot `fin` (the built-in tool): `FIN_CALL` is the key.
pub const FIN_CALL_IDEM: &str = "FIN_CALL_IDEM";

/// The longest `FIN_CALL` taken (a ULID is 26 characters).
const MAX_CALL: usize = 128;

/// The registry name of the parsed command (`clip.trim`, `try.keep`, `cherry-pick`), from
/// clap's subcommand chain.
pub fn command_name(m: &ArgMatches) -> Option<String> {
    let mut parts = Vec::new();
    let mut cur = m;
    while let Some((name, sub)) = cur.subcommand() {
        parts.push(name);
        cur = sub;
    }
    (!parts.is_empty()).then(|| parts.join("."))
}

/// The idempotency key a command runs with: the explicit `--idem-key`, else `call` for a
/// write, else none.
pub fn default_idem_key(
    cmd: &registry::Command,
    explicit: Option<&str>,
    call: Option<&str>,
) -> Option<String> {
    match (explicit, call) {
        (Some(k), _) => Some(k.to_string()),
        (None, Some(c)) if cmd.write => Some(c.to_string()),
        _ => None,
    }
}

/// Fills `globals.idem_key` from `FIN_CALL` for a write without `--idem-key`, when
/// `FIN_CALL_IDEM=1` says this run is the call's only command. Then a missing or malformed
/// `FIN_CALL` is `USAGE` for a write (it would become a txId); reads ignore both.
pub fn apply(globals: &mut Globals, command: Option<&str>) -> Result<(), CoreError> {
    if std::env::var_os(FIN_CALL_IDEM).is_none_or(|v| v != "1") {
        return Ok(());
    }
    let call = std::env::var_os(FIN_CALL);
    apply_with(
        globals,
        command,
        call.as_deref().and_then(|c| c.to_str()),
        true,
    )
}

fn apply_with(
    globals: &mut Globals,
    command: Option<&str>,
    call: Option<&str>,
    set: bool,
) -> Result<(), CoreError> {
    let Some(cmd) = command.and_then(registry::find) else {
        return Ok(());
    };
    if !cmd.write || globals.idem_key.is_some() || !set {
        return Ok(());
    }
    let call = call.filter(|c| valid_call(c)).ok_or_else(|| {
        usage(
            "FIN_CALL is not a call id",
            json!({ "field": FIN_CALL }),
            "FIN_CALL_IDEM=1 needs FIN_CALL set to the tool call's id; unset both, or pass --idem-key",
        )
    })?;
    globals.idem_key = default_idem_key(cmd, None, Some(call));
    Ok(())
}

fn valid_call(c: &str) -> bool {
    !c.is_empty()
        && c.len() <= MAX_CALL
        && c.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(name: &str) -> &'static registry::Command {
        registry::find(name).unwrap()
    }

    #[test]
    fn writes_take_the_call_reads_do_not() {
        let call = Some("01JC0000000000000000000006");
        assert_eq!(
            default_idem_key(cmd("clip.trim"), None, call).as_deref(),
            call
        );
        assert_eq!(default_idem_key(cmd("view"), None, call), None);
        assert_eq!(
            default_idem_key(cmd("clip.trim"), Some("mine"), call).as_deref(),
            Some("mine")
        );
        assert_eq!(default_idem_key(cmd("clip.trim"), None, None), None);
    }

    #[test]
    fn apply_fills_only_a_write_without_a_key() {
        let call = "01JC0000000000000000000006";
        let mut g = Globals::default();
        apply_with(&mut g, Some("item.trim"), Some(call), true).unwrap();
        assert_eq!(g.idem_key.as_deref(), Some(call));
        let mut g = Globals {
            idem_key: Some("k".into()),
            ..Globals::default()
        };
        apply_with(&mut g, Some("undo"), Some(call), true).unwrap();
        assert_eq!(g.idem_key.as_deref(), Some("k"));
        let mut g = Globals::default();
        apply_with(&mut g, Some("status"), Some("not a call"), true).unwrap();
        apply_with(&mut g, None, Some(call), true).unwrap();
        apply_with(&mut g, Some("item.trim"), None, false).unwrap();
        assert_eq!(g.idem_key, None);
    }

    #[test]
    fn a_malformed_call_is_refused_for_writes() {
        for bad in [Some(""), Some("has space"), Some("a/b"), None] {
            let mut g = Globals::default();
            let e = apply_with(&mut g, Some("clip.delete"), bad, true).unwrap_err();
            assert_eq!(e.code, "USAGE", "{bad:?}");
        }
        let long = "a".repeat(MAX_CALL + 1);
        let mut g = Globals::default();
        assert!(apply_with(&mut g, Some("batch"), Some(&long), true).is_err());
    }
}
