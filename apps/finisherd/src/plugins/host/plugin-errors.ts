// A plugin's own error (its `t: error` answer) cannot speak for the host (sp8b review of Task
// 18): an agent reads a host code such as `CONFLICT`, `CREDENTIAL_REVOKED` or
// `PLUGIN_AUTHORITY_RETIRED` as the host's verdict and follows its `fix` as the next action.
// So a plugin keeps its code only when it is one a plugin may legitimately pass on (the SDKs'
// contract and the broker's refusals it re-throws, from `fin-plugin-api::protocol::errors::codes`
// without the host's lifecycle, authority, reach and transaction verdicts); anything else is
// `PLUGIN_COMMAND_FAILED {cause: plugin}` with nothing of the plugin's. A kept error is marked
// `details.source: "plugin"`, and its message and every string in its details (the SDKs' `fix`
// included) are scrubbed like the plugin's log text. Host-made errors (a crash, a hang, a broken
// envelope: `mapPluginError`) never pass through here.
import type { JobError } from "@finisher/core-types";
import { scrubLogText } from "./log.ts";

/** The codes a plugin may answer with (each is also the host's, with the same meaning). */
export const PLUGIN_ANSWER_CODES: ReadonlySet<string> = new Set([
  // The SDKs' cross-SDK contract (Tasks 12, 13).
  "PLUGIN_COMMAND_ARGS",
  "PLUGIN_PROTOCOL",
  "PLUGIN_OUTPUT_INVALID",
  "PLUGIN_COMMAND_FAILED",
  "PLUGIN_PERMISSION_DENIED",
  // The egress broker's refusals, re-thrown by a plugin (Task 8).
  "EGRESS_DENIED",
  "EGRESS_NOT_HTTPS",
  "EGRESS_PRIVATE_ADDRESS",
  "EGRESS_TOO_LARGE",
  "EGRESS_TIMEOUT",
  "EGRESS_RATE_LIMITED",
  "EGRESS_UNREACHABLE",
  "SECRET_MISSING",
]);

/** Strings in a plugin's details, scrubbed; at most 4 levels and 64 entries per level. */
function scrubbed(v: unknown, depth = 0): unknown {
  if (typeof v === "string") return scrubLogText(v);
  if (v === null || typeof v !== "object" || depth >= 4) return typeof v === "object" && v !== null ? null : v;
  if (Array.isArray(v)) return v.slice(0, 64).map((x) => scrubbed(x, depth + 1));
  const out: Record<string, unknown> = {};
  for (const [k, x] of Object.entries(v).slice(0, 64)) out[scrubLogText(k, 64)] = scrubbed(x, depth + 1);
  return out;
}

/** The plugin's `t: error` answer as the host accepts it (`FamilySpec.workerError`). */
export function pluginAnswerError(e: JobError): JobError {
  const retry = e.retry === undefined ? {} : { retry: e.retry };
  // After a cancel the SDKs answer `JOB_CANCELLED` (their contract): the host's words, not theirs.
  if (e.code === "JOB_CANCELLED") return { code: "JOB_CANCELLED", message: "the job was cancelled", details: {}, ...retry };
  if (!PLUGIN_ANSWER_CODES.has(e.code)) return { code: "PLUGIN_COMMAND_FAILED", message: "the plugin's job failed", details: { cause: "plugin" }, ...retry };
  const given = e.details !== null && typeof e.details === "object" && !Array.isArray(e.details) ? (scrubbed(e.details) as Record<string, unknown>) : {};
  return { code: e.code, message: scrubLogText(String(e.message)), details: { ...given, source: "plugin" }, ...retry };
}
