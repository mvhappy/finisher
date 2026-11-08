// The plugin and registry RPCs' scopes (8A plan Task 9a; Decision 29): the table, the
// owner-only administration through the chain, and the same on a running server.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { defaultScopes, type FinError, FinRpcs, HUMAN_ONLY, type Principal, RPC_SCOPES, requiredScopes, type Scope, SCOPES } from "@finisher/contracts";
import { testHooks } from "@finisher/fin-node";
import { Effect, Exit } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { withChain } from "../src/chain/chain.ts";
import { Connections } from "../src/connections.ts";
import type { Audit, AuditInput } from "../src/control/audit.ts";
import { ANONYMOUS, CONSOLE, LOCAL } from "../src/principal.ts";
import { connectProved, type NdjsonClient, type RunningServer, startServer } from "./support/ndjson-client.ts";
import { SAMPLE_PAYLOADS } from "./support/payloads.ts";

const READS = ["registry.list", "registry.status", "registry.refresh", "plugin.list", "plugin.search", "plugin.info"] as const;
const ADMIN = ["registry.add", "registry.remove", "registry.forget"] as const;
const NEW = [...READS, ...ADMIN] as const;

const agent = (scopes: Scope[]): Principal => ({ kind: "agent", id: "01agent000000000000000000a", scopes, projects: "*", branches: "*", human: false });

function harness(principal: Principal) {
  const seen: string[] = [];
  const audited: AuditInput[] = [];
  const connections = new Connections("socket", () => "/sd/run/fin.sock");
  connections.attach(1, { close() {}, closeAfterReply() {}, terminate() {} });
  connections.get(1)!.principal = principal;
  const handlers = Object.fromEntries(NEW.map((tag) => [tag, () => Effect.sync(() => (seen.push(tag), {}))]));
  const audit = { append: (e: AuditInput) => audited.push(e) } as unknown as Audit;
  const projects = { isOpen: () => false, dirOf: () => undefined, resolveBranch: () => Effect.succeed("main"), branches: () => Effect.succeed([]) };
  const chained = withChain(handlers, { connections, projects, audit }) as unknown as Record<string, (p: unknown, o: unknown) => Effect.Effect<unknown, FinError>>;
  const call = async (tag: string) => {
    const exit = await Effect.runPromiseExit(chained[tag]!(SAMPLE_PAYLOADS[tag as keyof typeof SAMPLE_PAYLOADS], { client: { id: 1 }, headers: {} }));
    return Exit.isSuccess(exit) ? undefined : (Exit.findErrorOption(exit) as { value?: FinError }).value?.code;
  };
  return { seen, audited, call };
}

describe("scopes", () => {
  test("every_new_rpc_has_a_scope", () => {
    const tags = [...FinRpcs.requests.keys()];
    for (const tag of NEW) expect(tags, tag).toContain(tag);
    for (const tag of READS) expect(RPC_SCOPES[tag].scopes, tag).toEqual(["plugin.read"]);
    for (const tag of ADMIN) expect(RPC_SCOPES[tag].scopes, tag).toEqual(["plugin.admin"]);
    for (const s of ["plugin.read", "plugin.install", "plugin.admin", "plugin.suggest"] as const) expect(SCOPES).toContain(s);
    // Nothing else changed: the reads need nothing but `plugin.read`, whatever the payload.
    for (const tag of NEW) expect(requiredScopes(tag, SAMPLE_PAYLOADS[tag])).toEqual(RPC_SCOPES[tag].scopes);
  });

  test("lifecycle_rpcs_have_their_scopes", () => {
    const want: Record<string, Scope[]> = {
      "plugin.plan": ["plugin.read"],
      "plugin.install": ["plugin.install"],
      "plugin.remove": ["plugin.install"],
      "plugin.enable": ["plugin.admin"],
      "plugin.disable": ["plugin.admin"],
      "plugin.suggest": ["plugin.suggest", "project.read"],
    };
    for (const [tag, scopes] of Object.entries(want)) {
      expect(RPC_SCOPES[tag as keyof typeof RPC_SCOPES].scopes, tag).toEqual(scopes);
      expect(requiredScopes(tag as keyof typeof RPC_SCOPES, SAMPLE_PAYLOADS[tag as keyof typeof SAMPLE_PAYLOADS]), tag).toEqual(scopes);
    }
    // Only a downgrade escalates, to the owner's scope; nothing of the lifecycle writes a branch.
    expect(requiredScopes("plugin.plan", { id: "@acme/pack", version: "1.0.0", allowDowngrade: true })).toEqual(["plugin.read", "plugin.install"]);
    for (const tag of Object.keys(want)) expect(RPC_SCOPES[tag as keyof typeof RPC_SCOPES].writes, tag).toBeUndefined();
  });

  test("owner_scopes_are_human_only", () => {
    expect([...HUMAN_ONLY].filter((s) => s.startsWith("plugin."))).toEqual(["plugin.install", "plugin.admin"]);
    const local = defaultScopes("local");
    expect(local).toContain("plugin.read");
    expect(local).toContain("plugin.suggest");
    expect(local).not.toContain("plugin.install");
    expect(local).not.toContain("plugin.admin");
    expect(defaultScopes("console")).toEqual(SCOPES);
    expect(defaultScopes("agent")).toEqual([]);
  });
});

