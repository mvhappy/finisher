// The owner's agent settings in `<server-data>/settings.json` (sub-project 7 §4.4, §4.7, §10,
// §12.7; plan 7A Decisions 8, 11, 19, 24, 28). finisherd writes only the keys it owns
// (`agents.local`, `training.closedModelTargets`, through `patchSection`) and reads:
//
// - `agents.models`: models the owner describes (`ExtraModel`), read when the server starts;
// - `agents.local`: `{preset: ollama|lmstudio|custom, baseUrl}`, the local model endpoint, read
//   at each use. Set only by the owner (`agent.local.set`, human-only); no record holds its
//   host or port: records say `local`;
// - `agents.finPath`: the `fin` the built-in tools run (default: beside the bundled binaries,
//   else `target/release|debug/fin` in a checkout);
// - `agents.retentionDays` (1–3650, default 90) and `agents.toolTimeoutS` (1–3600, default 120);
// - `training.closedModelTargets`: the counsel gate, default `pendingCounsel` (an unusable value
//   reads as the default: it never clears the gate by accident).
//
// An unusable file or section reads as the defaults with `error` set (the server logs it);
// nothing here throws on read.
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { AgentWarning, ClosedModelTargets } from "@finisher/contracts";
import { workersBinaryPath } from "@finisher/fin-node";
import { settingsPath } from "../config.ts";
import { finError } from "../errors.ts";
import { patchSection } from "../settings-write.ts";
import { checkBaseUrl } from "./builtin/transport.ts";
import type { ExtraModel } from "./catalogue/catalogue.ts";

export type LocalPreset = "ollama" | "lmstudio" | "custom";

export interface LocalEndpoint {
  readonly preset: LocalPreset;
  readonly baseUrl: string;
}

export interface AgentSettings {
  readonly models: readonly ExtraModel[];
  readonly local?: LocalEndpoint;
  readonly finPath?: string;
  readonly retentionDays?: number;
  readonly toolTimeoutS?: number;
}

export const CLOSED_TARGETS_DEFAULT: ClosedModelTargets = "pendingCounsel";
const CLOSED_TARGETS = new Set<string>(["pendingCounsel", "cleared", "excluded"]);

const isObject = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);
const wholeIn = (v: unknown, lo: number, hi: number): v is number => typeof v === "number" && Number.isSafeInteger(v) && v >= lo && v <= hi;

/** `settings.json` as an object: `{}` when absent, `error` when it cannot be used. */
function readRaw(serverData: string): { raw: Record<string, unknown>; error?: string } {
  let text: string;
  try {
    text = readFileSync(settingsPath(serverData), "utf8");
  } catch (e) {
    return (e as { code?: unknown }).code === "ENOENT" ? { raw: {} } : { raw: {}, error: String(e) };
  }
  try {
    const raw = JSON.parse(text) as unknown;
    return isObject(raw) ? { raw } : { raw: {}, error: "settings must be a JSON object" };
  } catch (e) {
    return { raw: {}, error: (e as Error).message };
  }
}

function readLocal(v: unknown): LocalEndpoint | undefined {
  if (!isObject(v) || typeof v.baseUrl !== "string") return undefined;
  const preset = v.preset === "ollama" || v.preset === "lmstudio" ? v.preset : "custom";
  try {
    checkBaseUrl("local", v.baseUrl);
  } catch {
    return undefined;
  }
  return { preset, baseUrl: v.baseUrl };
}

/** The `agents` section, each key checked on its own (a bad key is dropped, with `error`). */
export function readAgentSettings(serverData: string): { settings: AgentSettings; error?: string } {
  const { raw, error } = readRaw(serverData);
  if (error !== undefined) return { settings: { models: [] }, error };
  const agents = raw.agents;
  if (agents === undefined) return { settings: { models: [] } };
  if (!isObject(agents)) return { settings: { models: [] }, error: "agents must be an object" };
  const problems: string[] = [];
  const models = agents.models === undefined ? [] : Array.isArray(agents.models) ? (agents.models as ExtraModel[]) : (problems.push("agents.models must be a list"), []);
  const local = agents.local === undefined ? undefined : readLocal(agents.local);
  if (agents.local !== undefined && local === undefined) problems.push("agents.local must be {preset, baseUrl} with an http(s) URL");
  const finPath = typeof agents.finPath === "string" && agents.finPath !== "" ? agents.finPath : undefined;
  if (agents.finPath !== undefined && finPath === undefined) problems.push("agents.finPath must be a path");
  const retentionDays = wholeIn(agents.retentionDays, 1, 3650) ? agents.retentionDays : undefined;
  if (agents.retentionDays !== undefined && retentionDays === undefined) problems.push("agents.retentionDays must be 1–3650");
  const toolTimeoutS = wholeIn(agents.toolTimeoutS, 1, 3600) ? agents.toolTimeoutS : undefined;
  if (agents.toolTimeoutS !== undefined && toolTimeoutS === undefined) problems.push("agents.toolTimeoutS must be 1–3600");
  const settings: AgentSettings = {
    models,
    ...(local === undefined ? {} : { local }),
    ...(finPath === undefined ? {} : { finPath }),
    ...(retentionDays === undefined ? {} : { retentionDays }),
    ...(toolTimeoutS === undefined ? {} : { toolTimeoutS }),
  };
  return problems.length === 0 ? { settings } : { settings, error: problems.join("; ") };
}

