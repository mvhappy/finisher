// Bundled plugins without TUF (8A plan Task 10, Decision 24): the plugins that come with the app
// install at start without a plan, update with the app, and are revoked when their files do
// not match the hashes built into the binary. Over fin-node's `Registry` and the real
// `resources/plugins/bundled`; the update and tamper cases copy it into a temporary folder.
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";
import { bundledIntegrity, defaultBundledRoot } from "../src/plugins/bundled.ts";
import { as, CONSOLE, codeOf, hooks, plugins, run } from "./support/plugins-fixture.ts";

const f = plugins();
const REAL = fileURLToPath(new URL("../../../resources/plugins/bundled", import.meta.url));
const ID = "@finisher/captions-basic";

const copyOfReal = () => {
  const root = join(f.tmp, "bundled");
  cpSync(REAL, root, { recursive: true });
  return root;
};

/** The tree `name` under `root` with a code plugin manifest of `version` and `permissions`, as a list entry. */
function codeTree(root: string, version: string, permissions: object[]) {
  const dir = join(root, "probe-tool");
  mkdirSync(join(dir, "dist"), { recursive: true });
  writeFileSync(join(dir, "dist", "plugin.mjs"), "export {};\n");
  writeFileSync(
    join(dir, "finisher-plugin.json"),
    JSON.stringify({
      manifestVersion: 1,
      id: "@finisher/probe-tool",
      version,
      kind: "code",
      hostApi: "^1.0",
      license: "MIT",
      publisher: { name: "Finisher" },
      platforms: ["darwin-arm64", "darwin-x64", "linux-x64", "linux-arm64", "win32-x64", "win32-arm64"],
      runtime: { type: "node", main: "dist/plugin.mjs", node: ">=24.0" },
      permissions,
    }),
  );
  return { id: "@finisher/probe-tool", version, treeHash: hooks.treeHash(dir) };
}

const entry = (dir: string, id: string, version: string) => ({ id, version, treeHash: hooks.treeHash(dir) });
const standIn = (s: { native: { setBundledForTest(j: string | null): void } }, list: object[]) => s.native.setBundledForTest(JSON.stringify(list));
const bumped = (root: string, version: string) => {
  const file = join(root, "captions-basic", "finisher-plugin.json");
  writeFileSync(file, readFileSync(file, "utf8").replace('"version": "1.0.0"', `"version": "${version}"`));
  return entry(join(root, "captions-basic"), ID, version);
};

describe("the shipped resources", () => {
  test("bundled_installs_at_first_start", async () => {
    const s = f.service();
    const status = await run(s.plugins.syncBundled());
    expect(status.plugins).toEqual([{ id: ID, version: "1.0.0", ok: true }]);
    expect(s.tables.installedFull(ID)).toMatchObject({ source: "bundled", kind: "content", enabled: true, state: "active", version: "1.0.0", installedBy: "system:bundled", autoUpdate: false });
    expect(s.tables.installedFull(ID)?.treeHash).toBe(JSON.parse(s.native.bundledList())[0].treeHash);
    expect(f.storeDirs(ID).length).toBe(1);
    expect(s.tables.grants(ID)).toEqual([]);
    expect(s.rows().map((r) => [r.kind, r.principalKind, r.outcome, r.details])).toEqual([["plugin.install", "system", "ok", { pluginId: ID, version: "1.0.0", permissionsAdded: 0, risk: "low", auto: true }]]);
    // No plan is left, and the compiled-in list is what the resources say.
    expect(f.control.db.prepare("SELECT COUNT(*) AS n FROM plugin_plans").get()).toEqual({ n: 0 });
    expect(readdirNames(join(f.serverData, "plugins", "staging"))).toEqual([]);
    // A second start changes nothing and records nothing.
    const before = f.snapshot();
    await run(s.plugins.syncBundled());
    expect(f.snapshot()).toEqual(before);
    expect(s.rows().length).toBe(1);
    expect(f.storeDirs(ID).length).toBe(1);
  });

  test("bundled_cannot_be_removed", async () => {
    const s = f.service();
    await run(s.plugins.syncBundled());
    expect(await codeOf(s.h["plugin.remove"]({ id: ID }, as(CONSOLE)))).toBe("PLUGIN_BUNDLED");
    expect(await codeOf(s.h["plugin.plan"]({ id: ID, update: true }, as(CONSOLE)))).toBe("PLUGIN_BUNDLED");
    expect(s.tables.installedOne(ID)).toBeDefined();
    expect(f.storeDirs(ID).length).toBe(1);
    // It can be disabled and enabled again.
    expect((await run(s.h["plugin.disable"]({ id: ID }, as(CONSOLE)))).plugin).toMatchObject({ enabled: false, state: "disabled" });
    expect((await run(s.h["plugin.enable"]({ id: ID }, as(CONSOLE)))).plugin).toMatchObject({ enabled: true, state: "active" });
  });

  test("status_reports_hashes_only_on_unsigned_builds", async () => {
    const s = f.service();
    await run(s.plugins.syncBundled());
    const list = await run(s.h["plugin.list"]());
    expect(list.bundled).toEqual({ integrity: "hashesOnly", plugins: [{ id: ID, version: "1.0.0", ok: true }] });
    expect(bundledIntegrity()).toBe("hashesOnly");
    // The status shows ids and versions, never the hashes themselves.
    expect(JSON.stringify(list.bundled)).not.toMatch(/[0-9a-f]{32}/);
    expect(existsSync(defaultBundledRoot())).toBe(true);
  });
});

