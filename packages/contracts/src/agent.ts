// The agent runtime's RPC-facing schemas (sub-project 7 §3.2, §3.5, §5.1; plan 7A Task 2,
// Decisions 6, 14a, 15): sessions, runs, nodes, approvals, the watch stream's items, the
// provider overview and the calling session's own state. TS-owned control-plane records, so
// Effect Schemas (the enums and `ProviderEvent` are in `agent-events.ts`). Times are ms since
// the epoch; ids are ULIDs; sizes and counts are non-negative integers.
import { Schema } from "effect";
import { BudgetSpec, ContentBlock, Harness, Mode, NodeKind, SessionStatus } from "./agent-events.ts";
import { ChallengeField, PrincipalKind } from "./auth.ts";

const Count = Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0));
const Id = Schema.String;

/** Where a session's model runs: `openrouter` and `local` for the built-in agent, the maker for a CLI. */
export const AgentProvider = Schema.Literals(["openrouter", "anthropic", "openai", "local"]);
export type AgentProvider = typeof AgentProvider.Type;

/** The two presets of the built-in agent's one client (plan 7A Decision 8): what `start` may ask for. */
export const BuiltinPreset = Schema.Literals(["openrouter", "local"]);
export type BuiltinPreset = typeof BuiltinPreset.Type;

/**
 * Whether a model's weights are published (`open`), not (`closed`), or not known (`unknown`):
 * a provenance tag on every session and turn (spec §12.7; owner decision 1), never a licence
 * statement. A local model is never `closed`.
 */
export const Weights = Schema.Literals(["open", "closed", "unknown"]);
export type Weights = typeof Weights.Type;

/** What the agent's work cost: how the figure was known (plan 7A Decisions 8, 19). */
export const CostBasis = Schema.Literals(["reported", "priced", "subscription", "unknown"]);
export type CostBasis = typeof CostBasis.Type;

/** Tokens and money used by a run or a session; money in millionths of a USD. */
export const Usage = Schema.Struct({
  input: Count,
  output: Count,
  cacheRead: Count,
  cacheWrite: Count,
  reasoning: Count,
  moneyMicros: Count,
  costBasis: Schema.optionalKey(CostBasis),
  /** Tokens counted by Finisher (characters ÷ 4), not reported by the server. */
  estimated: Schema.optionalKey(Schema.Boolean),
});
export type Usage = typeof Usage.Type;

/** What one run or session may still spend: the limit and what is used, per kind. */
export const BudgetState = Schema.Struct({
  tokens: Schema.Struct({ limit: Count, used: Count }),
  moneyMicros: Schema.Struct({ limit: Count, used: Count }),
  wallMs: Schema.Struct({ limit: Count, used: Count }),
});
export type BudgetState = typeof BudgetState.Type;

/** Who started a session: the principal, and the actor it acted for (a device, a person), if known. */
export const SessionCreator = Schema.Struct({
  principal: Schema.Struct({ kind: PrincipalKind, id: Id }),
  actor: Schema.optionalKey(Schema.Struct({ kind: Schema.String, id: Id })),
});

export const Session = Schema.Struct({
  id: Id,
  projectId: Id,
  harness: Harness,
  provider: AgentProvider,
  model: Schema.String,
  weights: Weights,
  role: Schema.String,
  mode: Mode,
  /**
   * The id of the branch the session writes (named `agent.<role>-<n>`; 7A Task 7); absent in
   * `plan` mode, which creates none.
   */
  rootBranch: Schema.optionalKey(Schema.String),
  /** The branch and revision it was started from. */
  base: Schema.Struct({ branch: Schema.String, seq: Count }),
  parent: Schema.optionalKey(Id),
  status: SessionStatus,
  /** The code that interrupted or ended it (`serverRestart`, `BUDGET_EXCEEDED`, …). */
  statusReason: Schema.optionalKey(Schema.String),
  /** The credential the session runs as: its id is the session's. */
  credentialId: Id,
  budget: BudgetSpec,
  usage: Usage,
  createdBy: SessionCreator,
  createdAt: Count,
  endedAt: Schema.optionalKey(Count),
  title: Schema.optionalKey(Schema.String),
});
export type Session = typeof Session.Type;

