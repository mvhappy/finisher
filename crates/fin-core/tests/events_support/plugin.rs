//! Samples of the plugin kinds (8A Task 11; 8B Task 15), shared by `events.rs` (one sample per kind) and
//! `events_plugin.rs`.

use fin_core::events::names::{
    OP_KINDS, PLUGIN_ACTIONS, PLUGIN_METHOD_FAMILIES, bundled_plugin_name,
};
use fin_core::events::names_jobs::SIGNALS;
use fin_core::events::payloads::*;
use fin_core::events::{AnyPayload, Code, Content, Hashed, IdRef};
use serde_json::json;

/// `plugin.lifecycle`, `registry.refresh`, `plugin.stats`, `plugin.exit` and
/// `plugin.command`, in registry order.
pub fn samples(hashed: Hashed, code: Code) -> Vec<AnyPayload> {
    vec![
        PluginLifecycle {
            action: PLUGIN_ACTIONS.get("install").unwrap(),
            outcome: PluginOutcome::Refused,
            code: Some(code.clone()),
            plugin: Some(PluginRef::Hashed(hashed.clone())),
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
        PluginStats {
            plugin: PluginRef::Hashed(hashed.clone()),
            window_s: 60,
            families: vec![PluginMethodStats {
                family: PLUGIN_METHOD_FAMILIES.get("project").unwrap(),
                calls: 12,
                errors: Counts([(code.clone(), 2)].into_iter().collect()),
                reach_denials: 1,
                p50_micros: 800,
                p95_micros: 4_000,
                bytes_in_log2: log2_bucket(600),
                bytes_out_log2: log2_bucket(2_000_000),
            }],
            timeouts: 1,
            crashes: 0,
            log_dropped: 3,
            stderr_dropped: 0,
            egress: Some(PluginEgressStats {
                requests: 4,
                completed: 3,
                denied: 1,
                not_https: 0,
                private_address: 0,
                secret_missing: 0,
                rate_limited: 0,
                too_large: 0,
                timeouts: 0,
                failed: 0,
                redirects_followed: 1,
                redirects_returned: 0,
                bytes_in_log2: log2_bucket(90_000),
                bytes_out_log2: 0,
            }),
        }
        .into(),
        PluginExit {
            plugin: PluginRef::Hashed(hashed.clone()),
            reason: PluginExitReason::Crash,
            exit_code: None,
            signal: SIGNALS.get("segv"),
            jobs_served: 5,
            uptime_s: 120,
        }
        .into(),
        PluginCommand {
            plugin: PluginRef::Hashed(hashed.clone()),
            command: hashed,
            outcome: PluginCommandOutcome::Rebased,
            code: None,
            ops: Counts(
                [(OP_KINDS.get("item.trim").unwrap(), 2)]
                    .into_iter()
                    .collect(),
            ),
            tx_id: IdRef::classify("01JC0000000000000000000002").ok(),
            dry_run: false,
            micros: 31_000,
            args: Some(Content(Body::Inline(json!({ "gapMs": 40 })))),
            args_omitted: None,
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
