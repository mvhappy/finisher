//! The static name tables (plan D, Decision 4): the only values a [`Name`] can hold besides
//! literals in code. Each table is a `&'static [&'static str]`; a value not in it reads as
//! `"other"` (except a media extension, which reads as absent). Drift tests keep them honest:
//! `fin` checks its registry's command and flag names against `CLI_COMMANDS`/`CLI_FLAGS`,
//! `finisherd` checks `RPC_METHODS` against its RPCs, and `OP_KINDS` is `Op::NAMES` itself.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

use super::class::Name;
use super::payloads::Counts;

/// A closed list of names.
#[derive(Debug)]
pub struct NameTable {
    /// The table's own name (`"CLI_COMMANDS"`), for tests and messages.
    pub table: &'static str,
    entries: &'static [&'static str],
}

impl NameTable {
    /// A table; every entry must match [`super::class::NAME_PATTERN`], checked when the
    /// `static` is evaluated (a compile error).
    pub(crate) const fn new(table: &'static str, entries: &'static [&'static str]) -> NameTable {
        let mut i = 0;
        while i < entries.len() {
            assert!(
                super::class::is_valid_name(entries[i]),
                "a NameTable entry must match NAME_PATTERN"
            );
            i += 1;
        }
        NameTable { table, entries }
    }

    /// The table's [`Name`] for `s`, if it holds it.
    pub fn get(&self, s: &str) -> Option<Name> {
        self.entries
            .iter()
            .find(|e| **e == s)
            .map(|e| Name::from_static(e))
    }

    /// [`NameTable::get`], or [`Name::OTHER`].
    pub fn get_or_other(&self, s: &str) -> Name {
        self.get(s).unwrap_or(Name::OTHER)
    }

    pub fn entries(&self) -> &'static [&'static str] {
        self.entries
    }
}

/// `fin` commands by registry name (dotted, as `fin`'s registry names them), plus the
/// `fin diagnostics` commands of Task D6 and the `fin plugin|registry` commands of 8A Task 12c
/// (`plugin.commands`: 8B Task 10; `fin @<id> <command>` itself is no registry command: `other`).
pub static CLI_COMMANDS: NameTable = NameTable::new(
    "CLI_COMMANDS",
    &[
        "agent.ls",
        "agent.rollback",
        "agent.send",
        "agent.spawn",
        "agent.stop",
        "agent.wait",
        "approval.ls",
        "approval.wait",
        "asset.add",
        "asset.move",
        "asset.remove",
        "batch",
        "bin.add",
        "bin.move",
        "bin.remove",
        "branch.archive",
        "branch.default",
        "branch.ls",
        "branch.new",
        "branch.rename",
        "caption.import",
        "caption.merge",
        "caption.set-text",
        "caption.split",
        "caption.style",
        "checkpoint",
        "cherry-pick",
        "clip.delete",
        "clip.insert",
        "clip.trim",
        "diagnostics.consent",
        "diagnostics.export",
        "diagnostics.project",
        "diagnostics.purge",
        "diagnostics.show",
        "drop",
        "effect.add",
        "effect.move",
        "effect.remove",
        "eval.audio",
        "eval.frame",
        "fonts.fetch",
        "fonts.ls",
        "fonts.pin",
        "fonts.repin",
        "fonts.status",
        "fonts.unpin",
        "graphic.insert",
        "help",
        "image.insert",
        "item.delete",
        "item.link",
        "item.move",
        "item.slide",
        "item.slip",
        "item.split",
        "item.trim",
        "item.unlink",
        "job.cancel",
        "job.ls",
        "job.pause",
        "job.resume",
        "job.show",
        "keyframe.remove",
        "keyframe.set",
        "library.files",
        "library.ls",
        "library.scan",
        "library.search",
        "library.set",
        "library.show",
        "library.status",
        "look",
        "marker.add",
        "marker.remove",
        "marker.set",
        "media.caps",
        "merge",
        "param.set",
        "plugin.commands",
        "plugin.disable",
        "plugin.enable",
        "plugin.info",
        "plugin.install",
        "plugin.ls",
        "plugin.rm",
        "plugin.search",
        "plugin.suggest",
        "plugin.update",
        "project.acknowledge",
        "project.compact",
        "project.create",
        "project.open",
        "project.rebuild",
        "project.recover",
        "project.status",
        "project.takeover",
        "project.verify",
        "redo",
        "registry.add",
        "registry.forget",
        "registry.ls",
        "registry.refresh",
        "registry.rm",
        "registry.status",
        "review.close",
        "review.comment",
        "review.decide",
        "review.ls",
        "review.merge",
        "review.refresh",
        "review.request",
        "review.show",
        "revert",
        "schema",
        "schema.param",
        "search",
        "status",
        "style.add",
        "style.ls",
        "style.remove",
        "style.set",
        "timeline.add",
        "timeline.remove",
        "timeline.set",
        "track.add",
        "track.move",
        "track.remove",
        "track.set",
        "transition.remove",
        "transition.set",
        "try",
        "try.drop",
        "try.keep",
        "undo",
        "view",
        "wait",
        "whoami",
        "other",
    ],
);

