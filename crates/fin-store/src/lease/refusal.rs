//! `LEASE_TAKEOVER_REFUSED`, with `details.reason` and a fix per reason (see
//! [`super::Contender::takeover`]).

use fin_core::CoreError;
use serde_json::json;

pub(super) fn refused(
    reason: &str,
    message: &str,
    holder: Option<(&str, u64, Option<i64>)>,
) -> CoreError {
    let fix = match reason {
        "CONFIRM_REQUIRED" => {
            "on a sync folder the lease is advisory: once the other machine has stopped editing, take over with confirmation (fin project takeover --confirm)"
        }
        "HELD" => {
            "another server is editing this project; close it there, or wait: a holder that stops is taken over automatically about 50 s after its last heartbeat"
        }
        "STALE_UNCONFIRMED" => {
            "the holder looks stopped; a second probe at least 5 s after the first confirms it, so retry shortly"
        }
        "CONTENDED" => "another server is acquiring the lease right now; retry shortly",
        _ => "check that the project folder is reachable and writable, then retry",
    };
    let mut details = json!({ "reason": reason });
    if let Some((holder, epoch, age)) = holder {
        details["holder"] = json!(holder);
        details["epoch"] = json!(epoch);
        details["ageServerMs"] = json!(age);
    }
    CoreError::new(
        "LEASE_TAKEOVER_REFUSED",
        format!("lease takeover refused: {message}"),
    )
    .with_details(details)
    .with_fix(fix)
}