describe("updates with the app", () => {
  test("bundled_updates_with_app", async () => {
    const root = copyOfReal();
    const s = f.service({ bundledRoot: root });
    const v1 = entry(join(root, "captions-basic"), ID, "1.0.0");
    const probe1 = codeTree(root, "1.0.0", [{ perm: "project:read" }]);
    standIn(s, [v1, probe1]);
    await run(s.plugins.syncBundled());
    expect(s.tables.installedOne("@finisher/probe-tool")).toMatchObject({ kind: "code", enabled: false, state: "disabled", version: "1.0.0" });
    expect(s.tables.grants("@finisher/probe-tool").length).toBe(1);
    // The person turned the content pack off; an app update does not turn it on again.
    await run(s.life.disable(ID));
    // The new app: a newer pack and a probe tool that wants network access too.
    const v11 = bumped(root, "1.1.0");
    const probe2 = codeTree(root, "1.1.0", [{ perm: "project:read" }, { perm: "net", origins: ["https://api.example.org"] }]);
    standIn(s, [v11, probe2]);
    const before = f.storeDirs("@finisher/probe-tool");
    const status = await run(s.plugins.syncBundled());
    expect(status.plugins.map((p) => [p.id, p.version, p.ok])).toEqual([[ID, "1.1.0", true], ["@finisher/probe-tool", "1.1.0", true]]);
    expect(s.tables.installedOne(ID)).toMatchObject({ version: "1.1.0", enabled: false, state: "disabled", source: "bundled" });
    expect(s.tables.installedOne("@finisher/probe-tool")).toMatchObject({ version: "1.1.0", enabled: false });
    // The widening was applied without a plan, and is on the record with its size.
    expect(s.tables.grants("@finisher/probe-tool").length).toBe(2);
    const rows = s.rows().filter((r) => r.principalKind === "system");
    const tool = rows.filter((r) => r.details.pluginId === "@finisher/probe-tool").map((r) => [r.kind, r.details.permissionsAdded, r.details.risk]);
    expect(tool).toEqual([["plugin.install", 1, "low"], ["plugin.grant", 1, undefined], ["plugin.update", 1, "high"], ["plugin.grant", 1, undefined]]);
    expect(rows.find((r) => r.kind === "plugin.update" && r.details.pluginId === ID)?.details).toMatchObject({ version: "1.1.0", permissionsAdded: 0, auto: true });
    // One version per plugin stays in the store.
    expect(f.storeDirs("@finisher/probe-tool").length).toBe(1);
    expect(f.storeDirs("@finisher/probe-tool")).not.toEqual(before);
    // The same app again: nothing.
    const snap = f.snapshot();
    await run(s.plugins.syncBundled());
    expect(f.snapshot()).toEqual(snap);
  });

  test("an_older_app_installs_its_own_version", async () => {
    const root = copyOfReal();
    const s = f.service({ bundledRoot: root });
    standIn(s, [bumped(root, "1.1.0")]);
    await run(s.plugins.syncBundled());
    expect(s.tables.installedOne(ID)?.version).toBe("1.1.0");
    writeFileSync(join(root, "captions-basic", "finisher-plugin.json"), readFileSync(join(REAL, "captions-basic", "finisher-plugin.json")));
    standIn(s, [entry(join(root, "captions-basic"), ID, "1.0.0")]);
    await run(s.plugins.syncBundled());
    expect(s.tables.installedOne(ID)).toMatchObject({ version: "1.0.0", source: "bundled" });
    expect(f.storeDirs(ID).length).toBe(1);
  });

  test("the_same_version_with_other_bytes_is_replaced", async () => {
    const root = copyOfReal();
    const s = f.service({ bundledRoot: root });
    standIn(s, [entry(join(root, "captions-basic"), ID, "1.0.0")]);
    await run(s.plugins.syncBundled());
    const first = s.tables.installedFull(ID)!.treeHash;
    writeFileSync(join(root, "captions-basic", "messages", "en.json"), JSON.stringify({ "style.bold": "Bolder", "style.clean": "Clean", "style.punch": "Punch" }));
    standIn(s, [entry(join(root, "captions-basic"), ID, "1.0.0")]);
    await run(s.plugins.syncBundled());
    expect(s.tables.installedFull(ID)?.treeHash).not.toBe(first);
    expect(f.storeDirs(ID).length).toBe(1);
  });

  /** Review M2 of Tasks 10 and 12: the app's own housekeeping must not undo the owner's switch. */
  test("a_rebuilt_app_keeps_an_owner_disabled_pack_off", async () => {
    const root = copyOfReal();
    const s = f.service({ bundledRoot: root });
    standIn(s, [entry(join(root, "captions-basic"), ID, "1.0.0")]);
    await run(s.plugins.syncBundled());
    await run(s.life.disable(ID));
    writeFileSync(join(root, "captions-basic", "messages", "en.json"), JSON.stringify({ "style.bold": "Bolder", "style.clean": "Clean", "style.punch": "Punch" }));
    standIn(s, [entry(join(root, "captions-basic"), ID, "1.0.0")]);
    await run(s.plugins.syncBundled());
    expect(s.tables.installedOne(ID)).toMatchObject({ enabled: false, state: "disabled", version: "1.0.0" });
    expect(f.storeDirs(ID).length).toBe(1);
    // An enabled one stays on, with its automatic-update choice.
    await run(s.life.enable(ID));
    const row = s.tables.installedFull(ID)!;
    s.tables.replacePlugin({ ...row, autoUpdate: true } as never, 1);
    writeFileSync(join(root, "captions-basic", "messages", "en.json"), JSON.stringify({ "style.bold": "Boldest", "style.clean": "Clean", "style.punch": "Punch" }));
    standIn(s, [entry(join(root, "captions-basic"), ID, "1.0.0")]);
    await run(s.plugins.syncBundled());
    expect(s.tables.installedOne(ID)).toMatchObject({ enabled: true, state: "active", autoUpdate: true });
  });

  test("a_plan_that_fails_leaves_the_installed_pack_in_place", async () => {
    const root = copyOfReal();
    const s = f.service({ bundledRoot: root });
    standIn(s, [entry(join(root, "captions-basic"), ID, "1.0.0")]);
    await run(s.plugins.syncBundled());
    const before = s.tables.installedFull(ID)!;
    const dirs = f.storeDirs(ID);
    // The rebuilt resources do not match the hash compiled into this binary: the plan is refused.
    writeFileSync(join(root, "captions-basic", "messages", "en.json"), JSON.stringify({ "style.bold": "Bolder", "style.clean": "Clean", "style.punch": "Punch" }));
    const rebuilt = { id: ID, version: "1.0.0", treeHash: hooks.treeHash(join(root, "captions-basic")) };
    expect(await codeOf(s.life.installBundled(root, rebuilt))).toBe("PLUGIN_BUNDLED_MISMATCH");
    expect(s.tables.installedFull(ID)).toEqual(before);
    expect(s.tables.grants(ID).length).toBeGreaterThanOrEqual(0);
    expect(f.storeDirs(ID)).toEqual(dirs);
  });

  test("two_starts_at_once_install_it_once", async () => {
    const s = f.service();
    await Promise.all([run(s.plugins.syncBundled()), run(s.plugins.syncBundled())]);
    expect(s.tables.installed().map((p) => p.id)).toEqual([ID]);
    expect(f.storeDirs(ID).length).toBe(1);
    expect(s.rows().filter((r) => r.kind === "plugin.install").length).toBe(1);
  });
});