export const RunTrigger = Schema.Literals(["prompt", "steer", "subagentResult", "reviewFeedback", "hook", "eval"]);
export type RunTrigger = typeof RunTrigger.Type;

export const RunStatus = Schema.Literals(["running", "completed", "interrupted", "failed"]);
export type RunStatus = typeof RunStatus.Type;

export const Run = Schema.Struct({
  id: Id,
  sessionId: Id,
  seq: Count,
  trigger: RunTrigger,
  status: RunStatus,
  startedAt: Count,
  endedAt: Schema.optionalKey(Count),
  /** The root branch's revision when the run began. */
  headAtStart: Schema.optionalKey(Count),
  stopReason: Schema.optionalKey(Schema.String),
  usage: Usage,
});
export type Run = typeof Run.Type;

/**
 * One item of the conversation. Small payloads are inline (`payload`); larger ones are kept
 * beside the session and named by `blobRef` (a BLAKE3 hex), readable through the same call.
 * `callId` is the Finisher call id of a `toolCall` node (`FIN_CALL`).
 */
export const Node = Schema.Struct({
  id: Id,
  runId: Id,
  sessionId: Id,
  seq: Count,
  kind: NodeKind,
  providerId: Schema.optionalKey(Schema.String),
  callId: Schema.optionalKey(Id),
  payload: Schema.optionalKey(Schema.Unknown),
  blobRef: Schema.optionalKey(Schema.String),
  createdAt: Count,
});
export type Node = typeof Node.Type;

export const ApprovalKind = Schema.Literals(["rule", "budget", "provider", "plugin"]);
export type ApprovalKind = typeof ApprovalKind.Type;

export const ApprovalDecision = Schema.Literals(["allow", "deny"]);
export type ApprovalDecision = typeof ApprovalDecision.Type;

/** `open` until answered; `expired` when its time ran out or its run is gone. */
export const ApprovalStatus = Schema.Literals(["open", "answered", "expired"]);
export type ApprovalStatus = typeof ApprovalStatus.Type;

/** What an answer may add to a rule: for the session, or for the project (spec §7.3). */
export const RememberScope = Schema.Literals(["session", "project"]);
export type RememberScope = typeof RememberScope.Type;

/** An `ask` waiting for a person (spec §7.3). The subject is written by the server, never by the agent. */
export const Approval = Schema.Struct({
  id: Id,
  sessionId: Id,
  kind: ApprovalKind,
  status: ApprovalStatus,
  /** The command and its resolved arguments, or the budget kind and amount: what the card shows. */
  subject: Schema.Unknown,
  argsHash: Schema.String,
  requestedAt: Count,
  expiresAt: Count,
  decision: Schema.optionalKey(ApprovalDecision),
  /** The principal that answered (`kind:id`), or `policy`. */
  by: Schema.optionalKey(Schema.String),
  answeredAt: Schema.optionalKey(Count),
  consumedAt: Schema.optionalKey(Count),
});
export type Approval = typeof Approval.Type;

// The watch stream (`agent.watch`): nodes, deltas, status, usage and approvals. Partial deltas
// are streamed, not stored; the completed node is (spec §3.5).
const item = <T extends string, F extends Schema.Struct.Fields>(type: T, fields: F) =>
  Schema.Struct({ type: Schema.Literal(type), ...fields });

export const NodeAdded = item("nodeAdded", { node: Node });
export const DeltaKind = Schema.Literals(["message", "reasoning", "toolOutput"]);
export const Delta = item("delta", {
  runId: Id,
  kind: DeltaKind,
  text: Schema.String,
  /** The tool call a `toolOutput` delta belongs to. */
  callId: Schema.optionalKey(Id),
});
export const StatusChanged = item("statusChanged", {
  status: SessionStatus,
  reason: Schema.optionalKey(Schema.String),
  runId: Schema.optionalKey(Id),
});
export const UsageUpdated = item("usageUpdated", { runId: Schema.optionalKey(Id), usage: Usage, session: Usage, budget: BudgetState });
export const ApprovalOpened = item("approvalOpened", { approval: Approval });

export const WatchItem = Schema.Union([NodeAdded, Delta, StatusChanged, UsageUpdated, ApprovalOpened]);
export type WatchItem = typeof WatchItem.Type;

