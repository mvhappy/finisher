// Diagnostics RPCs (plan D, Decision 32; spec §21.1; 5A Decisions 21, 22 and 27). Consent
// changes, purges, `project default` and content exports are human-only (`diagnostics.admin`);
// turning training on is a `device`'s alone (the app's onboarding question). The result types
// are the journal's generated ones (`@finisher/core-types`); the Effect schemas that carry
// them are `Schema.Unknown` narrowed to those types (runtime validation is Rust's job).
import { Schema } from "effect";
import { ChallengeField } from "./auth.ts";

export type {
  ConsentState,
  DiagnosticsReport,
  Envelope,
  ExportPage,
  Filter,
  JournalStatus,
  PurgeReport,
  PurgeSelector,
} from "@finisher/core-types";

/** The RPCs whose success is a stream: never timed into `rpc.stats` (a call's lifetime is not a latency). */
export const STREAM_RPCS: ReadonlyArray<string> = ["ops.subscribe", "project.events", "agent.watch"];

export const DiagnosticsStatusPayload = Schema.Struct({});
/** `DiagnosticsReport`: the consent, the journal's state and counts per kind and project. */
export const DiagnosticsStatusSuccess = Schema.Unknown;

/**
 * One page of the journal, oldest first. A caller that is not a person gets it redacted
 * (content fields and blob references removed, `redacted: true`); `content: true` asks for the
 * content and is human-only. `filter`: the journal's `Filter` (`since`, `until`,
 * `project`, `kinds`, `limit`: the newest n); `cursor`: the previous page's `next` (an opaque
 * position for paging one export, not an upload cursor); `limit` 1–1000 per page (default
 * 1000); `asUpload`: exactly what an uploader may ship now (closed files, current consent,
 * content only under training); `store`: which store. Answers `ExportPage {lines, next?}`.
 */
export const DiagnosticsExportPayload = Schema.Struct({
  filter: Schema.optional(Schema.Unknown),
  cursor: Schema.optional(Schema.String),
  limit: Schema.optional(Schema.Number),
  asUpload: Schema.optional(Schema.Literals(["diagnostics", "training"])),
  content: Schema.optional(Schema.Boolean),
  /** The diagnostics journal (default) or the agent-trace store (agent content; sp7 §12.1). */
  store: Schema.optional(Schema.Literals(["diagnostics", "agentTraces"])),
});
export const DiagnosticsExportSuccess = Schema.Unknown;

/**
 * `selector`: the journal's `PurgeSelector` (`"all"`, `{project: id}`, `{before: ms}`); answers
 * `PurgeReport`. Human-only, every selector.
 */
export const DiagnosticsPurgePayload = Schema.Struct({ selector: Schema.Unknown, challenge: ChallengeField });
export const DiagnosticsPurgeSuccess = Schema.Unknown;

/**
 * The project's collection flag in `finisher.project.json` (needs the writer lease); `never`
 * purges its entries and is open to every reader of diagnostics; `default` clears the flag,
 * which binds every collaborator, so it is human-only.
 */
export const DiagnosticsProjectPayload = Schema.Struct({
  projectId: Schema.String,
  collect: Schema.Literals(["default", "never"]),
  challenge: ChallengeField,
});
export const DiagnosticsProjectSuccess = Schema.Struct({
  project: Schema.String,
  collect: Schema.Literals(["default", "never"]),
});

/**
 * Sets one purpose in `settings.json` `privacy` (`unset` removes the key: training is then
 * `undecided` again) and answers the `ConsentState` in force, which says when an environment
 * override wins (`env`). Every change is human-only; `training: on` is refused
 * `CONSENT_UI_ONLY` for every principal but a `device` (checked first): opting in to content is
 * a person's choice, made in the app. `SETTINGS_INVALID` when `settings.json`
 * does not parse or the merged `privacy` section breaks the journal's rules (a hand edit is never
 * overwritten); `SETTINGS_WRITE_FAILED` when the write fails (the file is unchanged).
 */
export const DiagnosticsConsentPayload = Schema.Struct({
  purpose: Schema.Literals(["diagnostics", "training"]),
  value: Schema.Literals(["on", "off", "unset"]),
  challenge: ChallengeField,
});
export const DiagnosticsConsentSuccess = Schema.Unknown;

/**
 * An event from a client (`fin` and the MCP adapter: `cli.command`, `error.panic`; the console
 * and the UI: none yet), in the kind's wire form. The server sets `source` and `session` from
 * the connection's principal and the actor from it too, narrowed by the request headers
 * (`fin-actor-kind`, `fin-actor-session`, `fin-actor-call`: only `local` may claim a kind or a
 * session); `ctx` may name the project, branch and txId. Rate-limited per principal and capped
 * at 64 KiB; with recording off nothing is built. `EVENT_KIND_UNKNOWN`, `EVENT_INVALID`, `EVENT_TOO_LARGE`.
 */
export const DiagnosticsRecordPayload = Schema.Struct({
  kind: Schema.String,
  /** The kind's payload version (0–65535). */
  kv: Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: 65535 })),
  ctx: Schema.optional(
    Schema.Struct({
      project: Schema.optional(Schema.String),
      branch: Schema.optional(Schema.String),
      txId: Schema.optional(Schema.String),
    }),
  ),
  payload: Schema.Unknown,
});
/** `recorded`: false when it was dropped (recording off, the rate limit). */
export const DiagnosticsRecordSuccess = Schema.Struct({ recorded: Schema.Boolean });
