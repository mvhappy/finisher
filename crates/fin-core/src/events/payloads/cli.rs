//! `cli.command` (Decision 23): one per `fin` invocation, sent by `fin` at the end. Names and
//! flag names only, never values, positional arguments or stdin.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::approx::approx_struct;
use crate::events::class::{Code, Hashed, IdRef, Name};
use crate::events::names;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CliCommand {
    // The registry name (`CLI_COMMANDS`). Not doc comments on `$ref` fields: they would
    // inline the type in the generated TypeScript.
    #[serde(deserialize_with = "names::cli_command")]
    pub name: Name,
    /// The flag names parsed, sorted (`CLI_FLAGS`).
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "names::cli_flags"
    )]
    pub flags: Vec<Name>,
    // The keyed hash of `fin`'s unkeyed `argsHash` (the server keys it, Decision 6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Hashed")]
    pub args_hash: Option<Hashed>,
    pub micros: u64,
    pub exit: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Code")]
    pub code: Option<Code>,
    pub online: bool,
    pub dry_run: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tx_ids: Vec<IdRef>,
}
approx_struct!(CliCommand {
    name,
    flags,
    args_hash,
    micros,
    exit,
    code,
    online,
    dry_run,
    tx_ids
});