/** Every `WatchItem` type, in union order. */
export const WATCH_ITEM_TYPES = ["nodeAdded", "delta", "statusChanged", "usageUpdated", "approvalOpened"] as const satisfies readonly WatchItem["type"][];

// Payloads and answers, one per RPC (spec §3.5). `challenge` is the console's typed code on the
// human-only calls (5A Decision 30).

/** The longest prompt, steer or title text, in characters. */
export const AGENT_TEXT_MAX = 200_000;
export const AGENT_TITLE_MAX = 200;
/** At most this many attachments with a prompt. */
export const AGENT_ATTACHMENTS_MAX = 8;
/** At most this many sessions or approvals in one list page. */
export const AGENT_PAGE_MAX = 200;

const Text = Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(AGENT_TEXT_MAX));
const Attachments = Schema.Array(ContentBlock).check(Schema.isMaxLength(AGENT_ATTACHMENTS_MAX));
const SessionIdPayload = Schema.Struct({ sessionId: Id });

/**
 * `agent.session.start`: every field but `projectId` takes the role's or the server's default
 * (role `editor`, mode `draft`, harness `builtin`, preset `openrouter`). `base` is a branch
 * reference (default: `main`'s head). Answers the new session, and its first run when `prompt`
 * is given.
 */
export const AgentSessionStartPayload = Schema.Struct({
  projectId: Id,
  harness: Schema.optionalKey(Harness),
  /** The built-in agent's preset; ignored by a CLI harness. */
  provider: Schema.optionalKey(BuiltinPreset),
  model: Schema.optionalKey(Schema.String),
  role: Schema.optionalKey(Schema.String),
  mode: Schema.optionalKey(Mode),
  base: Schema.optionalKey(Schema.String),
  title: Schema.optionalKey(Schema.String.check(Schema.isMaxLength(AGENT_TITLE_MAX))),
  prompt: Schema.optionalKey(Text),
  attachments: Schema.optionalKey(Attachments),
  budget: Schema.optionalKey(BudgetSpec),
});
export const AgentSessionStartSuccess = Schema.Struct({ session: Session, run: Schema.optionalKey(Run) });

/** `agent.session.prompt`: starts a run; `SESSION_BUSY` while one runs (use `steer`). */
export const AgentSessionPromptPayload = Schema.Struct({
  sessionId: Id,
  text: Text,
  attachments: Schema.optionalKey(Attachments),
});
export const AgentSessionPromptSuccess = Schema.Struct({ run: Run });

/** `agent.session.steer`: into the running turn where the provider supports it, else queued. */
export const AgentSessionSteerPayload = Schema.Struct({ sessionId: Id, text: Text });
export const AgentSessionSteerSuccess = Schema.Struct({ queued: Schema.Boolean });

/** `agent.session.interrupt|resume|end`: the session after the change. Interrupting keeps the branch. */
export const AgentSessionControlPayload = SessionIdPayload;
export const AgentSessionControlSuccess = Schema.Struct({ session: Session });

/** `agent.session.list`: newest first; `cursor` is a page's `next`; `limit` 1–200 (default 50). */
export const AgentSessionListPayload = Schema.Struct({
  projectId: Schema.optionalKey(Id),
  status: Schema.optionalKey(SessionStatus),
  cursor: Schema.optionalKey(Schema.String),
  limit: Schema.optionalKey(Count.check(Schema.isBetween({ minimum: 1, maximum: AGENT_PAGE_MAX }))),
});
export const AgentSessionListSuccess = Schema.Struct({ sessions: Schema.Array(Session), next: Schema.optionalKey(Schema.String) });

/** `agent.session.get`: the session, its runs and the approvals still open. Nodes come through `agent.watch`. */
export const AgentSessionGetPayload = SessionIdPayload;
export const AgentSessionGetSuccess = Schema.Struct({ session: Session, runs: Schema.Array(Run), approvals: Schema.Array(Approval) });

/** `agent.session.delete` (owner, typed code): removes the session's product state at once. */
export const AgentSessionDeletePayload = Schema.Struct({ sessionId: Id, challenge: ChallengeField });
export const AgentSessionDeleteSuccess = Schema.Struct({ deleted: Schema.Literal(true) });

/** `agent.watch` (stream): the nodes after `fromNode` (a node id; default: all), then live items. */
export const AgentWatchPayload = Schema.Struct({ sessionId: Id, fromNode: Schema.optionalKey(Id) });

