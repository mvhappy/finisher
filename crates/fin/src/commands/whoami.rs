//! `fin whoami`: the principal this fin proves as (spec §4.8). Without a credential nothing is
//! proved and the answer is `anonymous` (exit 0): such a connection may only say hello. A
//! handshake that needed its one retry says why (`retried: "keyStale"` after a server restart).

use fin_core::CoreError;
use serde_json::{Value, json};

use crate::client::{Client, TIMEOUT};
use crate::endpoint;

pub fn run() -> Result<Value, CoreError> {
    let client = Client::connect(&endpoint::resolve()?, TIMEOUT)?;
    let principal = client.principal().cloned().unwrap_or_else(|| {
        json!({ "kind": "anonymous", "id": "", "scopes": ["public"], "projects": "*", "branches": "*", "human": false })
    });
    let mut out = json!({ "principal": principal });
    if let Some(reason) = client.retried() {
        out["retried"] = json!(reason);
    }
    Ok(out)
}