describe("through the chain", () => {
  test("registry_admin_is_owner_only", async () => {
    for (const tag of ADMIN) {
      // Whatever scopes it holds, only a person gets in; the refusal is audited.
      for (const p of [LOCAL, agent([...SCOPES]), { ...agent([...SCOPES]), kind: "mcp" as const }]) {
        const h = harness(p);
        expect(await h.call(tag), `${tag} × ${p.kind}`).toBe("HUMAN_ONLY");
        expect(h.seen).toEqual([]);
        expect(h.audited.map((a) => [a.kind, a.code, a.details?.rpc])).toEqual([["auth.denied", "HUMAN_ONLY", tag]]);
      }
      const owner = harness(CONSOLE);
      expect(await owner.call(tag)).toBeUndefined();
      expect(owner.seen).toEqual([tag]);
      expect(await harness(ANONYMOUS).call(tag)).toBe("UNAUTHENTICATED");
    }
  });

  test("reads_need_plugin_read_only", async () => {
    for (const tag of READS) {
      for (const p of [LOCAL, CONSOLE, agent(["plugin.read"])]) expect(await harness(p).call(tag), `${tag} × ${p.kind}`).toBeUndefined();
      // A credential without the scope is refused, with the scope named.
      const h = harness(agent(["server.read", "project.read", "plugin.suggest"]));
      expect(await h.call(tag), tag).toBe("SCOPE_DENIED");
      expect(h.audited[0]?.details).toMatchObject({ rpc: tag, scopes: ["plugin.read"] });
    }
  });
});

describe("on a server", () => {
  let tmp: string;
  let server: RunningServer;
  const clients: NdjsonClient[] = [];

  beforeEach(async () => {
    tmp = mkdtempSync(join(tmpdir(), "finisherd-plugin-scopes-"));
    server = await startServer(join(tmp, "server-data"));
  });
  afterEach(async () => {
    await Promise.all(clients.splice(0).map((c) => c.close().catch(() => undefined)));
    await server.stop();
    rmSync(tmp, { recursive: true, force: true });
  });
  const connect = async (credential: "local" | "console") => {
    const c = await connectProved(server, credential);
    clients.push(c);
    return c;
  };

  test("the_owner_adds_and_anyone_reads", async () => {
    const built = JSON.parse(testHooks!.buildRegistry(join(tmp, "registry"), join(tmp, "keys"), JSON.stringify({ packages: ["@acme/pack@1.0.0"] }))) as { rootSha256: string };
    const add = { cfg: { id: "team", path: join(tmp, "registry") }, root: { kind: "sha256", sha256: built.rootSha256 } };
    const local = await connect("local");
    const owner = await connect("console");
    // `local` reads, and cannot add.
    expect(await local.call("registry.list", {})).toEqual({ ok: true, value: { registries: [], problems: [] } });
    expect(await local.call("registry.add", add)).toMatchObject({ ok: false, error: { code: "HUMAN_ONLY" } });
    // The plugin that comes with the app is installed at start (8A Task 10), nothing else.
    expect(await local.call("plugin.list", {})).toMatchObject({
      ok: true,
      value: { plugins: [{ id: "@finisher/captions-basic", source: "bundled", enabled: true }], bundled: { integrity: "hashesOnly", plugins: [{ id: "@finisher/captions-basic", ok: true }] } },
    });
    // The owner adds it; `local` refreshes it (recorded as not a person) and searches it.
    expect(await owner.call("registry.add", add)).toMatchObject({ ok: true, value: { id: "team", addedBy: "console:console" } });
    const refreshed = await local.call("registry.refresh", {});
    expect(refreshed).toMatchObject({ ok: true, value: { byHuman: false, results: [{ id: "team", ok: true }] } });
    expect(await local.call("registry.status", { id: "team" })).toMatchObject({ ok: true, value: { plugins: 1, lastRefresh: { byHuman: false, by: "local:local", ok: true } } });
    expect(await local.call("plugin.search", { text: "pack" })).toMatchObject({ ok: true, value: [{ id: "@acme/pack", registryId: "team" }] });
    expect(await local.call("plugin.info", { id: "@acme/pack" })).toMatchObject({ ok: true, value: { id: "@acme/pack", registries: [{ registryId: "team" }] } });
    expect(await local.call("plugin.info", { id: "@acme/none" })).toMatchObject({ ok: false, error: { code: "PLUGIN_NOT_FOUND" } });
    expect(await local.call("registry.remove", { id: "team" })).toMatchObject({ ok: false, error: { code: "HUMAN_ONLY" } });
    expect(await owner.call("registry.remove", { id: "team" })).toEqual({ ok: true, value: { removed: true } });
  });
});
