//! The agent name tables (sub-project 7 §4.7, §6.2, §12.2): providers, models, roles, tool
//! kinds and OpenRouter's upstreams, re-exported by [`super::names`]. Every table here is
//! append-only (`agent_tables_are_append_only` checks a committed snapshot), so a name
//! recorded once never reads as `other` later. A model id or an upstream the tables do not
//! know is recorded as a keyed hash by the wire forms (`wire_agent.rs`), never as `other` alone.

use super::class::Name;
use super::names::NameTable;

/// Agent providers (`agent.*`): the built-in agent's (`openrouter`, or `local` for a
/// user-run OpenAI-compatible server), the CLI harnesses' (`anthropic`, `openai`), and the
/// upstreams OpenRouter routes a call to.
pub static AGENT_PROVIDERS: NameTable = NameTable::new(
    "AGENT_PROVIDERS",
    &[
        "openrouter",
        "anthropic",
        "openai",
        "google",
        "mistral",
        "local",
        "other",
    ],
);

/// Model ids diagnostics name (`agent.session.model`, `agent.turn.model`): ids the CLI
/// harnesses report and common OpenRouter slugs. A naming table only: it offers no model
/// (OpenRouter takes any id the user types; the CLIs list their own), and it is append-only.
/// An id it lacks is recorded as `other` with a keyed `modelHash` (`wire_agent.rs`). Every id
/// the bundled catalogue names must be here (`catalogue_ids_in_agent_models`).
pub static AGENT_MODELS: NameTable = NameTable::new(
    "AGENT_MODELS",
    &[
        "anthropic/claude-sonnet-5.5",
        "anthropic/claude-opus-5.5",
        "anthropic/claude-haiku-5.5",
        "openai/gpt-6.1-sol",
        "deepseek/deepseek-v4-pro",
        "moonshotai/kimi-k3",
        "z-ai/glm-5.3",
        "openai/gpt-oss-120b",
        "claude-fable-5-1",
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-haiku-5-5",
        "gpt-6.1-sol",
        "gpt-6.1-terra",
        "gpt-6-sol",
        "gpt-6-luna",
        "gpt-6-astra",
        "other",
    ],
);

/// The upstream providers OpenRouter routes a call to (`agent.turn.upstream`): OpenRouter's
/// provider slugs (`GET /api/v1/providers`, 2026-11-07). Append-only: an entry is never removed
/// (the snapshot test), so a recorded name never reads as `other` later. A display name maps
/// through [`agent_upstream`]; a name the table lacks is recorded as a keyed hash instead.
pub static AGENT_UPSTREAMS: NameTable = NameTable::new(
    "AGENT_UPSTREAMS",
    &[
        "ai21",
        "aion-labs",
        "akashml",
        "alibaba",
        "amazon-bedrock",
        "amazon-nova",
        "ambient",
        "antdigital",
        "anthropic",
        "arcee-ai",
        "assemblyai",
        "atlas-cloud",
        "avian",
        "azure",
        "baidu",
        "baseten",
        "black-forest-labs",
        "cerebras",
        "chutes",
        "cirrascale",
        "clarifai",
        "claude-on-aws",
        "cloudflare",
        "cohere",
        "coreweave",
        "cosine",
        "crucible",
        "crusoe",
        "darkbloom",
        "databricks",
        "decart",
        "deepgram",
        "deepinfra",
        "deepseek",
        "dekallm",
        "digitalocean",
        "elevenlabs",
        "featherless",
        "fireworks",
        "fish-audio",
        "friendli",
        "gmicloud",
        "google-ai-studio",
        "google-vertex",
        "groq",
        "heygen",
        "inception",
        "inceptron",
        "inferact-vllm",
        "inference-net",
        "infermatic",
        "inflection",
        "io-net",
        "ionstream",
        "krea",
        "kt",
        "liquid",
        "makora",
        "mancer",
        "mara",
        "meta",
        "minimax",
        "mistral",
        "modal",
        "modelrun",
        "modular",
        "moonshotai",
        "morph",
        "near-ai",
        "nebius",
        "nex-agi",
        "nextbit",
        "novita",
        "nvidia",
        "ollama",
        "open-inference",
        "openai",
        "parallel",
        "parasail",
        "pareto-alpha",
        "particle-ai",
        "pearl-research",
        "perceptron",
        "perplexity",
        "phala",
        "poolside",
        "primalabs",
        "primeintellect",
        "quiver",
        "recraft",
        "recursive",
        "reka",
        "relace",
        "respan",
        "river",
        "runway",
        "sail-research",
        "sakana",
        "sambanova",
        "scaledown",
        "seed",
        "siliconflow",
        "sourceful",
        "stealth",
        "stepfun",
        "streamlake",
        "switchpoint",
        "tencent",
        "tenstorrent",
        "thinkingmachines",
        "together",
        "typesafe",
        "unbiased",
        "upstage",
        "venice",
        "voyageai",
        "wafer",
        "xai",
        "xiaomi",
        "z-ai",
        "other",
    ],
);

/// OpenRouter display names whose letters differ from their slug's (`Google` is Vertex).
const UPSTREAM_ALIASES: &[(&str, &str)] = &[
    ("Claude Platform on AWS", "claude-on-aws"),
    ("Google", "google-vertex"),
    ("Mancer 2", "mancer"),
    ("Sakana AI", "sakana"),
    ("VoyageAI by MongoDB", "voyageai"),
];

/// Lower-case ASCII letters and digits only: `Moonshot AI`, `moonshotai` and `Z.AI`/`z-ai`
/// compare equal.
fn upstream_key(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The `AGENT_UPSTREAMS` name of an upstream as OpenRouter reports it (a display name such as
/// `Moonshot AI` or `DeepInfra`, or a slug); `None` when the table does not know it (the
/// caller records a keyed hash).
pub fn agent_upstream(reported: &str) -> Option<Name> {
    let k = upstream_key(reported);
    if k.is_empty() || k == "other" {
        return None;
    }
    if let Some(e) = AGENT_UPSTREAMS
        .entries()
        .iter()
        .find(|e| upstream_key(e) == k)
    {
        return AGENT_UPSTREAMS.get(e);
    }
    UPSTREAM_ALIASES
        .iter()
        .find(|(display, _)| upstream_key(display) == k)
        .and_then(|(_, slug)| AGENT_UPSTREAMS.get(slug))
}

/// Agent roles (sub-project 7 §6.2): the bundled roles, `other` for a plugin's.
pub static AGENT_ROLES: NameTable = NameTable::new(
    "AGENT_ROLES",
    &[
        "editor",
        "critic",
        "fixer",
        "clip-finder",
        "caption-stylist",
        "colourist",
        "mixer",
        "other",
    ],
);

/// Agent tool kinds (`agent.toolCall.tool`, §12.2).
pub static AGENT_TOOLS: NameTable = NameTable::new(
    "AGENT_TOOLS",
    &[
        "fin",
        "shell",
        "read",
        "write",
        "edit",
        "search",
        "webFetch",
        "webSearch",
        "mcp",
        "workspace",
        "other",
    ],
);
