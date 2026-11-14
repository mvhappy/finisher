//! Platforms and the runtimes of code plugins (spec §3.2).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use semver::VersionReq;
use serde::{Deserialize, Serialize};

/// The shipped OS and architecture pairs (the triples of `deny.toml`), in Node's spelling.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum Platform {
    #[serde(rename = "darwin-arm64")]
    DarwinArm64,
    #[serde(rename = "darwin-x64")]
    DarwinX64,
    #[serde(rename = "linux-x64")]
    LinuxX64,
    #[serde(rename = "linux-arm64")]
    LinuxArm64,
    #[serde(rename = "win32-x64")]
    Win32X64,
    #[serde(rename = "win32-arm64")]
    Win32Arm64,
}

impl Platform {
    pub const ALL: [Platform; 6] = [
        Platform::DarwinArm64,
        Platform::DarwinX64,
        Platform::LinuxX64,
        Platform::LinuxArm64,
        Platform::Win32X64,
        Platform::Win32Arm64,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::DarwinArm64 => "darwin-arm64",
            Platform::DarwinX64 => "darwin-x64",
            Platform::LinuxX64 => "linux-x64",
            Platform::LinuxArm64 => "linux-arm64",
            Platform::Win32X64 => "win32-x64",
            Platform::Win32Arm64 => "win32-arm64",
        }
    }
}

/// How a code plugin runs (spec §3.2). `python` and `wasm` are after v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Runtime {
    /// One executable per platform, by package path; its keys are the manifest's `platforms`.
    Native { exec: BTreeMap<Platform, String> },
    /// One bundled `.mjs` file run by finisherd's own Node, which must satisfy `node`.
    Node {
        main: String,
        #[schemars(with = "String")]
        node: VersionReq,
    },
}
