// Hooks (sub-project 7 spec §9; 7F plan Decisions 6 and 7): how a hook is configured, and how
// a configured hook is listed. The events, the request a hook reads and its reply are Rust
// (`fin_plugin_api::protocol::hook`), generated into `@finisher/core-types`; running hooks is
// the server's (`apps/finisherd/src/hooks/`).
import { Schema } from "effect";
import { HookEventSchema } from "./generated/schemas.ts";

/** An event a hook subscribes to (generated from `fin_plugin_api::protocol::hook::HookEvent`). */
export const HookEvent = HookEventSchema;
export type HookEvent = typeof HookEvent.Type;

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
});
export type ListedHook = typeof ListedHook.Type;