describe("tampered resources", () => {
  test("tampered_bundled_tree_revoked", async () => {
    const root = copyOfReal();
    const s = f.service({ bundledRoot: root });
    await run(s.plugins.syncBundled());
    expect(s.tables.installedOne(ID)).toMatchObject({ enabled: true, state: "active" });
    // Someone changes a style after the first start.
    const style = join(root, "captions-basic", "styles", "bold.json");
    const original = readFileSync(style);
    writeFileSync(style, original.toString("utf8").replace('"maxChars": 24', '"maxChars": 240'));
    // The next start: the server starts, the plugin is revoked, and the status says why.
    const again = f.service({ bundledRoot: root });
    const status = await run(again.plugins.syncBundled());
    expect(status.plugins).toEqual([{ id: ID, version: "1.0.0", ok: false, code: "PLUGIN_BUNDLED_MISMATCH" }]);
    // Revoking changes the state only: `enabled` stays the owner's choice (review M2).
    expect(again.tables.installedOne(ID)).toMatchObject({ enabled: true, state: "revoked" });
    expect((await run(again.h["plugin.list"]())).bundled?.plugins[0]).toMatchObject({ ok: false, code: "PLUGIN_BUNDLED_MISMATCH" });
    expect(await codeOf(again.life.enable(ID))).toBe("PLUGIN_REVOKED");
    expect(again.rows().find((r) => r.kind === "plugin.revoke")).toMatchObject({ principalKind: "system", code: "PLUGIN_BUNDLED_MISMATCH", details: { pluginId: ID } });
    // The files come back (a repaired app): it is restored.
    writeFileSync(style, original);
    const fixed = f.service({ bundledRoot: root });
    await run(fixed.plugins.syncBundled());
    expect(fixed.tables.installedOne(ID)).toMatchObject({ enabled: true, state: "active" });
    expect((await run(fixed.h["plugin.list"]())).bundled?.plugins[0]).toEqual({ id: ID, version: "1.0.0", ok: true });
  });

  test("revoke_and_restore_keep_an_owner_disabled_pack_off", async () => {
    const root = copyOfReal();
    const s = f.service({ bundledRoot: root });
    await run(s.plugins.syncBundled());
    await run(s.life.disable(ID));
    const style = join(root, "captions-basic", "styles", "bold.json");
    const original = readFileSync(style);
    writeFileSync(style, original.toString("utf8").replace('"maxChars": 24', '"maxChars": 240'));
    const again = f.service({ bundledRoot: root });
    await run(again.plugins.syncBundled());
    expect(again.tables.installedOne(ID)).toMatchObject({ enabled: false, state: "revoked" });
    writeFileSync(style, original);
    const fixed = f.service({ bundledRoot: root });
    await run(fixed.plugins.syncBundled());
    // Back as the owner left it: off, not switched on by the repair.
    expect(fixed.tables.installedOne(ID)).toMatchObject({ enabled: false, state: "disabled" });
    expect(fixed.rows().filter((r) => r.kind === "plugin.enable")).toEqual([]);
  });

  test("a_tampered_tree_is_never_installed", async () => {
    for (const how of ["changed", "added", "missing", "gone"] as const) {
      const root = copyOfReal();
      const dir = join(root, "captions-basic");
      if (how === "changed") writeFileSync(join(dir, "styles", "clean.json"), "{}");
      if (how === "added") writeFileSync(join(dir, "styles", "extra.json"), "{}");
      if (how === "missing") rmSync(join(dir, "styles", "punch.json"));
      if (how === "gone") rmSync(dir, { recursive: true });
      const s = f.service({ bundledRoot: root });
      const status = await run(s.plugins.syncBundled());
      expect(status.plugins, how).toEqual([{ id: ID, version: "1.0.0", ok: false, code: "PLUGIN_BUNDLED_MISMATCH" }]);
      expect(s.tables.installedOne(ID), how).toBeUndefined();
      expect(f.storeDirs(ID), how).toEqual([]);
      rmSync(join(f.serverData, "plugins"), { recursive: true, force: true });
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("the_plan_checks_the_compiled_in_hash_and_takes_no_other", async () => {
    const root = copyOfReal();
    const s = f.service();
    writeFileSync(join(root, "captions-basic", "styles", "bold.json"), "{}");
    const e = await s.native.plan(JSON.stringify({ id: ID, source: { kind: "bundled", root } })).then(() => undefined, (x: { code?: string }) => x.code);
    expect(e).toBe("PLUGIN_BUNDLED_MISMATCH");
    // A caller cannot name the hash to compare with, or a plugin that is not bundled.
    const named = await s.native.plan(JSON.stringify({ id: ID, source: { kind: "bundled", root, treeHash: hooks.treeHash(join(root, "captions-basic")) } })).then(() => undefined, (x: { code?: string }) => x.code);
    expect(named).toBe("INVALID_OP");
    const other = await s.native.plan(JSON.stringify({ id: "@acme/pack", source: { kind: "bundled", root } })).then(() => undefined, (x: { code?: string }) => x.code);
    expect(other).toBe("PLUGIN_NOT_FOUND");
    expect(readdirNames(join(f.serverData, "plugins", "staging"))).toEqual([]);
  });
});

function readdirNames(dir: string): string[] {
  return existsSync(dir) ? readdirSync(dir) : [];
}
