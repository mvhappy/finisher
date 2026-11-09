// Registries, plugins and their read RPCs (8A plan Task 9a): the service over fin-node's
// `Registry` and the `plugins` chain of `control.db`, and the handlers with the real audit log.
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError, RegistryAddRequest } from "@finisher/contracts";
import { Registry, testHooks } from "@finisher/fin-node";
import { Effect, Exit } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { Audit } from "../src/control/audit.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { ChainRegistry, CONTROL_CHAINS } from "../src/control/migrations.ts";
import { makePlugins, reconcile } from "../src/plugins/registries.ts";
import { pluginHandlers } from "../src/plugins/rpc.ts";
import { PluginTables } from "../src/plugins/tables.ts";

const hooks = testHooks!;
const CONSOLE = { kind: "console", id: "console", human: true } as const;
const LOCAL = { kind: "local", id: "local", human: false } as const;
const DAY_MS = 86_400_000;

let tmp: string;
let serverData: string;
let control: ControlStore;
let now: number;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-plugins-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true });
  control = openControlStore(serverData, { serverId: "sid-1", checkpoint: false });
  now = Date.UTC(2026, 10, 8, 12);
});

afterEach(async () => {
  await control.close();
  rmSync(tmp, { recursive: true, force: true });
});

interface Built {
  readonly dir: string;
  readonly rootSha256: string;
  readonly rootFile: string;
}

let built = 0;
function registry(spec: object = { packages: ["@acme/pack@1.0.0"] }): Built {
  const dir = join(tmp, `registry-${++built}`);
  const r = JSON.parse(hooks.buildRegistry(dir, join(tmp, `keys-${built}`), JSON.stringify(spec))) as { rootSha256: string; rootFile: string };
  return { dir, ...r };
}

function service() {
  const native = Registry.open(serverData);
  const tables = new PluginTables(control);
  return { native, tables, plugins: makePlugins(native, tables, () => now) };
}

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);
const code = async (e: Effect.Effect<unknown, FinError>): Promise<string | undefined> => {
  const exit = await Effect.runPromiseExit(e);
  return Exit.isSuccess(exit) ? undefined : (Exit.findErrorOption(exit) as { value?: FinError }).value?.code;
};
const addReq = (r: Built, id = "team", root: "sha" | "file" = "sha"): RegistryAddRequest => ({
  cfg: { id, path: r.dir },
  root: root === "sha" ? { kind: "sha256", sha256: r.rootSha256 } : { kind: "file", path: r.rootFile },
});

describe("plugins chain", () => {
  test("migration_up_from_5a_last_migration", async () => {
    await control.close();
    rmSync(join(serverData, "control.db"), { force: true });
    // Before 8A: every chain but `plugins`.
    const before = new ChainRegistry();
    for (const c of CONTROL_CHAINS.ordered()) if (c.name !== "plugins") before.register(c.name, c.migrations, c);
    const old = openControlStore(serverData, { serverId: "sid-1", checkpoint: false, chains: before });
    const env = old.envId;
    new Audit(old).append({ kind: "listener.start", principal: { kind: "console", id: "console" }, outcome: "ok" });
    expect(() => old.db.prepare("SELECT 1 FROM registries").get()).toThrow();
    await old.close();
    // 8A's code opens the same database: the chain is added, nothing else is touched.
    control = openControlStore(serverData, { serverId: "sid-1", checkpoint: false });
    expect(control.envId).toBe(env);
    for (const t of ["registries", "plugins", "plugin_grants", "plugin_plans", "plugin_suggestions"]) {
      expect(control.db.prepare(`SELECT COUNT(*) AS n FROM ${t}`).get(), t).toEqual({ n: 0 });
    }
    const meta = (k: string) => (control.db.prepare("SELECT value FROM meta WHERE key = ?").get(k) as { value: string } | undefined)?.value;
    expect(meta("formatVersion.plugins")).toBe("4");
    expect(meta("minReaderVersion.plugins")).toBe("1");
    expect(control.db.prepare("SELECT n FROM migrations WHERE chain = 'plugins' ORDER BY n").all()).toEqual([{ n: 1 }, { n: 2 }, { n: 3 }, { n: 4 }]);
    expect(control.db.prepare("SELECT COUNT(*) AS n FROM audit WHERE kind = 'listener.start'").get()).toEqual({ n: 1 });
  });

  test("plugin_rows_are_checked", () => {
    const { tables } = service();
    const row = { id: "@acme/pack", version: "1.0.0", source: "registry" as const, treeHash: "h", dir: "store/x", kind: "content" as const, installedBy: "console:console", installedAt: now };
    tables.addPlugin(row);
    expect(() => tables.addPlugin(row)).toThrow();
    expect(() => tables.addPlugin({ ...row, id: "@acme/b", source: "git" as never })).toThrow();
    expect(tables.installedOne("@acme/pack")).toEqual({ id: "@acme/pack", version: "1.0.0", source: "registry", kind: "content", enabled: false, state: "active", autoUpdate: false, installedBy: "console:console", installedAt: now });
  });
});