export const AgentApprovalListPayload = Schema.Struct({
  sessionId: Schema.optionalKey(Id),
  projectId: Schema.optionalKey(Id),
  status: Schema.optionalKey(ApprovalStatus),
});
export const AgentApprovalListSuccess = Schema.Struct({ approvals: Schema.Array(Approval) });

/**
 * `agent.approval.answer` (human, typed code): `remember` adds a visible, revocable rule for
 * the session or the project (never a standing grant); `add` raises a `budget` approval's limits.
 */
export const AgentApprovalAnswerPayload = Schema.Struct({
  id: Id,
  decision: ApprovalDecision,
  remember: Schema.optionalKey(RememberScope),
  add: Schema.optionalKey(
    Schema.Struct({ tokens: Schema.optionalKey(Count), moneyMicros: Schema.optionalKey(Count), wallMs: Schema.optionalKey(Count) }),
  ),
  challenge: ChallengeField,
});
export const AgentApprovalAnswerSuccess = Schema.Struct({ approval: Approval });

/** What an adapter can do (spec §4.1). */
export const Capabilities = Schema.Struct({
  steer: Schema.Boolean,
  rollback: Schema.Boolean,
  resume: Schema.Boolean,
  reasoning: Schema.Boolean,
  images: Schema.Boolean,
  parallelTools: Schema.Boolean,
  costReported: Schema.Boolean,
  exactCapture: Schema.Boolean,
  nativeSubagents: Schema.Boolean,
});
export type Capabilities = typeof Capabilities.Type;

/**
 * A harness as detection finds it: whether it is installed, its version, whether the version is
 * supported, and sign-in state and account label **as the CLI reports them**. Never a credential.
 */
export const ProviderInfo = Schema.Struct({
  harness: Harness,
  detected: Schema.Boolean,
  version: Schema.optionalKey(Schema.String),
  supported: Schema.optionalKey(Schema.Boolean),
  signedIn: Schema.optionalKey(Schema.NullOr(Schema.Boolean)),
  account: Schema.optionalKey(Schema.String),
  capabilities: Capabilities,
  /** Why detection failed, as `{code, message, details, fix?}`. */
  problem: Schema.optionalKey(Schema.Struct({ code: Schema.String, message: Schema.String, fix: Schema.optionalKey(Schema.String) })),
});
export type ProviderInfo = typeof ProviderInfo.Type;

/** The local endpoint's preset: shown, never its address (no record holds its host or port). */
export const LocalPreset = Schema.Literals(["ollama", "lmstudio", "custom"]);

/**
 * `agent.providers`: the harnesses, whether an OpenRouter key is set (never the key), and the
 * local endpoint's preset and whether it answers (never its address).
 */
export const AgentProvidersSuccess = Schema.Struct({
  providers: Schema.Array(ProviderInfo),
  openrouter: Schema.Struct({ keySet: Schema.Boolean }),
  local: Schema.Struct({ configured: Schema.Boolean, preset: Schema.optionalKey(LocalPreset), reachable: Schema.optionalKey(Schema.Boolean) }),
});

/** One recorded `fin try`: its branch and the default it replaced, restored when the try ends. */
export const SessionTry = Schema.Struct({ branch: Schema.String, previousDefault: Schema.String });

/**
 * `agent.self` (agent principals only, plan 7A Decision 14a): the calling session's own default
 * branch and try list, which `fin` uses in place of `fin-config.json`.
 */
export const AgentSelfSuccess = Schema.Struct({
  sessionId: Id,
  projectId: Id,
  rootBranch: Schema.optionalKey(Schema.String),
  defaultBranch: Schema.String,
  tries: Schema.Array(SessionTry),
});

export const SelfTryAction = Schema.Literals(["record", "setDefault", "clear"]);
export type SelfTryAction = typeof SelfTryAction.Type;

/**
 * `agent.self.try`: `record` notes a try branch (it must be in the session's subtree) with the
 * default it would replace; `setDefault` makes a recorded (or the root) branch the default;
 * `clear` ends a try and, if it was the default, restores the default it replaced. `fin try`
 * records, then sets the default.
 */
export const AgentSelfTryPayload = Schema.Struct({ action: SelfTryAction, branch: Schema.String });