/// `fin` long flag names, without the dashes, plus the `fin diagnostics` flags of Task D6.
pub static CLI_FLAGS: NameTable = NameTable::new(
    "CLI_FLAGS",
    &[
        "accept",
        "accept-empty-root",
        "accept-pending",
        "accept-unmounted",
        "accept-volume",
        "alias",
        "all",
        "any",
        "as",
        "as-upload",
        "asset",
        "at",
        "audio-track",
        "bandwidth",
        "before",
        "between",
        "bin",
        "branch",
        "brief",
        "budget",
        "cached",
        "catalog",
        "catalog-version",
        "clear",
        "color",
        "confirm",
        "content",
        "create",
        "cursor",
        "curve",
        "detail",
        "dir",
        "dismiss",
        "dismissed",
        "dry-run",
        "dur",
        "end",
        "exclude",
        "expect",
        "fallback",
        "from",
        "full",
        "hydrate",
        "id",
        "idem-key",
        "import",
        "in",
        "include",
        "index",
        "interp",
        "into",
        "italic",
        "key",
        "kind",
        "kinds",
        "label",
        "lib",
        "limit",
        "max-duration",
        "min-duration",
        "min-height",
        "min-width",
        "mine",
        "mode",
        "model",
        "name",
        "no-aliases",
        "no-audio",
        "no-exclude",
        "no-include",
        "no-linked",
        "no-wait",
        "out",
        "parent",
        "path",
        "pause",
        "pending",
        "pinned",
        "poll",
        "project",
        "purge-data",
        "range",
        "rate",
        "reason",
        "registry",
        "reject",
        "rejected",
        "relocate",
        "request-changes",
        "reset",
        "resolution",
        "restore",
        "resume",
        "rewind",
        "role",
        "root",
        "root-sha256",
        "salvage",
        "scope",
        "session",
        "set",
        "since",
        "size",
        "skip-conflicting",
        "slot",
        "srt",
        "start",
        "state",
        "status",
        "store",
        "style",
        "summary",
        "template",
        "timeline",
        "timeout",
        "title",
        "to",
        "to-run",
        "track",
        "tx",
        "undo",
        "up-to",
        "weight",
        "with-items",
        "word",
        "words",
        "other",
    ],
);

/// Media file extensions a [`super::class::PathRef`] may keep (lower case, no dot).
pub static MEDIA_EXTS: NameTable = NameTable::new(
    "MEDIA_EXTS",
    &[
        "aac", "aif", "aiff", "ass", "avi", "bmp", "cube", "flac", "gif", "heic", "jpeg", "jpg",
        "json", "lottie", "m2ts", "m4a", "m4v", "mkv", "mov", "mp3", "mp4", "mts", "mxf", "ogg",
        "opus", "otf", "png", "srt", "svg", "tif", "tiff", "ttf", "vtt", "wav", "webm", "webp",
        "woff", "woff2",
    ],
);

/// `eval.stats` keys: the three timed calls of `fin-node/src/eval.rs`.
pub static EVAL_KEYS: NameTable = NameTable::new(
    "EVAL_KEYS",
    &["eval.frame", "eval.audio", "fonts.plan", "other"],
);

/// Blob media types.
pub static MEDIA_TYPES: NameTable = NameTable::new(
    "MEDIA_TYPES",
    &[
        "image/webp",
        "image/png",
        "audio/opus",
        "audio/wav",
        "application/json",
        "other",
    ],
);

/// Op kinds (`log.commit.ops`): `fin-core`'s own op names.
pub static OP_KINDS: NameTable = NameTable::new("OP_KINDS", crate::ops::Op::NAMES);

