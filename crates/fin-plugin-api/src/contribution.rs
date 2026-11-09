//! Contribution kinds (spec §4.2) and which of them this host consumes, per host-API minor
//! (8A Decision 4). A kind is accepted only once a batch consumes it; newly consumable kinds
//! bump the host API's minor (spec §3.4).

use schemars::JsonSchema;
use semver::Version;
use serde::{Deserialize, Serialize};

/// The host API this build implements: 1.1 consumes `commands` (8B Decision 14).
pub const HOST_API: (u64, u64, u64) = (1, 1, 0);

pub fn host_api() -> Version {
    Version::new(HOST_API.0, HOST_API.1, HOST_API.2)
}

/// Every key of spec §4.2, consumed or not.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum ContributionKind {
    FontCatalogs,
    CaptionStyles,
    Commands,
    Hooks,
    Skills,
    Prompts,
    Roles,
    Rubrics,
    Analyzers,
    Providers,
    CodecBackends,
    MatteProducers,
    AudioProcessors,
    Luts,
    Effects,
    Transitions,
    LottieTemplates,
    FormatTemplates,
    ExportPresets,
    QaProfiles,
    DuckingRules,
    EditPresets,
    Ui,
    AgentProviders,
    Slots,
    Importers,
    Embedders,
}

impl ContributionKind {
    pub const ALL: [ContributionKind; 27] = [
        ContributionKind::FontCatalogs,
        ContributionKind::CaptionStyles,
        ContributionKind::Commands,
        ContributionKind::Hooks,
        ContributionKind::Skills,
        ContributionKind::Prompts,
        ContributionKind::Roles,
        ContributionKind::Rubrics,
        ContributionKind::Analyzers,
        ContributionKind::Providers,
        ContributionKind::CodecBackends,
        ContributionKind::MatteProducers,
        ContributionKind::AudioProcessors,
        ContributionKind::Luts,
        ContributionKind::Effects,
        ContributionKind::Transitions,
        ContributionKind::LottieTemplates,
        ContributionKind::FormatTemplates,
        ContributionKind::ExportPresets,
        ContributionKind::QaProfiles,
        ContributionKind::DuckingRules,
        ContributionKind::EditPresets,
        ContributionKind::Ui,
        ContributionKind::AgentProviders,
        ContributionKind::Slots,
        ContributionKind::Importers,
        ContributionKind::Embedders,
    ];

    /// The key in `contributes`.
    pub fn key(self) -> &'static str {
        match self {
            ContributionKind::FontCatalogs => "fontCatalogs",
            ContributionKind::CaptionStyles => "captionStyles",
            ContributionKind::Commands => "commands",
            ContributionKind::Hooks => "hooks",
            ContributionKind::Skills => "skills",
            ContributionKind::Prompts => "prompts",
            ContributionKind::Roles => "roles",
            ContributionKind::Rubrics => "rubrics",
            ContributionKind::Analyzers => "analyzers",
            ContributionKind::Providers => "providers",
            ContributionKind::CodecBackends => "codecBackends",
            ContributionKind::MatteProducers => "matteProducers",
            ContributionKind::AudioProcessors => "audioProcessors",
            ContributionKind::Luts => "luts",
            ContributionKind::Effects => "effects",
            ContributionKind::Transitions => "transitions",
            ContributionKind::LottieTemplates => "lottieTemplates",
            ContributionKind::FormatTemplates => "formatTemplates",
            ContributionKind::ExportPresets => "exportPresets",
            ContributionKind::QaProfiles => "qaProfiles",
            ContributionKind::DuckingRules => "duckingRules",
            ContributionKind::EditPresets => "editPresets",
            ContributionKind::Ui => "ui",
            ContributionKind::AgentProviders => "agentProviders",
            ContributionKind::Slots => "slots",
            ContributionKind::Importers => "importers",
            ContributionKind::Embedders => "embedders",
        }
    }

    pub fn from_key(key: &str) -> Option<ContributionKind> {
        ContributionKind::ALL.into_iter().find(|k| k.key() == key)
    }

    /// Content that steers agents: medium risk, never auto-updated, and any change to it is
    /// a widening (spec §4.2, §4.3, §5.3).
    pub fn agent_facing(self) -> bool {
        matches!(
            self,
            ContributionKind::Skills
                | ContributionKind::Prompts
                | ContributionKind::Roles
                | ContributionKind::Rubrics
        )
    }
}

/// The kinds a host implementing `host_api` consumes. Host API 1.0: font catalogs and
/// caption styles (8A); 1.1 adds commands (8B). A newly consumed kind is listed from the
/// minor that adds it. `hooks` stays unconsumable until 7F's hook engine (8B Decision 20).
pub fn consumable(host_api: &Version) -> &'static [ContributionKind] {
    const V1_0: &[ContributionKind] = &[
        ContributionKind::FontCatalogs,
        ContributionKind::CaptionStyles,
    ];
    const V1_1: &[ContributionKind] = &[
        ContributionKind::FontCatalogs,
        ContributionKind::CaptionStyles,
        ContributionKind::Commands,
    ];
    match (host_api.major, host_api.minor) {
        (1, 0) => V1_0,
        (1, _) => V1_1,
        _ => &[],
    }
}
