// The plugin store through fin-node (8A plan Task 8): plans from a package source (test builds
// only), commits, removals and the sweep, `validateManifest`, the worker thread, and the
// `plugin.lifecycle` journal entries (Task 11).
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { hostname, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import type { Envelope, ExportPage, InstallPlan, InstalledPlugin, ValidationReport } from "@finisher/core-types";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { type FinNodeError, Journal, Registry, testHooks, validateManifest } from "../index.js";
import { waitFor } from "./fontserver.ts";

const hooks = testHooks!;
let tmp: string;
let serverData: string;
let journals: Journal[] = [];

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "fin-node-registry-"));
  serverData = join(tmp, "server-data");
});

afterEach(async () => {
  for (const j of journals) await j.close();
  journals = [];
  rmSync(tmp, { recursive: true, force: true });
});

const ID = "@acme/fancy-captions";
const PUBLISHER = "Acme Studio";

function manifest(version = "1.0.0", extra: object = {}): object {
  return {
    manifestVersion: 1,
    id: ID,
    version,
    kind: "content",
    hostApi: "^1.0",
    license: "MIT",
    publisher: { name: PUBLISHER },
    contributes: { captionStyles: [{ name: "bold", file: "styles/bold.json" }] },
    ...extra,
  };
}

/** An unpacked plugin under `tmp/<name>` with `files` (path → bytes). */
function tree(name: string, m: object, files: Record<string, Buffer | string> = {}): string {
  const dir = join(tmp, name);
  const all: Record<string, Buffer | string> = {
    "finisher-plugin.json": JSON.stringify(m),
    "styles/bold.json": '{"text": {}}',
    ...files,
  };
  for (const [path, bytes] of Object.entries(all)) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), bytes);
  }
  return dir;
}

function pkg(name: string, m: object, files: Record<string, Buffer | string> = {}): string {
  const out = join(tmp, `${name}.finpkg`);
  hooks.packPlugin(tree(name, m, files), out);
  return out;
}

const planReq = (archive: string, o: object = {}, source: object = {}) =>
  JSON.stringify({ id: ID, source: { kind: "package", archive, registryId: "team-registry", ...source }, ...o });

async function rejection(p: Promise<unknown>): Promise<FinNodeError> {
  try {
    await p;
  } catch (e) {
    return e as FinNodeError;
  }
  throw new Error("expected a rejection");
}

async function entries(j: Journal, filter: object): Promise<Envelope[]> {
  const page = JSON.parse(await j.export(JSON.stringify(filter), undefined, 1000)) as ExportPage;
  return page.lines;
}

describe("Registry", () => {
  test("plan_commit_remove_round_trip", async () => {
    const r = Registry.open(serverData);
    const plan = JSON.parse(await r.plan(planReq(pkg("p1", manifest())))) as InstallPlan;
    expect(plan.plugin).toMatchObject({ id: ID, version: "1.0.0", kind: "content", publisher: PUBLISHER });
    expect(plan.source).toEqual({ kind: "registry", registryId: "team-registry" });
    expect(plan.action).toBe("install");
    expect(plan.expiresAt - Date.now()).toBeGreaterThan(9 * 60 * 1000);
    expect(existsSync(join(serverData, "plugins/staging", plan.planId, "plan.json"))).toBe(true);

    const wrong = await rejection(r.commit(plan.planId, "0".repeat(64)));
    expect([wrong.code, wrong.details.reason]).toEqual(["PLAN_MISMATCH", "digest"]);
    const got = JSON.parse(await r.commit(plan.planId, plan.digest)) as InstalledPlugin;
    expect(got).toMatchObject({ id: ID, version: "1.0.0", source: "registry", registryId: "team-registry" });
    expect(existsSync(join(serverData, "plugins", got.dir, "styles/bold.json"))).toBe(true);
    expect((await rejection(r.commit(plan.planId, plan.digest))).code).toBe("PLAN_EXPIRED");

    // An update against the installed version shows its widening.
    const v2 = manifest("1.1.0", { kind: "code", platforms: ["linux-x64", "darwin-arm64", "win32-x64"] });
    const code = {
      ...v2,
      contributes: undefined,
      runtime: { type: "node", main: "dist/plugin.mjs", node: ">=24.0" },
      permissions: [{ perm: "net", origins: ["https://api.example.org"] }],
    };
    const upd = JSON.parse(
      await r.plan(planReq(pkg("p2", code, { "dist/plugin.mjs": "export {}" }), { update: true, installed: got })),
    ) as InstallPlan;
    expect([upd.action, upd.from, upd.risk]).toEqual(["update", "1.0.0", "high"]);
    expect(upd.warnings?.[0]).toEqual({ code: "thirdPartyCode", details: { confined: false } });
    expect(upd.diff.added).toEqual([{ perm: "net", origins: ["https://api.example.org"] }]);
    expect((await rejection(r.commit(upd.planId, upd.digest))).code).toBe("THIRD_PARTY_CODE_NOT_ACCEPTED");
    await r.commit(upd.planId, upd.digest, JSON.stringify({ thirdPartyCode: true }));

    await r.removePlugin(ID, "1.0.0", false);
    expect(existsSync(join(serverData, "plugins", got.dir))).toBe(false);
    expect((await rejection(r.removePlugin(ID, "1.0.0", false))).code).toBe("PLUGIN_NOT_INSTALLED");
    await r.removePlugin(ID, "1.1.0", true);
    expect(await r.sweep()).toBe(0);
  });

  test("plans_refuse_bad_packages_and_sources", async () => {
    const r = Registry.open(serverData);
    const noRegistry = await rejection(
      r.plan(JSON.stringify({ id: ID, source: { kind: "registry", registry: "team" } })),
    );
    expect(noRegistry.code).toBe("REGISTRY_NOT_FOUND");
    const bad = await rejection(r.plan(planReq(pkg("p1", manifest("1.0.0", { license: "CC-BY-NC-4.0" })))));
    expect([bad.code, bad.details.reason]).toEqual(["PLUGIN_MANIFEST_INVALID", "licenceNonCommercial"]);
    expect((await rejection(r.plan("{}"))).code).toBe("INVALID_OP");
    const stale = JSON.parse(await r.plan(planReq(pkg("p2", manifest()), {}, { staleDays: 2 }))) as InstallPlan;
    expect(stale.registryStaleDays).toBe(2);
    expect(stale.warnings?.map((w) => w.code)).toEqual(["registryStale"]);
  });

  test("validate_manifest_offline", async () => {
    const ok = JSON.parse(await validateManifest(tree("ok", manifest()))) as ValidationReport;
    expect(ok.ok).toBe(true);
    expect(ok.manifest?.id).toBe(ID);
    const bad = JSON.parse(
      await validateManifest(tree("bad", manifest("1.0.0", { contributes: { skills: [{ file: "x" }] } }))),
    ) as ValidationReport;
    expect(bad.ok).toBe(false);
    expect(bad.errors.map((e) => e.code)).toContain("PLUGIN_CONTRIBUTION_UNSUPPORTED");
    const missing = JSON.parse(await validateManifest(join(tmp, "nothing"))) as ValidationReport;
    expect(missing.errors[0]?.details.reason).toBe("missing");
    // Reserved publishers are bundled-only.
    const reserved = tree("reserved", { ...manifest(), id: "@finisher/pack" });
    expect((JSON.parse(await validateManifest(reserved)) as ValidationReport).ok).toBe(false);
    expect((JSON.parse(await validateManifest(reserved, "bundled")) as ValidationReport).ok).toBe(true);
  });

  test("long_calls_do_not_block_event_loop", async () => {
    const r = Registry.open(serverData);
    const archive = pkg("big", manifest(), { "data/blob.bin": randomBytes(96 * 1024 * 1024) });
    let ticks = 0;
    const timer = setInterval(() => {
      ticks += 1;
    }, 1);
    const started = Date.now();
    const plan = JSON.parse(await r.plan(planReq(archive))) as InstallPlan;
    const elapsed = Date.now() - started;
    clearInterval(timer);
    expect(plan.bytes).toBeGreaterThan(96 * 1024 * 1024);
    // The loop kept turning while the plan copied, hashed and unpacked 96 MiB.
    expect(ticks).toBeGreaterThan(Math.min(10, elapsed / 4));
  }, 60_000);
});

