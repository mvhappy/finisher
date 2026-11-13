//! The account label a signed-in fake CLI reports (the `account` sidecar knob).

use serde_json::{Value, json};

use super::lines;
use crate::claude::script::Script;

/// The configured account label, where a signed-in CLI reports one: `initialize.account.email`
/// (a script's own `account` wins, and its `"email": "$remove"` models a start without it).
/// Verified on 2.1.321 (7D Task 0): signed in, `initialize.account` is `{subscriptionType,
/// apiProvider}` with no `tokenSource`, plus `email` once the CLI has the profile in its config
/// dir (a resumed instance, or the profile fetch finishing first); `auth status` reads the label
/// fields from its config dir's profile (`results.rs`), so the knob leaves `auth status` alone.
pub(super) fn with_account(script: &mut Script, label: &str) {
    let mut account = lines::defaults()["initialize"]["account"].clone();
    let own = match script.initialize.get("account") {
        Some(Value::Object(o)) => o.clone(),
        // The script removed or replaced it: left as the script has it.
        Some(_) => return,
        None => Default::default(),
    };
    if let Value::Object(o) = &mut account {
        o.insert("email".into(), json!(label));
    }
    lines::merge(&mut account, &own);
    // An unsigned account names no one.
    if let Value::Object(o) = &mut account
        && o.get("tokenSource").and_then(Value::as_str) == Some("none")
        && !own.contains_key("email")
    {
        o.remove("email");
    }
    script.initialize.insert("account".into(), account);
}
