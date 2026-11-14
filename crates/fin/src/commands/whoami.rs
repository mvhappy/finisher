//! `fin whoami`: the principal this fin proves as (spec §4.8). Without a credential nothing can
//! be proved, so fin does not connect at all and answers `anonymous` (exit 0). A handshake that
//! needed its one retry says why (`retried: "keyStale"` after a server restart).

use fin_core::CoreError;
use serde_json::{Value, json};

use crate::client::{Client, TIMEOUT};
use crate::{auth, endpoint};

fn anonymous() -> Value {
    json!({ "kind": "anonymous", "id": "", "scopes": ["public"], "projects": "*", "branches": "*", "human": false })
}

pub fn run() -> Result<Value, CoreError> {
    if auth::discover(endpoint::server_data().as_deref())?.is_none() {
        // A credential file that cannot be used is a refusal, not `anonymous` (Decision 14b).
        if std::env::var_os(auth::CREDENTIAL_ENV).is_some_and(|v| !v.is_empty()) {
            let at = endpoint::resolve().map(|e| e.display()).unwrap_or_default();
            return Err(auth::credential_unusable(&at));
        }
        return Ok(json!({ "principal": anonymous() }));
    }
    let client = Client::connect(&endpoint::resolve()?, TIMEOUT)?;
    let principal = client.principal().cloned().unwrap_or_else(anonymous);
    let mut out = json!({ "principal": principal });
    if let Some(reason) = client.retried() {
        out["retried"] = json!(reason);
    }
    Ok(out)
}
