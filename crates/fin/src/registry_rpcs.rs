//! The RPCs each command stands for, for `fin schema --registry` (plan 7B Decisions 1, 2): the
//! server names an incoming RPC back to its command through these rows, so a rule on a command
//! holds whether the call came from `fin`, a hand-written client or the built-in tool. Data
//! only; the edit commands (all `tx.commit`) are in `registry_rpcs_edit.rs`.
//!
//! A row lists the RPCs that *act as* the command. Helper RPCs several commands call for
//! themselves (`snapshot.get` for a base, which is why `view` lists none, `tx.get`,
//! `asset.probe`, `fonts.plan`, the handshake) are not rows' but the server's `RPC_ONLY` set
//! (`command-of.ts`). A command with several entries is named when any one holds. Commands that
//! share an RPC and payload (`try` and `branch.new`, `status` and `project.status`) list the same
//! entry; the server then names the first in registry order and may check all of them.

use serde_json::{Value, json};

/// A condition on one payload field. The field is a dotted path; a `*` segment stands for "any
/// element of that array". A `null` is no value (`Present(false)`).
#[derive(Clone, Copy)]
pub enum Match {
    /// The field equals this string.
    Eq(&'static str),
    /// The field equals this boolean.
    Is(bool),
    /// The field is (`true`) or is not (`false`) present.
    Present(bool),
}

impl Match {
    pub fn to_json(self) -> Value {
        match self {
            Match::Eq(s) => json!(s),
            Match::Is(b) => json!(b),
            Match::Present(p) => json!({ "present": p }),
        }
    }
}

/// One RPC a command sends, with the payload conditions that single it out among the commands
/// sharing the RPC.
pub struct RpcUse {
    pub method: &'static str,
    pub matches: &'static [(&'static str, Match)],
}

pub struct Row {
    pub name: &'static str,
    pub rpcs: &'static [RpcUse],
    /// Payload fields that identify what the command acts on (the approval card's subject).
    pub identity: &'static [&'static str],
}

/// Rows of commands sending an RPC with no condition.
macro_rules! rpc {
    ($($method:literal),*) => { &[$(RpcUse { method: $method, matches: &[] }),*] };
}
/// A row sending one RPC where the listed fields match.
macro_rules! rpc_if {
    ($method:literal, $($field:expr => $m:expr),+) => {
        &[RpcUse { method: $method, matches: &[$(($field, $m)),+] }]
    };
}
macro_rules! row {
    ($name:literal, $rpcs:expr, $id:expr) => {
        Row {
            name: $name,
            rpcs: $rpcs,
            identity: $id,
        }
    };
}
pub(crate) use {row, rpc, rpc_if};

const P: &[&str] = &["projectId"];
const PB: &[&str] = &["projectId", "branch"];
const NONE: &[&str] = &[];

