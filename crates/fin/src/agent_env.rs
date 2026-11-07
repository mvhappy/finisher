//! `FIN_CALL` (sub-project 7 §3.4, §12.3; plan 7A Decision 14): the call id the agent runtime
//! sets for each `fin` it runs per tool call. When it is set and a write has no `--idem-key`,
//! `FIN_CALL` is the key, so the write's txId is the tool call's id (an exact join) and a
//! call retried after a crash is a dedupe, not a second edit. One `fin` write is one
//! transaction (a `batch` too, and a merge's group is keyed `<txId>-<n>` by the server), so no
//! per-line suffix is needed. A `fin` without `FIN_CALL` is unchanged.

use clap::ArgMatches;
use fin_core::CoreError;
use serde_json::json;

use crate::commands::Globals;
use crate::output::usage;
use crate::registry;

pub const FIN_CALL: &str = "FIN_CALL";

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

/// Fills `globals.idem_key` from `FIN_CALL` for a write without `--idem-key`. A set but
/// malformed `FIN_CALL` is `USAGE` for a write (it would become a txId) and ignored otherwise.
pub fn apply(globals: &mut Globals, command: Option<&str>) -> Result<(), CoreError> {
    let call = std::env::var_os(FIN_CALL);
    apply_with(
        globals,
        command,
        call.as_deref().and_then(|c| c.to_str()),
        call.is_some(),
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
            "FIN_CALL is set by the agent runtime to the tool call's id; unset it, or pass --idem-key",
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
