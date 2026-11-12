// Errors a plugin raises and how a handler's failure becomes the job's `error` (spec §13's
// `{code, message, details, fix?}`; the envelope's `JobError` has no `fix`, so it travels in
// `details.fix`).
//
// Mapping of whatever a command handler throws:
// - a `PluginError`, or any object with a string `code` and `message` that is not one of
//   Node's own errors, keeps its code, message and details;
// - Node's permission model (`ERR_ACCESS_DENIED`, a `--permission` process touching what it
//   was not granted) is `PLUGIN_PERMISSION_DENIED {perm, nodePermission}`, without the resource
//   (it is usually a path, and errors to non-human callers lose paths). `perm` is a manifest
//   permission name (`fs`, for Node's `FileSystem*` scopes) or `"unknown"`: Node's other scopes
//   (child processes, workers, addons, WASI) have no manifest permission that grants them;
//   `nodePermission` keeps Node's own scope name;
// - a cancelled job's failure is `JOB_CANCELLED` (the runtime decides that, not this module);
// - anything else is `PLUGIN_COMMAND_FAILED {name, nodeCode?}` with the error's message.
import type { JobError, PluginError as PluginErrorData } from "@finisher/core-types";

/** What a plugin throws to fail a command or what a failed host call rejects with. */
export class PluginError extends Error implements PluginErrorData {
  readonly code: string;
  readonly details: { [k: string]: unknown };
  readonly fix?: string | null;

  constructor(code: string, message: string, details: { [k: string]: unknown } = {}, fix?: string) {
    super(message);
    this.name = "PluginError";
    this.code = code;
    this.details = details;
    if (fix !== undefined) this.fix = fix;
  }

  /** The `JobError` this becomes on the wire (`fix` moves into `details`). */
  toJobError(): JobError {
    const details = this.fix == null ? this.details : { ...this.details, fix: this.fix };
    return Object.keys(details).length === 0 ? { code: this.code, message: this.message } : { code: this.code, message: this.message, details };
  }
}

/** Codes the SDK itself raises. */
export const codes = {
  /** The host sent something this SDK cannot serve: an unknown command or kind, a malformed run, inconsistent pages. */
  PLUGIN_PROTOCOL: "PLUGIN_PROTOCOL",
  PLUGIN_PERMISSION_DENIED: "PLUGIN_PERMISSION_DENIED",
  /** A handler threw something that is not a `PluginError`, or a job file's I/O failed (`{reason: io}`). */
  PLUGIN_COMMAND_FAILED: "PLUGIN_COMMAND_FAILED",
  /** A job's output file is not what the host created (`{reason: size|truncated|link|notRegular}`, spec §13). */
  PLUGIN_OUTPUT_INVALID: "PLUGIN_OUTPUT_INVALID",
  /** 5C's cancelled job (`fin_jobs::JobError::cancelled`). */
  JOB_CANCELLED: "JOB_CANCELLED",
} as const;

export const protocolError = (message: string, details: { [k: string]: unknown }): PluginError => new PluginError(codes.PLUGIN_PROTOCOL, message, details);

export const cancelledError = (): PluginError => new PluginError(codes.JOB_CANCELLED, "the job was cancelled");

/** Node's permission scope (`err.permission`) as a manifest permission name, or `"unknown"`. */
function permOf(nodePermission: string): "fs" | "unknown" {
  return nodePermission.startsWith("FileSystem") ? "fs" : "unknown";
}

const isObject = (e: unknown): e is Record<string, unknown> => typeof e === "object" && e !== null;

/** Whether `e` is one of Node's own errors (a system error or an `ERR_*` code). */
function isNodeError(e: Record<string, unknown>): boolean {
  const code = e["code"];
  return "errno" in e || "syscall" in e || (typeof code === "string" && code.startsWith("ERR_"));
}

/** What a handler's thrown value becomes as the job's error. */
export function toPluginError(e: unknown): PluginError {
  if (e instanceof PluginError) return e;
  if (isObject(e) && e["code"] === "ERR_ACCESS_DENIED") {
    const nodePermission = typeof e["permission"] === "string" ? e["permission"] : "unknown";
    return new PluginError(codes.PLUGIN_PERMISSION_DENIED, "the plugin's runtime refused an access it was not granted", { perm: permOf(nodePermission), nodePermission });
  }
  if (isObject(e) && typeof e["code"] === "string" && typeof e["message"] === "string" && !isNodeError(e)) {
    const details = isObject(e["details"]) ? (e["details"] as { [k: string]: unknown }) : {};
    const fix = typeof e["fix"] === "string" ? e["fix"] : undefined;
    return new PluginError(e["code"], e["message"], details, fix);
  }
  const name = e instanceof Error ? e.name : typeof e;
  const message = e instanceof Error ? e.message : String(e);
  const details: { [k: string]: unknown } = { name };
  if (isObject(e) && typeof e["code"] === "string") details["nodeCode"] = e["code"];
  return new PluginError(codes.PLUGIN_COMMAND_FAILED, message === "" ? "the command failed" : message, details);
}

/** A reply's `JobError` as a thrown `PluginError`. */
export function fromJobError(e: JobError): PluginError {
  const details = { ...(e.details ?? {}) };
  const fix = typeof details["fix"] === "string" ? details["fix"] : undefined;
  if (fix !== undefined) delete details["fix"];
  return new PluginError(e.code, e.message, details, fix);
}
