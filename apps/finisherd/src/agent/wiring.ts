// The agent runtime as `main.ts` builds it (plan 7A Task 12; the 6b, 8b, 10 and 6b-fix notes):
// after the core and the project registry, before the transports serve.
//
// - Settings: `agents.*` (`settings.ts`); the catalogue with the owner's `agents.models` and
//   OpenRouter's cached listing (`<server-data>/agents/openrouter-models.json`), priced through
//   `cataloguePricing` (a non-local model without a price cannot start).
// - Secrets: the file `SecretStore` (none on Windows before 5B) behind `ModelGateway`, whose
//   grants and refusals are audited as `grant`.
// - Adapters: the built-in loop (`BuiltinAdapter` with both presets' clients, `nodes` for resume,
//   `index` for its write/read split) and its tools under `tools.builtin`; the scripted `fake`
//   harness only in a test-hooks build run with `FIN_TEST_HOOKS=1`.
// - The runtime gets the core's journal (Task 10) and boots (crash recovery, the outbox, the
//   credential reconcile, then its timers) before the transports serve; it closes on shutdown.
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import type { Harness, Node } from "@finisher/contracts";
import { testHooks } from "@finisher/fin-node";
import { Context, Effect, type Scope } from "effect";
import { childEnv } from "../env.ts";
import { finError } from "../errors.ts";
import type { Audit } from "../control/audit.ts";
import { ModelGateway } from "../secrets/gateway.ts";
import { FileSecretStore, type SecretStore } from "../secrets/store.ts";
import type { ProviderAdapter } from "./adapter.ts";
import { AgentAdmin, listLocalModels } from "./admin.ts";
import { builtinEnvironment } from "./builtin/environment.ts";
import { localClient, type ModelClient, openrouterClient } from "./builtin/compat.ts";
import { finIndex } from "./builtin/fin-tool.ts";
import { BuiltinAdapter } from "./builtin/loop.ts";
import { builtinTools } from "./builtin/tools.ts";
import { BUNDLED_CATALOGUE, Catalogue } from "./catalogue/catalogue.ts";
import { OpenRouterPrices } from "./catalogue/prices.ts";
import type { AgentCore } from "./core.ts";
import type { BranchRegistry } from "./credentials.ts";
import { FakeAdapter, type FakeScript } from "./fake.ts";
import { sessionPlanner } from "./planner.ts";
import { BUNDLED_ROLES, type Role } from "./roles.ts";
import { Runtime } from "./runtime.ts";
import { cataloguePricing, type SessionEnvironment } from "./runtime-deps.ts";
import { readAgentSettings, resolveFinPath } from "./settings.ts";
import type { AgentStore } from "./store.ts";
import { AgentLookup } from "./store-lookup.ts";

/** `serve`'s agent options (tests). */
export interface AgentServeOptions {
  /** The `fake` harness's scripts: used only when the fake is allowed (`fakeAllowed`). */
  readonly fakeScripts?: readonly FakeScript[];
  /**
   * Every request the wiring makes itself: the local server's `GET /models` and OpenRouter's
   * model list (tests: a loopback-only fetch, so no test reaches the network).
   */
  readonly fetch?: typeof fetch;
  readonly roles?: ReadonlyMap<string, Role>;
  /** The runtime's hourly sweep and budget check (default on). */
  readonly timers?: boolean;
}

/** What the handlers and the console RPCs use. */
export interface AgentRuntimeParts {
  readonly runtime: Runtime;
  readonly admin: AgentAdmin;
  readonly adapters: ReadonlyMap<Harness, ProviderAdapter>;
  readonly lookup: AgentLookup;
  readonly catalogue: Catalogue;
}

export class AgentRuntimeTag extends Context.Service<AgentRuntimeTag, AgentRuntimeParts>()("finisherd/AgentRuntime") {}

/** The scripted harness exists only in a test-hooks build run with `FIN_TEST_HOOKS=1` (check-test-hooks). */
export function fakeAllowed(env: NodeJS.ProcessEnv = process.env): boolean {
  return env.FIN_TEST_HOOKS === "1" && testHooks !== undefined;
}

