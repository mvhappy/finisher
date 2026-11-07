//! `error.internal` and `error.panic` (Decision 26).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::approx::approx_struct;
use crate::events::class::{Code, Content, Hashed, Name, Scrubbed, SrcLoc};
use crate::events::names;

/// An `INTERNAL` error, or one of the store's unexpected codes, returned through `fin-node`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErrorInternal {
    // The call that failed (`RPC_METHODS`). Not a doc comment: it would inline `Name` in the
    // generated TypeScript.
    #[serde(deserialize_with = "names::rpc_method")]
    pub call: Name,
    pub code: Code,
    /// The keys of the error's `details` (`DETAIL_KEYS`), never their values.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "names::detail_keys"
    )]
    pub details: Vec<Name>,
}
approx_struct!(ErrorInternal {
    call,
    code,
    details
});

/// A panic or an uncaught error. Under diagnostics, metadata only: the repo-relative
/// location (or none, with `thirdParty`), the keyed hash of the scrubbed message (so the same
/// crash groups without its text) and the thread. The scrubbed message itself is content.
/// No backtrace frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPanic {
    #[serde(rename = "where")]
    pub at: PanicWhere,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "SrcLoc")]
    pub location: Option<SrcLoc>,
    pub third_party: bool,
    pub message_hash: Hashed,
    pub thread: PanicThread,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<Scrubbed>", extend("x-fin-content" = "errorMessage"))]
    pub message: Option<Content<Scrubbed>>,
}
approx_struct!(ErrorPanic {
    at,
    location,
    third_party,
    message_hash,
    thread,
    message
});

/// The process that panicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PanicWhere {
    FinNode,
    Finisherd,
    Fin,
}

/// The thread that panicked (the plan's `thread: Name`, as a closed enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PanicThread {
    Main,
    Worker,
    Tick,
    Writer,
    Other,
}
