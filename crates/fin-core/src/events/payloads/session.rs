//! `session.start`, `session.end`, `session.unclean` (Decision 25).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::approx::approx_struct;
use crate::events::class::{IdRef, Version};

/// The process run began (the journal opened with a purpose on, or one turned on). Platform
/// facts only (Decision 5): no host, user, IP, serial or hardware model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionStart {
    pub app: Version,
    /// `@finisher/contracts` `PROTOCOL_VERSION`.
    pub protocol: u32,
    pub os: OsFamily,
    pub os_major: u32,
    pub arch: Arch,
    /// Logical cores.
    pub cores: u32,
    /// Memory rounded to a power-of-two GiB.
    pub mem_gi_b: u32,
    pub profile: BuildProfile,
}
approx_struct!(SessionStart {
    app,
    protocol,
    os,
    os_major,
    arch,
    cores,
    mem_gi_b,
    profile
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum OsFamily {
    Linux,
    Macos,
    Windows,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Arch {
    X86_64,
    Aarch64,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum BuildProfile {
    Debug,
    Release,
}

/// The run ended cleanly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionEnd {
    pub uptime_s: u64,
}
approx_struct!(SessionEnd { uptime_s });

/// A start found the previous run still open in `state.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionUnclean {
    pub previous: IdRef,
}
approx_struct!(SessionUnclean { previous });