/// Every command but the edit commands, in registry order.
pub const ROWS: &[Row] = &[
    row!("project.create", rpc!("project.create"), &["dir", "name"]),
    row!("project.open", rpc!("project.open"), &["dir"]),
    row!("project.status", rpc!("project.status"), P),
    row!("project.takeover", rpc!("project.takeover"), P),
    row!("project.verify", rpc!("project.verify"), P),
    row!("project.rebuild", rpc!("project.rebuild"), P),
    row!("project.compact", rpc!("project.compact"), P),
    row!(
        "project.recover",
        rpc!("project.recover"),
        &["projectId", "create", "importDir", "dismiss"]
    ),
    row!(
        "project.acknowledge",
        rpc!("project.acknowledgeRewind", "project.acknowledgeSalvage"),
        P
    ),
    row!("status", rpc!("project.status"), P),
    row!(
        "look",
        rpc!("media.look"),
        &["projectId", "branch", "timeline", "at"]
    ),
    row!("view", rpc!(), PB),
    row!(
        "branch.new",
        rpc!("branch.create"),
        &["projectId", "from", "name"]
    ),
    row!("branch.ls", rpc!("branch.list"), &["projectId", "all"]),
    row!(
        "branch.rename",
        rpc_if!("branch.set", "name" => Match::Present(true)),
        &["projectId", "branch", "name"]
    ),
    row!(
        "branch.archive",
        rpc_if!("branch.set", "archived" => Match::Present(true)),
        &["projectId", "branch", "archived"]
    ),
    row!("branch.default", rpc!(), NONE),
    row!(
        "checkpoint",
        rpc!("history.checkpoint"),
        &["projectId", "branch", "name"]
    ),
    row!("undo", rpc!("history.undo"), &["projectId", "branch", "to"]),
    row!("redo", rpc!("history.redo"), &["projectId", "branch"]),
    row!(
        "merge",
        rpc!("history.merge"),
        &["projectId", "from", "into"]
    ),
    row!(
        "cherry-pick",
        rpc!("history.cherryPick"),
        &["projectId", "origin", "into"]
    ),
    row!(
        "drop",
        rpc!("history.drop"),
        &["projectId", "from", "into", "drops", "undrops"]
    ),
    row!(
        "review.request",
        rpc!("review.request"),
        &["projectId", "source", "into"]
    ),
    row!(
        "review.ls",
        rpc!("review.list"),
        &["projectId", "state", "kind"]
    ),
    row!("review.show", rpc!("review.get"), &["itemId"]),
    row!("review.refresh", rpc!("review.refresh"), &["itemId"]),
    row!(
        "review.comment",
        rpc!("review.comment"),
        &["itemId", "txId"]
    ),
    row!(
        "review.decide",
        rpc!("review.decide"),
        &["itemId", "decisions.*.txId", "decisions.*.decision"]
    ),
    row!(
        "review.merge",
        rpc!("review.merge"),
        &["itemId", "acceptPending"]
    ),
    row!("review.close", rpc!("review.close"), &["itemId", "state"]),
    row!(
        "revert",
        rpc!("history.revert"),
        &["projectId", "mergeId", "branch"]
    ),
    row!("try", rpc!("branch.create"), &["projectId", "from", "name"]),
    row!(
        "try.keep",
        rpc!("history.merge"),
        &["projectId", "from", "into"]
    ),
    row!(
        "try.drop",
        rpc_if!("branch.set", "archived" => Match::Is(true)),
        &["projectId", "branch", "archived"]
    ),
    row!("style.ls", rpc!(), NONE),
    row!(
        "eval.frame",
        rpc!("eval.frame"),
        &["projectId", "branch", "timeline", "at"]
    ),
    row!(
        "eval.audio",
        rpc!("eval.audio"),
        &["projectId", "branch", "timeline", "from", "to"]
    ),
    row!("media.caps", rpc!("media.capabilities"), NONE),
    row!("fonts.ls", rpc!("fonts.catalog"), NONE),
    row!(
        "fonts.fetch",
        rpc!("fonts.fetch"),
        &["sha256", "entries", "keys", "pinned"]
    ),
    row!("fonts.status", rpc!("fonts.status"), NONE),
    row!("diagnostics.show", rpc!("diagnostics.status"), NONE),
    row!("diagnostics.export", rpc!("diagnostics.export"), NONE),
    row!(
        "diagnostics.purge",
        rpc!("diagnostics.purge"),
        &["selector"]
    ),
    row!(
        "diagnostics.project",
        rpc!("diagnostics.project"),
        &["projectId"]
    ),
    row!("diagnostics.consent", rpc!("diagnostics.consent"), NONE),
    row!(
        "plugin.search",
        rpc!("plugin.search"),
        &["text", "kind", "registry"]
    ),
    row!("plugin.info", rpc!("plugin.info"), &["id"]),
    row!("plugin.ls", rpc!("plugin.list"), NONE),
    row!("plugin.suggest", rpc!("plugin.suggest"), &["id"]),
    row!(
        "plugin.install",
        &[
            RpcUse {
                method: "plugin.plan",
                matches: &[("update", Match::Present(false))]
            },
            RpcUse {
                method: "plugin.plan",
                matches: &[("update", Match::Is(false))]
            },
            RpcUse {
                method: "plugin.install",
                matches: &[]
            },
        ],
        &["id", "version", "registry", "planId", "digest"]
    ),
    row!(
        "plugin.update",
        rpc_if!("plugin.plan", "update" => Match::Is(true)),
        &["id", "version", "registry"]
    ),
    row!("plugin.rm", rpc!("plugin.remove"), &["id"]),
    row!("plugin.enable", rpc!("plugin.enable"), &["id"]),
    row!("plugin.disable", rpc!("plugin.disable"), &["id"]),
    row!("plugin.commands", rpc!("plugin.commands"), NONE),
    row!("registry.ls", rpc!("registry.list"), NONE),
    row!("registry.status", rpc!("registry.status"), &["id"]),
    row!("registry.refresh", rpc!("registry.refresh"), &["id"]),
    row!("registry.add", rpc!("registry.add"), &["cfg", "root"]),
    row!("registry.rm", rpc!("registry.remove"), &["id"]),
    row!("registry.forget", rpc!("registry.forget"), &["rootSha256"]),
    row!(
        "job.ls",
        rpc!("jobs.list"),
        &["state", "kind", "project", "mine"]
    ),
    row!("job.show", rpc!("jobs.get"), &["job"]),
    row!("job.cancel", rpc!("jobs.cancel"), &["job"]),
    row!("job.pause", rpc!("jobs.pause"), &["job"]),
    row!("job.resume", rpc!("jobs.resume"), &["job"]),
    row!("wait", rpc!("jobs.wait"), &["ids", "any"]),
    row!("library.ls", rpc!("library.list"), NONE),
    row!("library.status", rpc!("library.status"), &["id"]),
    row!(
        "library.scan",
        rpc!("library.scan"),
        &["id", "full", "dirs"]
    ),
    row!(
        "library.files",
        rpc!("library.files"),
        &["id", "dir", "state", "kind"]
    ),
    row!(
        "library.show",
        rpc!("library.file"),
        &["id", "path", "fileId"]
    ),
    row!(
        "library.search",
        rpc!("library.search"),
        &["query", "libraries"]
    ),
    row!("library.set", rpc!("library.set"), &["id"]),
    row!("search", rpc!("library.search"), &["query", "libraries"]),
    row!("approval.wait", rpc!("agent.approval.wait"), &["id"]),
    row!(
        "approval.ls",
        rpc!("agent.approval.list"),
        &["sessionId", "status"]
    ),
    // 7B Task 6: `send` is a steer, which prompts an idle child itself; `ls` lists children.
    row!("agent.spawn", rpc!("agent.session.start"), &["role"]),
    row!("agent.wait", rpc!("agent.session.wait"), &["ids"]),
    row!("agent.ls", rpc!("agent.session.list"), NONE),
    row!("agent.send", rpc!("agent.session.steer"), &["sessionId"]),
    row!("agent.stop", rpc!("agent.session.end"), &["sessionId"]),
    // 7B Task 8: a parent's rollback of its own direct child.
    row!(
        "agent.rollback",
        rpc!("agent.session.rollback"),
        &["sessionId"]
    ),
    // 7B Decision 16 as amended (2026-11-13): a resume of a child the parent interrupted.
    row!("agent.resume", rpc!("agent.session.resume"), &["sessionId"]),
    row!("help", rpc!(), NONE),
    row!("whoami", rpc!(), NONE),
    row!("schema", rpc!(), NONE),
    row!("schema.param", rpc!(), NONE),
];