describe("registries", () => {
  test("registry_add_needs_root", async () => {
    const r = registry();
    const { plugins, tables } = service();
    const wrong = "0".repeat(64);
    expect(await code(plugins.add({ cfg: { id: "team", path: r.dir }, root: { kind: "sha256", sha256: wrong } }, CONSOLE))).toBe("REGISTRY_UNTRUSTED");
    expect(tables.registries()).toEqual([]);
    expect((await run(plugins.registries())).registries).toEqual([]);
    // Another registry's root file does not pin this folder either.
    const other = registry();
    expect(await code(plugins.add({ cfg: { id: "team", path: r.dir }, root: { kind: "file", path: other.rootFile } }, CONSOLE))).toBe("REGISTRY_UNTRUSTED");
    expect(await code(plugins.add(addReq(r, "Bad_Id"), CONSOLE))).toBe("REGISTRY_INVALID");
    expect(tables.registries()).toEqual([]);
    // A root file or its hash pins it; the row names who added it.
    const entry = await run(plugins.add(addReq(r, "team", "file"), CONSOLE));
    expect(entry).toMatchObject({ id: "team", kind: "local", rootSha256: r.rootSha256, freshness: "relaxed", addedBy: "console:console", addedAt: now });
    expect(await code(plugins.add(addReq(r), CONSOLE))).toBe("REGISTRY_EXISTS");
    expect(tables.registry("team")?.addedBy).toBe("console:console");
  });

  test("add_rolls_back_when_the_row_cannot_be_written", async () => {
    const r = registry();
    const { plugins, tables, native } = service();
    tables.addRegistry({ id: "team", kind: "local", rootSha256: "x", freshness: "relaxed", addedBy: "x:x", addedAt: 1 });
    expect(await code(plugins.add(addReq(r), CONSOLE))).toBe("INTERNAL");
    // Neither half is left: fin-registry took the registry back.
    expect(JSON.parse(await native.registryList()).registries).toEqual([]);
  });

  test("refresh_records_by_human", async () => {
    const r = registry();
    const { plugins, tables } = service();
    await run(plugins.add(addReq(r), CONSOLE));
    expect(tables.registry("team")?.lastRefresh).toBeUndefined();
    const human = await run(plugins.refresh("team", CONSOLE));
    expect(human.byHuman).toBe(true);
    expect(human.results[0]).toMatchObject({ id: "team", ok: true });
    expect(tables.registry("team")?.lastRefresh).toEqual({ at: now, by: "console:console", byHuman: true, ok: true });
    // A script on this machine is not a person: recorded as such.
    now += 5000;
    const script = await run(plugins.refresh(undefined, LOCAL));
    expect(script.byHuman).toBe(false);
    expect(tables.registry("team")?.lastRefresh).toEqual({ at: now, by: "local:local", byHuman: false, ok: true });
    const status = await run(plugins.status("team"));
    expect(status.plugins).toBe(1);
    expect(status.trusted.timestamp).toBeGreaterThan(0);
    expect(status.lastRefresh?.byHuman).toBe(false);
  });

  test("refresh_failure_is_recorded_and_isolated", async () => {
    const good = registry();
    const gone = registry();
    const { plugins, tables } = service();
    await run(plugins.add(addReq(good, "good"), CONSOLE));
    await run(plugins.add(addReq(gone, "gone"), CONSOLE));
    rmSync(join(gone.dir, "metadata", "timestamp.json"));
    // Every registry: the broken one is a result, the other still refreshes.
    const all = await run(plugins.refresh(undefined, CONSOLE));
    const bad = all.results.find((x) => x.id === "gone")!;
    expect(bad.ok).toBe(false);
    expect(all.results.find((x) => x.id === "good")?.ok).toBe(true);
    const failedCode = tables.registry("gone")?.lastRefresh?.code;
    expect(failedCode).toBeTypeOf("string");
    expect(tables.registry("gone")?.lastRefresh?.ok).toBe(false);
    // One registry named: its failure is the call's.
    expect(await code(plugins.refresh("gone", CONSOLE))).toBe(failedCode);
    expect(await code(plugins.refresh("nope", CONSOLE))).toBe("REGISTRY_NOT_FOUND");
  });

  test("plugin_list_shows_staleness", async () => {
    const fresh = registry();
    const stale = registry({ packages: ["@acme/old@1.0.0"], ageDays: 40, maxStaleDays: 60 });
    const { plugins, tables } = service();
    await run(plugins.add(addReq(fresh, "fresh"), CONSOLE));
    await run(plugins.add(addReq(stale, "stale"), CONSOLE));
    const refreshed = await run(plugins.refresh("stale", CONSOLE));
    const r = refreshed.results[0];
    expect(r?.ok && r.report.staleDays).toBeGreaterThanOrEqual(9);
    expect(r?.ok && r.report.warnings.map((w) => w.code)).toContain("REGISTRY_STALE");
    await run(plugins.refresh("fresh", CONSOLE));
    const seed = (id: string, source: "registry" | "bundled", registryId?: string) =>
      tables.addPlugin({ id, version: "1.0.0", source, ...(registryId === undefined ? {} : { registryId }), treeHash: "h", dir: `store/${id}`, kind: "content", installedBy: "console:console", installedAt: now });
    seed("@acme/old", "registry", "stale");
    seed("@acme/pack", "registry", "fresh");
    seed("@finisher/captions-basic", "bundled");
    const list = await run(plugins.installed());
    const by = Object.fromEntries(list.map((p) => [p.id, p]));
    expect(by["@acme/old"]?.registryStaleDays).toBeGreaterThanOrEqual(9);
    expect(by["@acme/pack"]).not.toHaveProperty("registryStaleDays");
    expect(by["@finisher/captions-basic"]).not.toHaveProperty("registryStaleDays");
    expect((await run(plugins.registries())).registries.find((x) => x.id === "stale")?.staleDays).toBeGreaterThanOrEqual(9);
  });

  test("search_and_info_read_the_registries", async () => {
    const a = registry({ packages: ["@acme/pack@1.0.0", "@acme/pack@1.1.0", "@acme/other@0.1.0"] });
    const b = registry({ packages: ["@beta/thing@3.0.0", "@acme/pack@1.0.0"] });
    const { plugins, tables } = service();
    await run(plugins.add(addReq(a, "a"), CONSOLE));
    await run(plugins.add(addReq(b, "b"), CONSOLE));
    await run(plugins.refresh(undefined, CONSOLE));
    const ids = (l: ReadonlyArray<{ id: string; registryId: string }>) => l.map((x) => `${x.id}@${x.registryId}`);
    expect(ids(await run(plugins.search({})))).toEqual(["@acme/other@a", "@acme/pack@a", "@acme/pack@b", "@beta/thing@b"]);
    expect(ids(await run(plugins.search({ text: "THING" })))).toEqual(["@beta/thing@b"]);
    expect(ids(await run(plugins.search({ registry: "a", text: "pack" })))).toEqual(["@acme/pack@a"]);
    expect(await run(plugins.search({ kind: "code" }))).toEqual([]);
    expect(await code(plugins.search({ registry: "nope" }))).toBe("REGISTRY_NOT_FOUND");
    const info = await run(plugins.info("@acme/pack"));
    expect(info.registries.map((x) => [x.registryId, x.versions.map((v) => v.version)])).toEqual([["a", ["1.0.0", "1.1.0"]], ["b", ["1.0.0"]]]);
    expect(info.installed).toBeUndefined();
    tables.addPlugin({ id: "@acme/pack", version: "1.0.0", source: "registry", registryId: "a", treeHash: "h", dir: "d", kind: "content", installedBy: "console:console", installedAt: now });
    expect((await run(plugins.info("@acme/pack"))).installed).toMatchObject({ version: "1.0.0", registryId: "a" });
    expect(await code(plugins.info("@acme/unknown"))).toBe("PLUGIN_NOT_FOUND");
    expect(await code(plugins.info("not an id"))).toBe("PLUGIN_NOT_FOUND");
  });

  test("remove_keeps_trust_and_forget_deletes_it", async () => {
    const r = registry();
    const { plugins, tables } = service();
    await run(plugins.add(addReq(r), CONSOLE));
    await run(plugins.refresh("team", CONSOLE));
    expect(await code(plugins.forget(r.rootSha256))).toBe("REGISTRY_INVALID");
    await run(plugins.remove("team"));
    expect(tables.registries()).toEqual([]);
    expect(await code(plugins.remove("team"))).toBe("REGISTRY_NOT_FOUND");
    // Re-adding the same root resumes from the trusted versions.
    const again = await run(plugins.add(addReq(r), CONSOLE));
    expect(again.trusted.timestamp).toBeGreaterThan(0);
    await run(plugins.remove("team"));
    await run(plugins.forget(r.rootSha256));
    expect((await run(plugins.add(addReq(r), CONSOLE))).trusted.timestamp).toBeUndefined();
  });

  test("a_damaged_registry_does_not_hide_the_others", async () => {
    const a = registry();
    const b = registry();
    const { plugins } = service();
    await run(plugins.add(addReq(a, "a"), CONSOLE));
    await run(plugins.add(addReq(b, "b"), CONSOLE));
    writeFileSync(join(serverData, "plugins", "registries", "b", "registry.json"), "{");
    const list = await run(plugins.registries());
    expect(list.registries.map((x) => x.id)).toEqual(["a"]);
    expect(list.problems.map((p) => [p.id, p.code])).toEqual([["b", "REGISTRY_INVALID"]]);
  });

  test("reconcile_aligns_rows_and_folders", async () => {
    const a = registry();
    const { native, tables } = service();
    // A folder without a row (a crash after fin-registry wrote it) and a row without a folder.
    await native.registryAdd(JSON.stringify({ id: "a", path: a.dir }), JSON.stringify({ kind: "sha256", sha256: a.rootSha256 }));
    tables.addRegistry({ id: "ghost", kind: "local", rootSha256: "x", freshness: "relaxed", addedBy: "x:x", addedAt: 1 });
    await Effect.runPromise(reconcile(native, tables, () => now));
    expect(tables.registries().map((x) => [x.id, x.addedBy])).toEqual([["a", "system:reconcile"]]);
  });
});

