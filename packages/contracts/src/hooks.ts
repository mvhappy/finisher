// Hooks (sub-project 7 spec §9; 7F plan Decisions 6 and 7): how a hook is configured, and how
// a configured hook is listed. The events, the request a hook reads and its reply are Rust
// (`fin_plugin_api::protocol::hook`), generated into `@finisher/core-types`; running hooks is
// the server's (`apps/finisherd/src/hooks/`).
import { Schema } from "effect";
import { HookEventSchema, HookReplySchema } from "./generated/schemas.ts";

/** An event a hook subscribes to (generated from `fin_plugin_api::protocol::hook::HookEvent`). */
export const HookEvent = HookEventSchema;
export type HookEvent = typeof HookEvent.Type;

/** A hook's reply (generated from `HookReply`): `allow`, `block {reason}` or `context {text}`. */
export const HookReply = HookReplySchema;
export type HookReply = typeof HookReply.Type;

const decodeReply = Schema.decodeUnknownExit(HookReplySchema);
/**
 * Decodes a hook's reply as Rust does (`deny_unknown_fields`): an unknown key, `modify` or a
 * missing field is refused. Every reply a command hook or plugin handler gives goes through
 * this, never through `HookReplySchema` alone (an Effect `Struct` drops unknown keys).
 */
export const decodeHookReply = (raw: unknown) => decodeReply(raw, { onExcessProperty: "error" });

/** The bounds of hooks, their processes and their replies (Decisions 2, 4, 6, 9, 10, 18). */
export const HOOK_LIMITS = {
  /** Hooks in one layer file. */
  perLayer: 64,
  /** Strings in `run` (the program and its arguments). */
  runArgs: 64,
  /** Bytes of one `run` string. */
  argBytes: 4096,
  /** Bytes of a `block` reason, after control characters are stripped. */
  reasonBytes: 1024,
  /** Bytes of one `context` text, after control characters are stripped. */
  textBytes: 16384,
  /** Bytes of an event's joined context. */
  contextBytes: 65536,
  /** Bytes a command hook may write on stdout. */
  stdoutBytes: 65536,
  /** Bytes of a payload sent as is (larger: `{payloadOmitted: true, bytes}`). */
  payloadBytes: 1048576,
  timeoutS: { default: 10, max: 60 },
  /** Hook processes at once per server. */
  processes: 8,
  /** Observe-only events waiting per server. */
  queue: 256,
} as const;

/**
 * How an event's hooks run (Decision 1; `HookEvent::class()` in Rust): a block refuses the
 * action (`blockingBefore`), stops the later handlers of a landed command (`blockingAfter`),
 * is ignored with a notice (`awaited`: waited for its context; `observe`: detached).
 */
export type HookClass = "blockingBefore" | "blockingAfter" | "awaited" | "observe";
export const HOOK_CLASSES: Readonly<Record<HookEvent, HookClass>> = {
  UserPrompt: "blockingBefore",
  PreCommand: "blockingBefore",
  PreMerge: "blockingBefore",
  BranchReady: "blockingBefore",
  PreRender: "blockingBefore",
  PostCommand: "blockingAfter",
  SessionStart: "awaited",
  SessionEnd: "observe",
  PostMerge: "observe",
  PostRender: "observe",
  JobDone: "observe",
  Approval: "observe",
  Notification: "observe",
};

/** Where a hook is configured, in the order its hooks run (spec §9.1). */
export const HookLayer = Schema.Literals(["managed", "user", "project", "local", "plugin"]);
export type HookLayer = typeof HookLayer.Type;

/**
 * The events whose `match` names something (Decision 6): the command names of `PreCommand` and
 * `PostCommand`, the job kind of `JobDone`, `status.<status>` or `budget.<kind>` of
 * `Notification`, the approval kind of `Approval`. Other events refuse a `match`.
 */
export const HOOK_MATCH_EVENTS: ReadonlySet<HookEvent> = new Set<HookEvent>(["PreCommand", "PostCommand", "JobDone", "Notification", "Approval"]);

/** A hook's id: a slug, unique in its layer file. */
export const HOOK_ID_PATTERN = /^[a-z0-9][a-z0-9-]{0,63}$/;

const utf8 = new TextEncoder();
const utf8Bytes = (s: string) => utf8.encode(s).length;

/** A string of `run`: at most `argBytes` bytes, no NUL (it cannot cross into a process's argv). */
const RunArg = Schema.String.check(
  Schema.makeFilter((s: string) => utf8Bytes(s) <= HOOK_LIMITS.argBytes && !s.includes("\u0000"), {
    expected: `a string of at most ${HOOK_LIMITS.argBytes} bytes, without NUL`,
  }),
);