/** A plain turn for the fake when a test names no script: one message, then the end of the turn. */
const DEFAULT_FAKE: readonly FakeScript[] = [{ name: "echo", turns: [[{ emit: [{ type: "message", role: "assistant", blocks: [{ type: "text", text: "ok" }] }, { type: "turnCompleted", stopReason: "endTurn" }] }]] }];

export interface AgentWiringDeps {
  readonly serverData: string;
  readonly core: AgentCore;
  readonly registry: BranchRegistry & { readonly dirOf: (projectId: string) => string | undefined };
  /** `control.db` (the store's), for the lookups `AgentStore` does not offer. */
  readonly db: ConstructorParameters<typeof AgentLookup>[0];
  readonly audit: Audit;
  /** `FIN_SERVER` for the built-in `fin`: the local socket until 7C's agent listener. */
  readonly endpoint: string;
  readonly log: (message: string) => void;
  readonly options?: AgentServeOptions;
}

/** Every stored node of a session, in order (the built-in loop rebuilds its history on resume). */
function allNodes(store: AgentStore, sessionId: string): Node[] {
  const out: Node[] = [];
  for (let from = 1; ; ) {
    const page = store.nodes(sessionId, from, 1000);
    out.push(...page);
    if (page.length < 1000) return out;
    from = page[page.length - 1]!.seq + 1;
  }
}

function openSecrets(serverData: string, log: (m: string) => void): SecretStore | undefined {
  try {
    return new FileSecretStore(serverData);
  } catch (e) {
    log(`agent keys unavailable: ${(e as Error).message}`);
    return undefined;
  }
}

