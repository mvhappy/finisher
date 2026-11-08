//! Samples of the plugin kinds (8A Task 11), shared by `events.rs` (one sample per kind) and
//! `events_plugin.rs`.

use fin_core::events::names::{PLUGIN_ACTIONS, bundled_plugin_name};
use fin_core::events::payloads::*;
use fin_core::events::{AnyPayload, Code, Hashed};

/// `plugin.lifecycle` and `registry.refresh`, in registry order.
pub fn samples(hashed: Hashed, code: Code) -> Vec<AnyPayload> {
    vec![
        PluginLifecycle {
            action: PLUGIN_ACTIONS.get("install").unwrap(),
            outcome: PluginOutcome::Refused,
            code: Some(code.clone()),
            plugin: Some(PluginRef::Hashed(hashed)),
            source: PluginSourceName::Registry,
            kind: PluginKindName::Content,
            permissions_added: 2,
            risk: RiskName::Medium,
            bytes_log2: log2_bucket(40_000),
            micros: 12_000,
        }
        .into(),
        RegistryRefresh {
            outcome: PluginOutcome::Ok,
            code: None,
            roles_updated: 3,
            targets_log2: log2_bucket(9),
            stale_days: Some(2),
            by_human: true,
            micros: 80_000,
        }
        .into(),
    ]
}

/// Inserts [`samples`] after `storage.state` (their place in the registry).
#[allow(dead_code)]
pub fn insert(v: &mut Vec<AnyPayload>, hashed: Hashed, code: Code) {
    let at = 1 + v
        .iter()
        .position(|p| p.kind().name == "storage.state")
        .unwrap();
    v.splice(at..at, samples(hashed, code));
}

/// A bundled plugin's reference.
#[allow(dead_code)]
pub fn bundled() -> PluginRef {
    PluginRef::Bundled(bundled_plugin_name("@finisher/captions-basic").unwrap())
}
