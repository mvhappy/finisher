//! `fin-sandbox` (sub-project 7C, spec §8): one pure [`SandboxPolicy`], the four profiles that
//! build it from typed inputs ([`profile`]), and the root enumeration the add-only mechanisms
//! need ([`enumerate`]). On Linux, [`linux`] applies a policy with Landlock, and
//! [`self_apply`] is the self-sandbox entry `fin-workers` registers. Seatbelt, AppContainer
//! and the `Spawner` build on these in later tasks; the seccomp filter is deferred (owner
//! decision 2026-11-08, plan 7C Task 4). [`launch`] is the launcher
//! (`fin-sandbox-exec`) for the programs `finisherd` starts itself: agents and hooks.

pub mod data;
pub mod enumerate;
pub mod error;
pub mod inputs;
pub mod launch;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod policy;
pub mod profile;
pub mod self_apply;
pub mod status;

pub use error::{ApplyError, PolicyError};
pub use inputs::{
    AgentInput, FinEndpoint, HookFin, HookInput, PluginData, PluginInput, WorkerInput, WorkerScope,
};
pub use policy::{
    DeviceAccess, DeviceRule, HEADLESS_DEVICES, Limits, Net, Os, Profile, ReadPolicy,
    SandboxPolicy, headless_devices,
};
pub use self_apply::self_apply;
pub use status::{
    DisabledReason, LauncherProbe, Mechanism, Namespaces, READY, SandboxStatus, status,
};

/// The types `@finisher/core-types` generates from this crate, registered by
/// `crates/fin-store/examples/export_schema.rs`. The policy itself is not among them: it
/// crosses into TypeScript only as JSON checked here (plan 7C Decision 1).
pub fn register_schemas(g: &mut schemars::SchemaGenerator) {
    g.subschema_for::<AgentInput>();
    g.subschema_for::<HookInput>();
    g.subschema_for::<WorkerInput>();
    g.subschema_for::<PluginInput>();
    g.subschema_for::<Profile>();
    g.subschema_for::<SandboxStatus>();
    g.subschema_for::<LauncherProbe>();
}
