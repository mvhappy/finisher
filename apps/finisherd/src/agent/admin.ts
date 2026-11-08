// The owner's agent settings behind `agent.key.*`, `agent.local.*`, `training.closedTargets.*`
// and the facts of `agent.providers` (plan 7A Decisions 24, 28, Task 12; sub-project 7 §10).
//
// - **Keys** go to the secret store only (`secrets.json`, 0600) and come back as metadata only.
//   v1 stores the OpenRouter key alone: `local` needs none (refused with a fix pointing at
//   `provider local set`), any other provider is `PROVIDER_UNAVAILABLE`.
// - **The local endpoint** is a setting (`agents.local`), validated (`http`/`https`, a host, no
//   credentials). Audit rows and answers other than `agent.local.show` (the owner's own) say
//   `local` and the preset, never the host or port.
// - **The counsel gate** (`training.closedModelTargets`) is recorded by a person and audited.
// - Every change is audited as `settings.write` with words only (`rpc`, `value`, `projectId`).
import { type AgentWarning, type ClosedModelTargets, type Harness, type Principal, type ProviderInfo, type ProviderKey } from "@finisher/contracts";
import { Effect } from "effect";
import { finError } from "../errors.ts";
import type { SecretStore } from "../secrets/store.ts";
import type { ProviderAdapter } from "./adapter.ts";
import { closedTargets, type LocalEndpoint, localEndpoint, parseEndpoint, setClosedTargets, setLocalEndpoint } from "./settings.ts";
import { agentWarnings } from "./warnings.ts";

/** What an `ok` `settings.write` row says (words, a project id; never a key, URL or host). */
export type AdminAudit = (by: Principal, details: { rpc: string; value?: string; projectId?: string }) => void;

export interface AdminDeps {
  readonly serverData: string;
  /** Undefined where the file backend cannot run (Windows before 5B's keychain). */
  readonly secrets: SecretStore | undefined;
  /** The catalogue's loopback defaults for `ollama` and `lmstudio`. */
  readonly localPresets: Readonly<Record<string, string>>;
  readonly audit: AdminAudit;
  /** `GET /models` of the local server (tests: the fake model server). */
  readonly fetch?: typeof fetch;
  /** How long `agent.local.test` and `agent.providers` wait for the local server (ms). */
  readonly probeMs?: number;
}

/** What an API key looks like (the console checks the same before it asks for a confirmation). */
export const KEY_RULE = /^[\x21-\x7e]{8,1024}$/;
export const KEY_RULE_TEXT = "an API key is 8–1024 printable characters without spaces";

const keyInReach = (by: Principal, k: { projectId?: string }) => by.projects === "*" || (k.projectId !== undefined && by.projects.includes(k.projectId));

const storeUnavailable = () =>
  finError("SECRET_STORE_UNAVAILABLE", "this host has no secret store Finisher can use yet", { platform: process.platform }, "the keychain backend arrives with sub-project 5B");

/** The models a local OpenAI-compatible server lists (`GET <base>/models`); no redirect is followed. */
export async function listLocalModels(baseUrl: string, fetchImpl: typeof fetch = fetch, timeoutMs = 3000): Promise<string[]> {
  const res = await fetchImpl(`${baseUrl.replace(/\/+$/, "")}/models`, {
    headers: { accept: "application/json" },
    redirect: "manual",
    signal: AbortSignal.timeout(timeoutMs),
  });
  if (!res.ok) throw finError("PROVIDER_UNAVAILABLE", `the local model server answered ${res.status}`, { provider: "local", status: res.status });
  const body = (await res.json()) as { data?: unknown; models?: unknown };
  const list = Array.isArray(body?.data) ? body.data : Array.isArray(body?.models) ? body.models : [];
  return list
    .map((m) => (m as { id?: unknown; name?: unknown } | null)?.id ?? (m as { name?: unknown } | null)?.name)
    .filter((id): id is string => typeof id === "string" && id.length > 0 && id.length <= 200)
    .slice(0, 500);
}