/** The local endpoint now (re-read at each use: the owner may set it while the server runs). */
export function localEndpoint(serverData: string): LocalEndpoint | undefined {
  return readAgentSettings(serverData).settings.local;
}

/** The counsel gate (Decision 28): `pendingCounsel` unless a person recorded another state. */
export function closedTargets(serverData: string): ClosedModelTargets {
  const training = readRaw(serverData).raw.training;
  const v = isObject(training) ? training.closedModelTargets : undefined;
  return typeof v === "string" && CLOSED_TARGETS.has(v) ? (v as ClosedModelTargets) : CLOSED_TARGETS_DEFAULT;
}

/** Records the counsel gate; `pendingCounsel` removes the key (the default). */
export function setClosedTargets(serverData: string, state: ClosedModelTargets): void {
  patchSection(serverData, "training", { closedModelTargets: state === CLOSED_TARGETS_DEFAULT ? null : state });
}

/**
 * What `agent.local.set` accepts: `ollama` or `lmstudio` (the catalogue's loopback default for
 * it) or a base URL; `PROVIDER_UNAVAILABLE` for anything else. Answers the endpoint and the
 * owner's warnings (plain `http` to another machine).
 */
export function parseEndpoint(endpoint: string, presets: Readonly<Record<string, string>>): { local: LocalEndpoint; warnings: AgentWarning[] } {
  const named = endpoint === "ollama" || endpoint === "lmstudio" ? endpoint : undefined;
  const baseUrl = named === undefined ? endpoint.trim() : presets[named];
  if (baseUrl === undefined) throw finError("PROVIDER_UNAVAILABLE", `no default address is known for ${endpoint}`, { preset: endpoint }, "give the server's base URL instead");
  if (named === undefined && !/^https?:\/\//i.test(baseUrl)) {
    throw finError("PROVIDER_UNAVAILABLE", "the local endpoint is ollama, lmstudio or an http(s) base URL", { reason: "notUrl" }, "for example `finisherd provider local set http://127.0.0.1:11434/v1`");
  }
  const { url, warnings } = checkBaseUrl("local", baseUrl);
  if (url.hostname === "") throw finError("PROVIDER_UNAVAILABLE", "the local endpoint's URL has no host", { reason: "noHost" }, "give a URL with a host");
  return {
    local: { preset: named ?? "custom", baseUrl: url.href.replace(/\/+$/, "") },
    warnings: warnings.map((message) => ({
      code: "LOCAL_ENDPOINT_UNENCRYPTED",
      message,
      details: { preset: named ?? "custom" },
      fix: "use https, or a model server on this machine",
    })),
  };
}

/** Writes `agents.local` (`null` removes it). */
export function setLocalEndpoint(serverData: string, local: LocalEndpoint | null): void {
  patchSection(serverData, "agents", { local: local === null ? null : { preset: local.preset, baseUrl: local.baseUrl } });
}

/** `fin[.exe]` in `dir`. */
const finIn = (dir: string) => join(dir, process.platform === "win32" ? "fin.exe" : "fin");

/**
 * The `fin` the built-in tools run: `agents.finPath`, else the one beside the bundled binaries
 * (fin-node's directory), else a checkout's `target/release/fin` or `target/debug/fin`. The
 * last candidate is answered even when missing: the provider reports it not installed.
 */
export function resolveFinPath(settings: AgentSettings): string {
  if (settings.finPath !== undefined) return settings.finPath;
  const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");
  const candidates = [finIn(dirname(workersBinaryPath())), finIn(join(repo, "target", "release")), finIn(join(repo, "target", "debug"))];
  return candidates.find((p) => existsSync(p)) ?? candidates[candidates.length - 1]!;
}