export const buildAgentRuntime = (d: AgentWiringDeps): Effect.Effect<AgentRuntimeParts, never, Scope.Scope> =>
  Effect.gen(function* () {
    const { core, serverData, log } = d;
    const roles = d.options?.roles ?? BUNDLED_ROLES;
    const read = readAgentSettings(serverData);
    if (read.error !== undefined) log(`settings.json agents: ${read.error} (defaults used)`);
    const settings = read.settings;
    const prices = new OpenRouterPrices(join(serverData, "agents", "openrouter-models.json"), core.clock);
    let catalogue: Catalogue;
    try {
      catalogue = Catalogue.load(BUNDLED_CATALOGUE, { extra: settings.models, prices });
    } catch (e) {
      log(`agents.models is not used: ${(e as Error).message}`);
      catalogue = Catalogue.load(BUNDLED_CATALOGUE, { prices });
    }
    const secrets = openSecrets(serverData, log);
    const admin = new AgentAdmin({
      serverData,
      secrets,
      localPresets: catalogue.localPresets,
      ...(d.options?.fetch === undefined ? {} : { fetch: d.options.fetch }),
      audit: (by, details) => d.audit.append({ kind: "settings.write", principal: { kind: by.kind, id: by.id }, outcome: "ok", details }),
    });
    const gateway =
      secrets === undefined
        ? undefined
        : new ModelGateway(
            secrets,
            { isRunning: (id) => core.store.get(id)?.status === "running" },
            {
              record: (g) =>
                d.audit.append({
                  kind: "grant",
                  principal: { kind: "agent", id: g.sessionId },
                  outcome: g.outcome === "granted" ? "ok" : "denied",
                  // The key's scope (`project` or `server`), never the key.
                  details: { projectId: g.projectId, credentialId: g.sessionId, ...(g.reason === undefined ? {} : { reason: g.reason }), ...(g.scope === undefined ? {} : { value: g.scope }) },
                }),
            },
            core.clock,
          );
    const finPath = resolveFinPath(settings);
    const cachingFor = (model: string) => catalogue.describe("openrouter", model).capabilities.caching;
    const openrouter = openrouterClient({ cachingFor, clock: core.clock });
    const locals = new Map<string, ModelClient>();
    const client = (preset: "openrouter" | "local"): ModelClient => {
      if (preset === "openrouter") return openrouter;
      const local = admin.local();
      if (local === undefined) throw finError("PROVIDER_UNAVAILABLE", "no local model endpoint is set", { provider: "local", reason: "notConfigured" }, "a person sets one with `finisherd provider local set <ollama|lmstudio|url>`");
      let c = locals.get(local.baseUrl);
      if (c === undefined) locals.set(local.baseUrl, (c = localClient(local.baseUrl, { clock: core.clock })));
      return c;
    };
    const adapters = new Map<Harness, ProviderAdapter>();
    adapters.set(
      "builtin",
      BuiltinAdapter({
        client,
        ...(gateway === undefined ? {} : { gateway }),
        catalogue,
        finPath,
        index: () => finIndex(finPath).catch(() => undefined),
        nodes: (id) => allNodes(core.store, id),
      }),
    );
    if (fakeAllowed()) adapters.set("fake", FakeAdapter(d.options?.fakeScripts ?? DEFAULT_FAKE));
    const rolePrompt = (role: string) => (roles.get(role) ?? roles.get("editor"))!.prompt;
    const builtinEnv = builtinEnvironment({ finPath, server: d.endpoint, projectDirOf: d.registry.dirOf, rolePrompt });
    const environment: SessionEnvironment = {
      async prepare(s) {
        if (s.harness !== "fake") return builtinEnv.prepare(s);
        // The fake runs no `fin`: the role prompt alone, the same workspace and environment.
        const dir = d.registry.dirOf(s.projectId);
        if (dir === undefined) throw finError("PROJECT_NOT_FOUND", "the session's project is not open on this server", { projectId: s.projectId });
        const cwd = join(dir, "workspace", s.id);
        await mkdir(cwd, { recursive: true, mode: 0o700 });
        return { system: rolePrompt(s.role), cwd, env: childEnv({ FIN_SERVER: d.endpoint, FIN_SESSION: s.id, FIN_PROJECT_DIR: dir }) };
      },
    };
    const planner = sessionPlanner({
      roles,
      catalogue,
      harnesses: () => new Set(adapters.keys()),
      hasKey: (projectId) => admin.hasKey(projectId),
      local: () => admin.local(),
      localModels: (e) => listLocalModels(e.baseUrl, d.options?.fetch),
      // Best effort: the cached listing stands when OpenRouter cannot be reached.
      refreshListing: async () => {
        if (!prices.stale()) return;
        await prices.refresh(catalogue.openrouterBaseUrl, d.options?.fetch ?? fetch, AbortSignal.timeout(10_000)).catch((e: unknown) => log(`OpenRouter's model list was not refreshed: ${String(e)}`));
      },
    });
    const runtime = new Runtime({
      core,
      registry: d.registry,
      adapters: [...adapters.values()],
      planner,
      environment,
      tools: { builtin: builtinTools({ finPath, projectDirOf: d.registry.dirOf, ...(settings.toolTimeoutS === undefined ? {} : { timeoutMs: settings.toolTimeoutS * 1000 }) }) },
      journal: core.journal,
      pricing: cataloguePricing(catalogue),
      ...(settings.retentionDays === undefined ? {} : { retentionDays: settings.retentionDays }),
      ...(d.options?.timers === undefined ? {} : { timers: d.options.timers }),
      log: (message, details) => log(`${message} ${JSON.stringify(details)}`),
    });
    // Recovery before the transports serve: every live session interrupted, open calls given
    // their synthetic results, reservations returned, the outbox and credentials reconciled.
    const recovered = yield* Effect.promise(() => runtime.boot());
    if (recovered.interrupted.length > 0 || recovered.closedCalls > 0) log(`agent sessions recovered: ${recovered.interrupted.length} interrupted, ${recovered.closedCalls} open calls closed`);
    yield* Effect.addFinalizer(() => Effect.promise(() => runtime.close()));
    return { runtime, admin, adapters, lookup: new AgentLookup(d.db), catalogue };
  });