/** Why the local server did not answer: the cause's words, never the address it tried. */
const unreachable = (e: unknown) => ({
  code: "PROVIDER_UNAVAILABLE",
  message: (e as { code?: unknown } | null)?.code === "PROVIDER_UNAVAILABLE" ? (e as Error).message : "the local model server does not answer",
  fix: "start the model server, or check the endpoint with `finisherd provider local show`",
});

export class AgentAdmin {
  readonly deps: AdminDeps;

  constructor(deps: AdminDeps) {
    this.deps = deps;
  }

  /** Whether an OpenRouter key would be used for `projectId`: its own, else the server's default. */
  hasKey(projectId?: string): boolean {
    return this.#keys().some((k) => k.projectId === undefined || (projectId !== undefined && k.projectId === projectId));
  }

  #keys(): ProviderKey[] {
    try {
      return this.deps.secrets?.list().filter((k) => k.provider === "openrouter") ?? [];
    } catch {
      return [];
    }
  }

  local(): LocalEndpoint | undefined {
    return localEndpoint(this.deps.serverData);
  }

  // Keys -------------------------------------------------------------------------------------

  async keySet(by: Principal, req: { provider: string; key: string; label?: string; projectId?: string }): Promise<ProviderKey> {
    if (req.provider === "local") {
      throw finError("PROVIDER_UNAVAILABLE", "a local model server needs no key", { provider: "local" }, "set its address with `finisherd provider local set <ollama|lmstudio|url>`");
    }
    if (req.provider !== "openrouter") {
      throw finError("PROVIDER_UNAVAILABLE", `Finisher stores no ${req.provider} key in v1`, { provider: req.provider }, "only an OpenRouter key can be set; a CLI agent uses its own sign-in");
    }
    const secrets = this.deps.secrets ?? (() => {
      throw storeUnavailable();
    })();
    // The server's default key spends for every project: only a caller who reaches them all may set it.
    if (req.projectId === undefined && by.projects !== "*") {
      throw finError("SCOPE_DENIED", "the server's default key is set by a caller who reaches every project", { reason: "serverDefaultKey" }, "give the key to one project (`--project`), or set it as the owner");
    }
    // Checked here, never by the schema: its refusal would quote the key back. Trimmed first,
    // then the console's rule, so a key the console would refuse is refused on the wire too.
    const key = req.key.trim();
    if (!KEY_RULE.test(key)) throw finError("SECRET_INVALID", KEY_RULE_TEXT, { field: "key" }, "copy the key again from OpenRouter's keys page");
    const label = req.label ?? (req.projectId === undefined ? "server default" : `project ${req.projectId}`);
    const value = Buffer.from(key, "utf8");
    let id: string;
    try {
      id = await secrets.put({ provider: "openrouter", label, ...(req.projectId === undefined ? {} : { projectId: req.projectId }), value });
    } finally {
      value.fill(0);
    }
    this.deps.audit(by, { rpc: "agent.key.set", value: "openrouter", ...(req.projectId === undefined ? {} : { projectId: req.projectId }) });
    return secrets.list().find((k) => k.id === id)!;
  }

  /** The keys `by` reaches: a project's key when its project is in reach, the server's default only for `*`. */
  keyList(by: Principal): ProviderKey[] {
    if (this.deps.secrets === undefined) throw storeUnavailable();
    return this.deps.secrets.list().filter((k) => keyInReach(by, k));
  }

  async keyRemove(by: Principal, id: string): Promise<boolean> {
    if (this.deps.secrets === undefined) throw storeUnavailable();
    // A key out of reach is answered as a missing one.
    if (!this.deps.secrets.list().some((k) => k.id === id && keyInReach(by, k))) return false;
    await this.deps.secrets.remove(id);
    this.deps.audit(by, { rpc: "agent.key.remove", value: "openrouter" });
    return true;
  }

  // The local endpoint ------------------------------------------------------------------------

  localSet(by: Principal, endpoint: string): { preset: LocalEndpoint["preset"]; warnings: AgentWarning[] } {
    const { local, warnings } = parseEndpoint(endpoint, this.deps.localPresets);
    setLocalEndpoint(this.deps.serverData, local);
    this.deps.audit(by, { rpc: "agent.local.set", value: local.preset });
    return { preset: local.preset, warnings };
  }

  localShow(): { configured: boolean; preset?: LocalEndpoint["preset"]; baseUrl?: string } {
    const l = this.local();
    return l === undefined ? { configured: false } : { configured: true, preset: l.preset, baseUrl: l.baseUrl };
  }

  async localTest(): Promise<{ configured: boolean; reachable: boolean; models?: string[]; problem?: { code: string; message: string; fix?: string } }> {
    const l = this.local();
    if (l === undefined) {
      return { configured: false, reachable: false, problem: { code: "PROVIDER_UNAVAILABLE", message: "no local model endpoint is set", fix: "set one with `finisherd provider local set <ollama|lmstudio|url>`" } };
    }
    try {
      return { configured: true, reachable: true, models: await listLocalModels(l.baseUrl, this.deps.fetch, this.deps.probeMs) };
    } catch (e) {
      return { configured: true, reachable: false, problem: unreachable(e) };
    }
  }

  localRemove(by: Principal): boolean {
    if (this.local() === undefined) return false;
    setLocalEndpoint(this.deps.serverData, null);
    this.deps.audit(by, { rpc: "agent.local.remove", value: "local" });
    return true;
  }

  // The counsel gate -------------------------------------------------------------------------

  closedTargets(): ClosedModelTargets {
    return closedTargets(this.deps.serverData);
  }

  setClosedTargets(by: Principal, state: ClosedModelTargets): ClosedModelTargets {
    setClosedTargets(this.deps.serverData, state);
    this.deps.audit(by, { rpc: "training.closedTargets.set", value: state });
    return this.closedTargets();
  }

  // `agent.providers` ------------------------------------------------------------------------

  /**
   * The harnesses as detection finds them, whether an OpenRouter key is set (never the key),
   * the local endpoint's preset and whether it answers (never its address), and the warnings.
   */
  providers(adapters: ReadonlyMap<Harness, ProviderAdapter>, inReach: (projectId: string) => boolean): Effect.Effect<{
    providers: ProviderInfo[];
    openrouter: { keySet: boolean; projects: string[] };
    local: { configured: boolean; preset?: LocalEndpoint["preset"]; reachable?: boolean };
    warnings: AgentWarning[];
  }> {
    return Effect.promise(async () => {
      const providers: ProviderInfo[] = [];
      for (const [harness, adapter] of adapters) {
        const exit = await Effect.runPromiseExit(adapter.detect());
        if (exit._tag === "Success") {
          const d = exit.value;
          providers.push({
            harness,
            detected: d.installed,
            ...(d.version === null ? {} : { version: d.version }),
            supported: d.installed,
            // The built-in agent "signs in" with a key or a local endpoint.
            signedIn: harness === "builtin" ? this.#keys().length > 0 || this.local() !== undefined : d.signedIn,
            capabilities: d.capabilities,
          });
        } else {
          const e = exit.cause.reasons.map((r) => (r._tag === "Fail" ? r.error : undefined)).find((x) => x !== undefined);
          providers.push({
            harness,
            detected: false,
            capabilities: { steer: false, rollback: false, resume: false, reasoning: false, images: false, parallelTools: false, costReported: false, exactCapture: false, nativeSubagents: false },
            problem: { code: e?.code ?? "INTERNAL", message: e?.message ?? "detection failed" },
          });
        }
      }
      const l = this.local();
      let reachable: boolean | undefined;
      if (l !== undefined) reachable = await listLocalModels(l.baseUrl, this.deps.fetch, this.deps.probeMs).then(() => true, () => false);
      return {
        providers,
        // The server's default, and the projects in the caller's reach with a key of their own.
        openrouter: {
          keySet: this.#keys().some((k) => k.projectId === undefined),
          projects: [...new Set(this.#keys().flatMap((k) => (k.projectId !== undefined && inReach(k.projectId) ? [k.projectId] : [])))].sort(),
        },
        local: l === undefined ? { configured: false } : { configured: true, preset: l.preset, reachable: reachable ?? false },
        warnings: agentWarnings(),
      };
    });
  }
}
