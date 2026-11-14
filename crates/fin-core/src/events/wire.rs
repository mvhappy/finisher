//! Wire forms (plan D, Decisions 6, 7, 24 and 26): what a remote source sends through
//! `diagnostics.record` for the kinds it may send (`fin`: `cli.command`, `error.panic`), and
//! the pure conversions the server applies before an entry exists: raw ids and `fin`'s
//! unkeyed `argsHash` are keyed with the server's key, and a message is scrubbed again with
//! the server's [`ScrubCtx`] and hashed. [`super::kinds::validate_payload`] decodes these for
//! those kinds; a sender builds them (`fin` serialises a [`CliCommandIn`]).

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::class::{Blake3Hex, Code, Content, Hashed, Name, RawId, ScrubCtx, Scrubbed, SrcLoc};
use super::names;
use super::payloads::{CliCommand, ErrorPanic, PanicThread, PanicWhere};

/// A wire form and its conversion to the stored payload.
pub trait WireForm: DeserializeOwned {
    type Payload;

    /// The stored payload: keyed with `key` (the journal's keyed hash, 16 bytes) and
    /// scrubbed with `ctx`.
    fn into_payload(self, key: &dyn Fn(&[u8]) -> [u8; 16], ctx: &ScrubCtx) -> Self::Payload;
}

/// `cli.command` as `fin` sends it (Decision 23): `argsHash` unkeyed (`blake3:<hex>`), the
/// txIds raw (an idempotency key is not a ULID).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliCommandIn {
    #[serde(deserialize_with = "names::cli_command")]
    pub name: Name,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "names::cli_flags"
    )]
    pub flags: Vec<Name>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_hash: Option<Blake3Hex>,
    pub micros: u64,
    pub exit: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Code>,
    pub online: bool,
    pub dry_run: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tx_ids: Vec<RawId>,
}

impl CliCommandIn {
    /// The stored `cli.command`: `argsHash` keyed (the same bytes an intent's `argsHash` keys,
    /// so the two join), each txId an [`super::class::IdRef`] (a ULID kept, else keyed).
    pub fn into_payload(self, key: &dyn Fn(&[u8]) -> [u8; 16], _ctx: &ScrubCtx) -> CliCommand {
        let mut flags = self.flags;
        flags.sort();
        flags.dedup();
        CliCommand {
            name: self.name,
            flags,
            args_hash: self
                .args_hash
                .map(|h| Hashed::keyed(h.as_str().as_bytes(), key)),
            micros: self.micros,
            exit: self.exit,
            code: self.code,
            online: self.online,
            dry_run: self.dry_run,
            tx_ids: self
                .tx_ids
                .into_iter()
                .map(|id| id.into_id_ref(key))
                .collect(),
        }
    }
}

impl WireForm for CliCommandIn {
    type Payload = CliCommand;

    fn into_payload(self, key: &dyn Fn(&[u8]) -> [u8; 16], ctx: &ScrubCtx) -> CliCommand {
        CliCommandIn::into_payload(self, key, ctx)
    }
}

/// `error.panic` as a sender reports it (Decision 26): the message, scrubbed by the sender
/// with what it knows, and no `messageHash` (the server's key makes it). Also how `fin-node`
/// and `finisherd` build their own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPanicIn {
    #[serde(rename = "where")]
    pub at: PanicWhere,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<SrcLoc>,
    pub third_party: bool,
    pub thread: PanicThread,
    pub message: Scrubbed,
}

impl ErrorPanicIn {
    /// The stored `error.panic`: the message scrubbed again with `ctx`, `messageHash` the keyed
    /// hash of that, and the message itself as content (redaction removes it unless training
    /// is on).
    pub fn into_payload(self, key: &dyn Fn(&[u8]) -> [u8; 16], ctx: &ScrubCtx) -> ErrorPanic {
        let message = self.message.rescrub(ctx);
        ErrorPanic {
            at: self.at,
            location: self.location,
            third_party: self.third_party,
            message_hash: Hashed::keyed(message.as_str().as_bytes(), key),
            thread: self.thread,
            message: Some(Content(message)),
        }
    }
}

impl WireForm for ErrorPanicIn {
    type Payload = ErrorPanic;

    fn into_payload(self, key: &dyn Fn(&[u8]) -> [u8; 16], ctx: &ScrubCtx) -> ErrorPanic {
        ErrorPanicIn::into_payload(self, key, ctx)
    }
}