/**
 * What a hook's `match` takes: a name glob of `RuleCommand`'s characters (`*` inside a segment,
 * `**` across them). Unlike a rule's command it may hold capitals, for statuses such as
 * `status.waitingApproval`.
 */
export const HookMatch = Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(200), Schema.isPattern(/^[A-Za-z0-9_.:@/*-]+$/));

const HookSpecFields = Schema.Struct({
  id: Schema.String.check(Schema.isPattern(HOOK_ID_PATTERN)),
  event: HookEvent,
  /** The program, then its arguments; never run through a shell. */
  run: Schema.Array(RunArg).check(Schema.isMinLength(1), Schema.isMaxLength(HOOK_LIMITS.runArgs)),
  /** The hook's own file (read-only to it), relative to the declaring file's folder unless absolute. */
  file: Schema.optionalKey(Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(4096))),
  match: Schema.optionalKey(HookMatch),
  /** Whose calls a `PreCommand`/`PostCommand` hook sees: agents' (the default) or everyone's. */
  principals: Schema.optionalKey(Schema.Literals(["agent", "all"])),
  timeoutS: Schema.optionalKey(Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(1), Schema.isLessThanOrEqualTo(HOOK_LIMITS.timeoutS.max))),
  /** What a timeout or any other failure answers; absent: `block` for `PreMerge`, else `allow`. */
  onTimeout: Schema.optionalKey(Schema.Literals(["allow", "block"])),
  /** The hook gets a `plan`-scoped credential of its own for `fin`. */
  fin: Schema.optionalKey(Schema.Literal(true)),
});

/** One configured hook (Decision 6). */
export const HookSpec = HookSpecFields.check(
  Schema.makeFilter((h: typeof HookSpecFields.Type) => h.run[0] !== "" && (h.match === undefined || HOOK_MATCH_EVENTS.has(h.event)), {
    expected: "a non-empty program, and a match only on an event that names something",
  }),
);
export type HookSpec = typeof HookSpec.Type;

const decodeSpec = Schema.decodeUnknownExit(HookSpec);
/** Decodes one hook, refusing unknown keys. */
export const decodeHookSpec = (raw: unknown) => decodeSpec(raw, { onExcessProperty: "error" });

/** A configured hook as `agent.hooks.list` shows it. */
export const ListedHook = Schema.Struct({
  layer: HookLayer,
  /** The file it came from, or the plugin's id for the `plugin` layer. */
  source: Schema.String,
  id: Schema.String,
  event: HookEvent,
  /** False while its layer does not apply (an untrusted or changed project file, a disabled plugin). */
  applies: Schema.Boolean,
  /** Accepted but never fired: `PreRender`/`PostRender` until a render job kind exists. */
  inert: Schema.optionalKey(Schema.Literal("noRenderKind")),
  plugin: Schema.optionalKey(Schema.String),
  /** A command hook's program and arguments, as configured (what a person trusts). */
  run: Schema.optionalKey(Schema.Array(Schema.String)),
  match: Schema.optionalKey(Schema.String),
  /** What a person weighs before trusting, as configured: its own file, everyone's calls, a `fin` credential, its failure answer. */
  file: Schema.optionalKey(Schema.String),
  principals: Schema.optionalKey(Schema.Literals(["agent", "all"])),
  fin: Schema.optionalKey(Schema.Literal(true)),
  timeoutS: Schema.optionalKey(Schema.Number),
  onTimeout: Schema.optionalKey(Schema.Literals(["allow", "block"])),
});
export type ListedHook = typeof ListedHook.Type;

/**
 * `agent.hooks.list` (a person's, `agent.admin`): every layer's hooks, those of a project's
 * file listed with `applies: false` while it does not apply. With a project: whether its
 * `finisher.hooks.json` applies, its hash now with the project files its hooks name (what
 * `agent.project.trust` binds through `expectedHooksHash`; absent when one cannot be read or a
 * path leaves the project) and why it does not apply as it is.
 */
export const AgentHooksListPayload = Schema.Struct({ projectId: Schema.optionalKey(Schema.String) });
export const AgentHooksListSuccess = Schema.Struct({
  hooks: Schema.Array(ListedHook),
  projectHooks: Schema.optionalKey(Schema.Literals(["none", "trusted", "untrusted", "changed"])),
  hooksHash: Schema.optionalKey(Schema.String.check(Schema.isPattern(/^(?:sha256:[0-9a-f]{64}|none)$/))),
  projectHooksReason: Schema.optionalKey(Schema.Literals(["deleted", "unreadable", "invalidHooks"])),
  /** The project files its hooks name (relative to the project), which `hooksHash` binds too. */
  projectHooksFiles: Schema.optionalKey(Schema.Array(Schema.String)),
});
export type AgentHooksListSuccess = typeof AgentHooksListSuccess.Type;
