//! `plugin.lifecycle` (8A Task 11, Decision 27) from the registry calls: one entry per plan,
//! commit and removal, with the plugin as a [`PluginRef`] (a bundled name or the journal's
//! keyed hash), sizes as log₂ buckets and no registry id, path, publisher or version. Nothing
//! is built while the journal is off.

use std::path::Path;
use std::time::Instant;

use fin_core::events::Code;
use fin_core::events::Source;
use fin_core::events::names::PLUGIN_ACTIONS;
use fin_core::events::payloads::{
    PluginKindName, PluginLifecycle, PluginOutcome, PluginRef, PluginSourceName, RegistryRefresh,
    RiskName, log2_bucket,
};
use fin_plugin_api::{PluginKind, Risk};
use fin_registry::RegistryError;
use fin_registry::plan::{InstallPlan, PlanAction, SourceKind};

/// What an entry says about the plugin, when known.
pub(crate) struct About<'a> {
    /// `None` when the call never learnt which plugin (a commit of a plan that is gone).
    pub(crate) id: Option<&'a str>,
    pub(crate) source: SourceKind,
    pub(crate) kind: Option<PluginKind>,
    pub(crate) risk: Option<Risk>,
    pub(crate) permissions_added: usize,
    pub(crate) bytes: u64,
}

impl<'a> About<'a> {
    pub(crate) fn of_plan(plan: &'a InstallPlan) -> About<'a> {
        About {
            id: Some(&plan.plugin.id),
            source: plan.source.kind,
            kind: Some(plan.plugin.kind),
            risk: Some(plan.risk),
            permissions_added: plan.diff.added.len() + plan.diff.widened.len(),
            bytes: plan.bytes,
        }
    }
}

pub(crate) fn action_name(a: PlanAction) -> &'static str {
    match a {
        PlanAction::Install => "install",
        PlanAction::Update => "update",
        PlanAction::Downgrade => "downgrade",
    }
}

/// Records `action` on `about` with `result`'s outcome.
pub(crate) fn lifecycle(
    server_key: &Path,
    action: &str,
    about: &About,
    result: Result<(), &RegistryError>,
    started: Instant,
) {
    let j = crate::journal::for_key(server_key);
    if j.gate().is_none() {
        return;
    }
    let plugin = match about.id {
        Some(id) => match PluginRef::of(id, |s| j.hash(s)) {
            Some(r) => Some(r),
            None => return,
        },
        None => None,
    };
    let (outcome, code) = match result {
        Ok(()) => (PluginOutcome::Ok, None),
        Err(e) => {
            let failed = matches!(e.code.as_str(), "IO_ERROR" | "INTERNAL");
            let outcome = if failed {
                PluginOutcome::Failed
            } else {
                PluginOutcome::Refused
            };
            (outcome, Code::parse(&e.code))
        }
    };
    let p = PluginLifecycle {
        action: PLUGIN_ACTIONS.get_or_other(action),
        outcome,
        code,
        plugin,
        source: match about.source {
            SourceKind::Bundled => PluginSourceName::Bundled,
            SourceKind::Registry => PluginSourceName::Registry,
            SourceKind::Dev => PluginSourceName::Dev,
        },
        kind: match about.kind {
            Some(PluginKind::Content) => PluginKindName::Content,
            Some(PluginKind::Code) => PluginKindName::Code,
            None => PluginKindName::Unknown,
        },
        permissions_added: u8::try_from(about.permissions_added).unwrap_or(u8::MAX),
        risk: match about.risk {
            Some(Risk::Low) => RiskName::Low,
            Some(Risk::Medium) => RiskName::Medium,
            Some(Risk::High) => RiskName::High,
            None => RiskName::Unknown,
        },
        bytes_log2: log2_bucket(about.bytes),
        micros: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
    };
    j.record(Source::FinNode, None, p.into());
}

/// What a refresh left behind, for [`refresh`].
pub(crate) struct Refreshed {
    pub(crate) roles_updated: u16,
    pub(crate) targets: u64,
    pub(crate) stale_days: Option<u16>,
}

/// Records `registry.refresh` (8A Decision 27): the outcome and `TUF_*` code, how many roles
/// moved, the listed targets as a log₂ bucket and whether a person asked. No registry id,
/// path or publisher.
pub(crate) fn registry_refresh(
    server_key: &Path,
    by_human: bool,
    result: Result<&Refreshed, &str>,
    started: Instant,
) {
    let j = crate::journal::for_key(server_key);
    if j.gate().is_none() {
        return;
    }
    let (outcome, code, seen) = match result {
        Ok(r) => (PluginOutcome::Ok, None, Some(r)),
        Err(code) => {
            let outcome = if matches!(code, "IO_ERROR" | "INTERNAL" | "REGISTRY_UNREACHABLE") {
                PluginOutcome::Failed
            } else {
                PluginOutcome::Refused
            };
            (outcome, Code::parse(code), None)
        }
    };
    let p = RegistryRefresh {
        outcome,
        code,
        roles_updated: seen.map_or(0, |r| r.roles_updated),
        targets_log2: seen.map_or(0, |r| log2_bucket(r.targets)),
        stale_days: seen.and_then(|r| r.stale_days),
        by_human,
        micros: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
    };
    j.record(Source::FinNode, None, p.into());
}
