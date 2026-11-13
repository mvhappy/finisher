//! The account label a signed-in fake CLI reports (the `account` sidecar knob).

use serde_json::{Value, json};

use super::lines;
use crate::claude::script::Script;

/// The configured account label, where a signed-in CLI reports one: `auth status` and
/// `initialize.account` (a script's own value wins; a signed-out script gets none).
// TODO(7D Task 0 Step 4): the field that holds the account label when signed in (and whether
// `initialize.account` carries it); modelled by fake-claude as `email` in both.
pub(super) fn with_account(script: &mut Script, label: &str) {
    let auth = script.auth.get_or_insert_with(Default::default);
    if auth.get("loggedIn") != Some(&Value::Bool(false)) {
        auth.entry("email").or_insert_with(|| json!(label));
    }
    let mut account = lines::defaults()["initialize"]["account"].clone();
    match script.initialize.get("account") {
        Some(Value::Object(o)) => lines::merge(&mut account, o),
        // The script removed or replaced it: left as the script has it.
        Some(_) => return,
        None => {}
    }
    if let Value::Object(o) = &mut account
        && o.get("tokenSource").and_then(Value::as_str) != Some("none")
    {
        o.entry("email").or_insert_with(|| json!(label));
    }
    script.initialize.insert("account".into(), account);
}
