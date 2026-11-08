//! `fin-sandbox` (sub-project 7C, spec §8): one pure [`SandboxPolicy`], the four profiles that
//! build it from typed inputs ([`profile`]), and the root enumeration the add-only mechanisms
//! need ([`enumerate`]). On Linux, [`linux`] applies a policy with Landlock, and
//! [`self_apply`] is the self-sandbox entry `fin-workers` registers. Seccomp, Seatbelt,
//! AppContainer, the launcher and the `Spawner` build on these in later tasks.

pub mod data;
pub mod enumerate;
pub mod error;
pub mod inputs;
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
pub use policy::{DeviceAccess, DeviceRule, Limits, Net, Os, Profile, ReadPolicy, SandboxPolicy};
pub use self_apply::self_apply;
pub use status::{DisabledReason, Mechanism, READY, SandboxStatus, status};

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
}
