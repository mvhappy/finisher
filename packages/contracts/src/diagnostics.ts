// Diagnostics RPCs (plan D, Decision 32; spec §21.1), scope `diagnostics`. The result types
// are the journal's generated ones (`@finisher/core-types`); the Effect schemas that carry
// them are `Schema.Unknown` narrowed to those types (runtime validation is Rust's job).
import { Schema } from "effect";

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
export const STREAM_RPCS: ReadonlyArray<string> = ["ops.subscribe", "project.events"];

export const DiagnosticsStatusPayload = Schema.Struct({});
/** `DiagnosticsReport`: the consent, the journal's state and counts per kind and project. */
export const DiagnosticsStatusSuccess = Schema.Unknown;

/**
 * One page of the journal, oldest first. `filter`: the journal's `Filter` (`since`, `until`,
 * `project`, `kinds`, `limit`: the newest n); `cursor`: the previous page's `next` (an opaque
 * position for paging one export, not an upload cursor); `limit` 1–1000 per page (default
 * 1000); `asUpload`: exactly what an uploader may ship now (closed files, current consent,
 * content only under training). Answers `ExportPage {lines, next?}`.
 */
export const DiagnosticsExportPayload = Schema.Struct({
  filter: Schema.optional(Schema.Unknown),
  cursor: Schema.optional(Schema.String),
  limit: Schema.optional(Schema.Number),
  asUpload: Schema.optional(Schema.Literals(["diagnostics", "training"])),
});
export const DiagnosticsExportSuccess = Schema.Unknown;

/** `selector`: the journal's `PurgeSelector` (`"all"`, `{project: id}`, `{before: ms}`); answers `PurgeReport`. */
export const DiagnosticsPurgePayload = Schema.Struct({ selector: Schema.Unknown });
export const DiagnosticsPurgeSuccess = Schema.Unknown;

/** The project's collection flag in `finisher.project.json` (needs the writer lease); `never` purges its entries. */
export const DiagnosticsProjectPayload = Schema.Struct({
  projectId: Schema.String,
  collect: Schema.Literals(["default", "never"]),
});
export const DiagnosticsProjectSuccess = Schema.Struct({
  project: Schema.String,
  collect: Schema.Literals(["default", "never"]),
});

/**
 * Sets one purpose in `settings.json` `privacy` (`unset` removes the key: training is then
 * `undecided` again) and answers the `ConsentState` in force, which says when an environment
 * override wins (`env`). `training: on` is refused `CONSENT_UI_ONLY` unless the connection is
 * the UI's: opting in to content is a person's choice. `SETTINGS_INVALID` when `settings.json`
 * does not parse or the merged `privacy` section breaks the journal's rules (a hand edit is never
 * overwritten); `SETTINGS_WRITE_FAILED` when the write fails (the file is unchanged).
 */
export const DiagnosticsConsentPayload = Schema.Struct({
  purpose: Schema.Literals(["diagnostics", "training"]),
  value: Schema.Literals(["on", "off", "unset"]),
});
export const DiagnosticsConsentSuccess = Schema.Unknown;

/**
 * An event from a client (`fin`: `cli.command`, `error.panic`; the UI: none yet), in the
 * kind's wire form. The server sets `source` and `session` from the connection and the actor
 * from the request headers (`fin-actor-kind`, `fin-actor-session`, `fin-actor-call`); `ctx`
 * may name the project, branch and txId. Rate-limited per source and capped at 64 KiB; with
 * recording off nothing is built. `EVENT_KIND_UNKNOWN`, `EVENT_INVALID`, `EVENT_TOO_LARGE`.
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