pub use super::names_details::DETAIL_KEYS;

pub use super::agent_names::{
    AGENT_MODELS, AGENT_PROVIDERS, AGENT_ROLES, AGENT_TOOLS, AGENT_UPSTREAMS, agent_upstream,
};
pub use super::names_rpc::RPC_METHODS;

pub use super::plugin_names::{
    BUNDLED_PLUGINS, PLUGIN_ACTIONS, PLUGIN_METHOD_FAMILIES, bundled_plugin_name,
};

/// Every table, for the pattern test.
pub static TABLES: &[&NameTable] = &[
    &CLI_COMMANDS,
    &CLI_FLAGS,
    &RPC_METHODS,
    &MEDIA_EXTS,
    &EVAL_KEYS,
    &MEDIA_TYPES,
    &OP_KINDS,
    &DETAIL_KEYS,
    &AGENT_PROVIDERS,
    &AGENT_MODELS,
    &AGENT_ROLES,
    &AGENT_TOOLS,
    &AGENT_UPSTREAMS,
    &super::names_jobs::JOB_KINDS,
    &super::names_jobs::WORKER_FAMILIES,
    &super::names_jobs::SIGNALS,
    &super::names_media::MEDIA_CODECS,
    &super::names_media::MEDIA_BACKENDS,
    &BUNDLED_PLUGINS,
    &PLUGIN_ACTIONS,
    &PLUGIN_METHOD_FAMILIES,
];

// `deserialize_with` helpers: a `Name` field reads through its table.

fn one<'de, D: Deserializer<'de>>(d: D, t: &NameTable) -> Result<Name, D::Error> {
    Ok(t.get_or_other(&String::deserialize(d)?))
}

fn many<'de, D: Deserializer<'de>>(d: D, t: &NameTable) -> Result<Vec<Name>, D::Error> {
    let raw = Vec::<String>::deserialize(d)?;
    Ok(raw.iter().map(|s| t.get_or_other(s)).collect())
}

fn counts<'de, D: Deserializer<'de>>(d: D, t: &NameTable) -> Result<Counts<Name>, D::Error> {
    let raw = BTreeMap::<String, u64>::deserialize(d)?;
    let mut out: BTreeMap<Name, u64> = BTreeMap::new();
    for (k, n) in raw {
        *out.entry(t.get_or_other(&k)).or_default() += n;
    }
    Ok(Counts(out))
}

macro_rules! table_fns {
    ($helper:ident -> $out:ty: $($f:ident => $table:ident),* $(,)?) => {$(
        pub fn $f<'de, D: Deserializer<'de>>(d: D) -> Result<$out, D::Error> {
            $helper(d, &$table)
        }
    )*};
}

table_fns!(one -> Name:
    cli_command => CLI_COMMANDS,
    rpc_method => RPC_METHODS,
    eval_key => EVAL_KEYS,
    media_type => MEDIA_TYPES,
    agent_provider => AGENT_PROVIDERS,
    agent_model => AGENT_MODELS,
    agent_role => AGENT_ROLES,
    agent_tool => AGENT_TOOLS,
    bundled_plugin => BUNDLED_PLUGINS,
    plugin_action => PLUGIN_ACTIONS,
    plugin_method_family => PLUGIN_METHOD_FAMILIES,
);
table_fns!(many -> Vec<Name>: cli_flags => CLI_FLAGS, detail_keys => DETAIL_KEYS);
table_fns!(counts -> Counts<Name>: op_counts => OP_KINDS);

fn one_opt<'de, D: Deserializer<'de>>(d: D, t: &NameTable) -> Result<Option<Name>, D::Error> {
    Ok(Option::<String>::deserialize(d)?.map(|s| t.get_or_other(&s)))
}

table_fns!(one_opt -> Option<Name>:
    cli_command_opt => CLI_COMMANDS,
    agent_provider_opt => AGENT_PROVIDERS,
    agent_upstream_opt => AGENT_UPSTREAMS,
);

/// A media extension: kept only when `MEDIA_EXTS` holds it (lower-cased), else absent.
pub fn media_ext_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Name>, D::Error> {
    let raw = Option::<String>::deserialize(d)?;
    Ok(raw.and_then(|e| MEDIA_EXTS.get(&e.to_ascii_lowercase())))
}
