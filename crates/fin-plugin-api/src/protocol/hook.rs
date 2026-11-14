//! Hooks (sub-project 7 spec §9; 7F plan Decisions 1 and 2): the events, the request a hook
//! reads and the reply it gives. One source for command hooks (stdin and stdout of a hook
//! process) and plugin handlers (a `plugin.hook` v1 job's params and result).
//!
//! A reply is `allow`, `block {reason}` or `context {text}`: there is no `modify` (spec §9.1,
//! §1.6: cut from v1), so a `modify` reply does not decode and counts as a failed hook. The
//! request is host → hook and decodes leniently; the reply is hook → host and refuses unknown
//! fields.

use fin_core::events::payloads::{AgentMode, Harness};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The version of [`HookRequest`] this host sends (`v`).
pub const HOOK_PROTOCOL_VERSION: u32 = 1;

/// An event a hook can subscribe to (arch §10 without `PreOp`/`PostOp`, spec §9.1).
/// `PreRender` and `PostRender` are accepted and inert until a render job kind exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum HookEvent {
    SessionStart,
    SessionEnd,
    UserPrompt,
    PreCommand,
    PostCommand,
    BranchReady,
    PreMerge,
    PostMerge,
    PreRender,
    PostRender,
    JobDone,
    Approval,
    Notification,
}

/// How an event's hooks run and what their `block` does (Decision 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookClass {
    /// Awaited before the action; a `block` refuses it.
    BlockingBefore,
    /// `PostCommand`: after the command landed; a `block` stops the later handlers and is
    /// reported, but cannot undo the command.
    BlockingAfter,
    /// `SessionStart`: awaited for its context; a `block` is ignored with a notice.
    Awaited,
    /// Detached; a `block` is ignored with a notice.
    Observe,
}

impl HookEvent {
    /// Every event, in spec §9.1's order.
    pub const ALL: [HookEvent; 13] = [
        HookEvent::SessionStart,
        HookEvent::SessionEnd,
        HookEvent::UserPrompt,
        HookEvent::PreCommand,
        HookEvent::PostCommand,
        HookEvent::BranchReady,
        HookEvent::PreMerge,
        HookEvent::PostMerge,
        HookEvent::PreRender,
        HookEvent::PostRender,
        HookEvent::JobDone,
        HookEvent::Approval,
        HookEvent::Notification,
    ];

    pub fn class(self) -> HookClass {
        match self {
            HookEvent::UserPrompt
            | HookEvent::PreCommand
            | HookEvent::PreMerge
            | HookEvent::BranchReady
            | HookEvent::PreRender => HookClass::BlockingBefore,
            HookEvent::PostCommand => HookClass::BlockingAfter,
            HookEvent::SessionStart => HookClass::Awaited,
            HookEvent::SessionEnd
            | HookEvent::PostMerge
            | HookEvent::PostRender
            | HookEvent::JobDone
            | HookEvent::Approval
            | HookEvent::Notification => HookClass::Observe,
        }
    }

    /// Whether a timeout or other failure blocks when the hook names no `onTimeout`
    /// (Decision 3): only `PreMerge` fails closed by default.
    pub fn default_on_timeout_blocks(self) -> bool {
        self == HookEvent::PreMerge
    }
}

/// A hook's answer (Decision 2): one JSON object on stdout, or a `plugin.hook` job's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "reply", rename_all = "camelCase", deny_unknown_fields)]
pub enum HookReply {
    /// A struct variant, not a unit one: serde's internally tagged unit variants ignore unknown
    /// fields, and a reply refuses them.
    Allow {},
    Block {
        reason: String,
    },
    Context {
        text: String,
    },
}

/// What a `plugin.hook` v1 job returns.
pub type HookOut = HookReply;

/// The session an event belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HookSession {
    pub id: String,
    /// The session's role name.
    pub role: String,
    pub mode: AgentMode,
    pub harness: Harness,
    /// The branch the session's root works on, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_branch: Option<String>,
}

/// The project an event belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HookProject {
    pub id: String,
    /// The project folder (read-only to the hook).
    pub dir: String,
}

/// What a hook reads: one JSON line on stdin, then EOF (Decision 9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HookRequest {
    pub event: HookEvent,
    /// [`HOOK_PROTOCOL_VERSION`].
    pub v: u32,
    /// Absent for events outside a session (`JobDone` of a job, a person's merge, `UserPrompt`
    /// before `agent.session.start` opens the session).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<HookSession>,
    /// Absent for a server-scoped event (a call or a job that names no project): only the
    /// managed, user and plugin layers run for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<HookProject>,
    /// The event's payload (over 1 MiB: `{payloadOmitted: true, bytes}`).
    pub payload: Value,
}

impl HookRequest {
    /// A request of this host's protocol version.
    pub fn new(
        event: HookEvent,
        session: Option<HookSession>,
        project: Option<HookProject>,
        payload: Value,
    ) -> Self {
        HookRequest {
            event,
            v: HOOK_PROTOCOL_VERSION,
            session,
            project,
            payload,
        }
    }
}

/// A `plugin.hook` v1 job's params: the manifest handler to call with the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HookRun {
    pub handler: String,
    pub request: HookRequest,
}
