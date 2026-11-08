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
/// `fin diagnostics` commands of Task D6.
pub static CLI_COMMANDS: NameTable = NameTable::new(
    "CLI_COMMANDS",
    &[
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
        "keyframe.remove",
        "keyframe.set",
        "marker.add",
        "marker.remove",
        "marker.set",
        "merge",
        "param.set",
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
        "revert",
        "schema",
        "schema.param",
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
        "whoami",
        "other",
    ],
);

/// `fin` long flag names, without the dashes, plus the `fin diagnostics` flags of Task D6.
pub static CLI_FLAGS: NameTable = NameTable::new(
    "CLI_FLAGS",
    &[
        "all",
        "as",
        "as-upload",
        "asset",
        "at",
        "audio-track",
        "before",
        "between",
        "bin",
        "branch",
        "budget",
        "cached",
        "catalog",
        "catalog-version",
        "clear",
        "color",
        "confirm",
        "create",
        "curve",
        "detail",
        "dismiss",
        "dry-run",
        "dur",
        "end",
        "fallback",
        "from",
        "idem-key",
        "import",
        "in",
        "index",
        "interp",
        "into",
        "italic",
        "kind",
        "label",
        "limit",
        "mode",
        "name",
        "no-audio",
        "no-linked",
        "out",
        "parent",
        "path",
        "pinned",
        "project",
        "range",
        "rate",
        "relocate",
        "reset",
        "resolution",
        "restore",
        "rewind",
        "salvage",
        "scope",
        "set",
        "since",
        "skip-conflicting",
        "slot",
        "srt",
        "start",
        "style",
        "template",
        "timeline",
        "to",
        "track",
        "weight",
        "with-items",
        "word",
        "words",
        "other",
    ],
);

/// `finisherd` RPC methods (`@finisher/contracts` `FinRpcs`), plus the `diagnostics.*` RPCs of
/// Task D4 and the handshake's `auth.prove` (5A Task 3).
pub static RPC_METHODS: NameTable = NameTable::new(
    "RPC_METHODS",
    &[
        "asset.probe",
        "auth.prove",
        "branch.create",
        "branch.list",
        "branch.set",
        "diagnostics.consent",
        "diagnostics.export",
        "diagnostics.project",
        "diagnostics.purge",
        "diagnostics.record",
        "diagnostics.status",
        "eval.audio",
        "eval.frame",
        "fonts.catalog",
        "fonts.fetch",
        "fonts.plan",
        "fonts.status",
        "history.checkpoint",
        "history.cherryPick",
        "history.merge",
        "history.redo",
        "history.revert",
        "history.undo",
        "ops.subscribe",
        "plugin.info",
        "plugin.list",
        "plugin.search",
        "project.acknowledgeRewind",
        "project.acknowledgeSalvage",
        "project.compact",
        "project.create",
        "project.events",
        "project.open",
        "project.rebuild",
        "project.recover",
        "project.status",
        "project.takeover",
        "project.verify",
        "registry.add",
        "registry.forget",
        "registry.list",
        "registry.refresh",
        "registry.remove",
        "registry.status",
        "server.hello",
        "snapshot.get",
        "tx.commit",
        "tx.get",
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

/// Keys of an error's `details` object (`error.internal.details`), as Finisher code writes
/// them.
pub static DETAIL_KEYS: NameTable = NameTable::new(
    "DETAIL_KEYS",
    &[
        "actual",
        "allocated",
        "alternatives",
        "archived",
        "asset",
        "assetDuration",
        "assetKind",
        "assets",
        "at",
        "audioTrack",
        "author",
        "baseRev",
        "between",
        "bin",
        "bins",
        "branch",
        "canonical",
        "cause",
        "client",
        "clip",
        "clips",
        "code",
        "codePoint",
        "conflicts",
        "cwd",
        "dbProjectId",
        "defect",
        "delta",
        "edge",
        "effect",
        "end",
        "error",
        "expected",
        "face",
        "failed",
        "field",
        "files",
        "frame",
        "from",
        "genesis",
        "got",
        "group",
        "have",
        "head",
        "id",
        "idemKey",
        "in",
        "infoFile",
        "input",
        "into",
        "item",
        "items",
        "key",
        "keyframes",
        "kind",
        "line",
        "link",
        "lock",
        "max",
        "maxDepth",
        "min",
        "mirrorOf",
        "name",
        "need",
        "neighbour",
        "op",
        "opIndex",
        "operation",
        "out",
        "parent",
        "path",
        "pins",
        "placeholder",
        "prefix",
        "problem",
        "problems",
        "projectId",
        "rate",
        "reason",
        "recordKind",
        "recorded",
        "report",
        "rev",
        "rule",
        "schemaVersion",
        "scope",
        "seq",
        "server",
        "sinceMs",
        "slot",
        "slots",
        "source",
        "sourceIn",
        "start",
        "step",
        "style",
        "styles",
        "supported",
        "tag",
        "timeline",
        "timelines",
        "to",
        "track",
        "trackKind",
        "transition",
        "txId",
        "unit",
        "value",
        "wanted",
        "with",
        "word",
        "other",
    ],
);

pub use super::agent_names::{
    AGENT_MODELS, AGENT_PROVIDERS, AGENT_ROLES, AGENT_TOOLS, AGENT_UPSTREAMS, agent_upstream,
};

/// Bundled plugins (sub-project 8 spec §11): the ids of the plugins the app ships, without
/// the leading `@` (a [`Name`] starts with a letter). `fin-node`'s compiled-in
/// `BUNDLED_PLUGINS` list (8A Task 10) must be a subset; any other plugin is a keyed `Hashed`.
pub static BUNDLED_PLUGINS: NameTable =
    NameTable::new("BUNDLED_PLUGINS", &["finisher/captions-basic"]);

/// `plugin.lifecycle` actions (sub-project 8 spec §11).
pub static PLUGIN_ACTIONS: NameTable = NameTable::new(
    "PLUGIN_ACTIONS",
    &[
        "plan",
        "install",
        "update",
        "downgrade",
        "remove",
        "enable",
        "disable",
        "revoke",
    ],
);

/// The [`BUNDLED_PLUGINS`] name of plugin id `@publisher/name`, if the table lists it.
pub fn bundled_plugin_name(id: &str) -> Option<Name> {
    id.strip_prefix('@')
        .and_then(|rest| BUNDLED_PLUGINS.get(rest))
}

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
    &BUNDLED_PLUGINS,
    &PLUGIN_ACTIONS,
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