describe("plugin.lifecycle", () => {
  test("lifecycle_events_hold_no_free_text", async () => {
    const j = Journal.open(serverData, null, hostname());
    journals.push(j);
    const r = Registry.open(serverData);
    const plan = JSON.parse(await r.plan(planReq(pkg("p1", manifest())))) as InstallPlan;
    await r.commit(plan.planId, plan.digest);
    await r.removePlugin(ID, "1.0.0", true);
    await rejection(r.plan(planReq(pkg("p2", manifest("2.0.0", { license: "CC-BY-NC-4.0" })))));
    // A commit of a plan that is gone: refused, and which plugin is unknown.
    await rejection(r.commit(plan.planId, plan.digest));
    const lines = await waitFor(async () => {
      const got = await entries(j, { kinds: ["plugin.lifecycle"] });
      return got.length >= 5 ? got : undefined;
    }, 10_000, "plugin.lifecycle");
    const payloads = lines.map((l) => l.payload as Record<string, unknown>);
    expect(payloads.map((p) => [p.action, p.outcome, p.code])).toEqual([
      ["plan", "ok", undefined],
      ["install", "ok", undefined],
      ["remove", "ok", undefined],
      ["plan", "refused", "PLUGIN_MANIFEST_INVALID"],
      ["install", "refused", "PLAN_EXPIRED"],
    ]);
    const gone = payloads.pop();
    expect(gone).toMatchObject({ kind: "unknown", risk: "unknown", source: "registry" });
    expect(gone?.plugin).toBeUndefined();
    for (const p of payloads) {
      expect(p.source).toBe("registry");
      expect(Object.keys(p.plugin as object)).toEqual(["hashed"]);
      expect((p.plugin as { hashed: string }).hashed).toMatch(/^k1:[0-9a-f]{32}$/);
    }
    expect(payloads[0]).toMatchObject({ kind: "content", risk: "low", permissionsAdded: 0 });
    expect(payloads[3]).toMatchObject({ kind: "unknown", risk: "unknown" });
    const text = JSON.stringify(lines);
    for (const leak of [ID, "acme", "fancy", PUBLISHER, "team-registry", tmp, "1.0.0", "2.0.0", ".finpkg"]) {
      expect(text, leak).not.toContain(leak);
    }
  });

  test("events_off_means_nothing_built", async () => {
    const j = Journal.open(serverData, JSON.stringify({ diagnostics: "off" }), hostname());
    journals.push(j);
    const r = Registry.open(serverData);
    const plan = JSON.parse(await r.plan(planReq(pkg("p1", manifest())))) as InstallPlan;
    await r.commit(plan.planId, plan.digest);
    await r.removePlugin(ID, "1.0.0", false);
    await rejection(r.plan(planReq(pkg("p2", manifest("2.0.0", { license: "CC-BY-NC-4.0" })))));
    expect(existsSync(join(serverData, "events"))).toBe(false);
  });
});