describe("handlers", () => {
  function handlers() {
    const { plugins } = service();
    const audit = new Audit(control, () => now);
    const h = pluginHandlers({ plugins, audit, guard: (_tag, self) => self });
    const rows = () =>
      (control.db.prepare("SELECT kind, principalKind, outcome, code, details FROM audit WHERE kind LIKE 'registry.%' OR kind LIKE 'plugin.%' ORDER BY id").all() as Array<{ kind: string; principalKind: string; outcome: string; code: string | null; details: string }>).map((r) => ({
        ...r,
        details: JSON.parse(r.details) as Record<string, unknown>,
      }));
    return { h, audit, rows };
  }
  const as = (principal: { kind: string; id: string; human: boolean }) => ({ headers: {}, guard: { denyBranches: [] }, principal: { ...principal, scopes: [], projects: "*", branches: "*" } });

  test("registry_administration_is_audited_with_ids_only", async () => {
    const r = registry();
    const { h, audit, rows } = handlers();
    const wrong = { cfg: { id: "team", path: r.dir }, root: { kind: "sha256" as const, sha256: "1".repeat(64) } };
    expect(await code(h["registry.add"](wrong, as(CONSOLE)))).toBe("REGISTRY_UNTRUSTED");
    await run(h["registry.add"](addReq(r), as(CONSOLE)));
    await run(h["registry.refresh"]({}, as(LOCAL)));
    expect(await run(h["registry.remove"]({ id: "team" }, as(CONSOLE)))).toEqual({ removed: true });
    expect(await run(h["registry.forget"]({ rootSha256: r.rootSha256.toUpperCase() }, as(CONSOLE)))).toEqual({ forgotten: true });
    expect(await code(h["registry.add"]({ cfg: { id: "x", path: "relative/path" }, root: wrong.root }, as(CONSOLE)))).toBe("INVALID_PATH");
    expect(rows().map((x) => [x.kind, x.principalKind, x.outcome, x.code])).toEqual([
      ["registry.add", "console", "denied", "REGISTRY_UNTRUSTED"],
      ["registry.trust", "console", "ok", null],
      ["registry.add", "console", "ok", null],
      ["registry.remove", "console", "ok", null],
      ["registry.forget", "console", "ok", null],
      ["registry.add", "console", "failed", "INVALID_PATH"],
    ]);
    expect(rows()[1]?.details).toEqual({ registryId: "team", rootSha256: r.rootSha256 });
    expect(rows()[2]?.details).toEqual({ registryId: "team" });
    expect(rows()[4]?.details).toEqual({ rootSha256: r.rootSha256 });
    // Every row's details fit the closed schema: nothing was dropped.
    expect(audit.failures()).toBe(0);
    expect(JSON.stringify(rows())).not.toContain(r.dir);
  });

  test("a_refresh_that_does_not_verify_is_audited", async () => {
    const r = registry();
    const { h, audit, rows } = handlers();
    await run(h["registry.add"](addReq(r), as(CONSOLE)));
    // The folder's timestamp is replaced by another registry's: its signatures do not verify.
    const other = registry();
    writeFileSync(join(r.dir, "metadata", "timestamp.json"), readFileSync(join(other.dir, "metadata", "timestamp.json")));
    const failed = await Effect.runPromiseExit(h["registry.refresh"]({ id: "team" }, as(LOCAL)));
    expect(Exit.isFailure(failed)).toBe(true);
    const fails = rows().filter((x) => x.kind === "registry.verifyFail");
    expect(fails).toHaveLength(1);
    expect(fails[0]).toMatchObject({ principalKind: "local", outcome: "failed", details: { registryId: "team" } });
    expect(fails[0]?.code).toMatch(/^(TUF_|REGISTRY_)/);
    expect(audit.failures()).toBe(0);
  });

  test("reads_answer_without_the_audit_log", async () => {
    const r = registry();
    const { h, rows } = handlers();
    await run(h["registry.add"](addReq(r), as(CONSOLE)));
    const before = rows().length;
    await run(h["registry.refresh"]({ id: "team" }, as(LOCAL)));
    expect((await run(h["registry.list"]({}, as(LOCAL)))).registries).toHaveLength(1);
    expect((await run(h["registry.status"]({ id: "team" }, as(LOCAL)))).id).toBe("team");
    expect(await run(h["plugin.list"]())).toEqual({ plugins: [], bundled: { integrity: "hashesOnly", plugins: [] } });
    expect((await run(h["plugin.search"]({ text: "pack" }))).map((l) => l.id)).toEqual(["@acme/pack"]);
    expect((await run(h["plugin.info"]({ id: "@acme/pack" }))).registries).toHaveLength(1);
    expect(rows()).toHaveLength(before);
    expect(now - DAY_MS).toBeLessThan(now);
  });
});
