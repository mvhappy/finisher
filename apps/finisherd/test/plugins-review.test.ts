// The review of 8A Task 9b: a registry id is not a trust root (M1), what non-human principals
// may cost (M2), an advisory between plan and install (M3), who sees locations (L1, L2), a
// removal whose folder is gone (L3), store folders without rows and rows without folders (L4),
// the audit details (L5), and the lifecycle's lock and rollbacks (L6).
import { existsSync, mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { FinError } from "@finisher/contracts";
import { Effect, Exit } from "effect";
import { describe, expect, test, vi } from "vitest";
import { scrubParts } from "../src/plugins/scrub.ts";
import { MAX_OPEN_PLANS } from "../src/plugins/plans.ts";
import { ReviewStore } from "../src/reviews/store.ts";
import { MAX_SUGGESTIONS } from "../src/reviews/suggestions.ts";
import { advisory, AGENT, as, CONSOLE, code, codeOf, failure, LOCAL, MCP, MINUTE, plugins, run, type Setup } from "./support/plugins-fixture.ts";

const f = plugins();

/** Plans `id` as the owner and installs it. */
async function install(s: Setup, id: string, o: { update?: boolean; registry?: string; autoUpdate?: boolean } = {}) {
  const plan = (await run(s.life.plan({ id, ...(o.update === true ? { update: true } : {}), ...(o.registry === undefined ? {} : { registry: o.registry }) }, CONSOLE))).plan;
  return run(s.life.install({ planId: plan.planId, digest: plan.digest, ...(o.autoUpdate === undefined ? {} : { autoUpdate: o.autoUpdate }) }, CONSOLE));
}

/** Removes registry `team` and adds `reg` under the same id, refreshed by the owner. */
async function swapRegistry(s: Setup, reg: { dir: string; rootSha256: string }) {
  await run(s.plugins.remove("team"));
  await run(s.plugins.add({ cfg: { id: "team", path: reg.dir }, root: { kind: "sha256", sha256: reg.rootSha256 } }, CONSOLE));
  await run(s.plugins.refresh("team", CONSOLE));
}

describe("M1: a registry id is not a trust root", () => {
  test("registry_id_reuse_takes_no_updates_from_the_new_root", async () => {
    const s = await f.setup({ packages: ["@acme/pack@1.0.0"] });
    await install(s, "@acme/pack", { autoUpdate: true });
    const oldRoot = s.reg.rootSha256;
    expect(s.tables.installedFull("@acme/pack")?.rootSha256).toBe(oldRoot);
    // Another folder, another root, the same id; it lists a newer pack and an advisory.
    const other = f.registry({ packages: ["@acme/pack@2.0.0"], advisories: [advisory("@acme/pack", "1.0.0")] });
    await swapRegistry(s, other);
    // A person's refresh: no automatic update, and the advisory of a root that never signed the package revokes nothing.
    const refreshed = await run(s.h["registry.refresh"]({}, as(CONSOLE)));
    expect(refreshed.applied).toEqual({ revoked: [], updated: [] });
    expect(s.tables.installedOne("@acme/pack")).toMatchObject({ version: "1.0.0", state: "active", enabled: true });
    // A manual plan has to name the registry; naming it carries the warning.
    const e = await failure(s.life.plan({ id: "@acme/pack", update: true }, CONSOLE));
    expect([e?.code, (e?.details as { reason?: string }).reason]).toEqual(["REGISTRY_AMBIGUOUS", "rootChanged"]);
    const named = (await run(s.life.plan({ id: "@acme/pack", update: true, registry: "team" }, CONSOLE))).plan;
    expect(named.warnings?.map((w) => w.code)).toContain("registryChanged");
    const done = await run(s.life.install({ planId: named.planId, digest: named.digest }, CONSOLE));
    expect(done.plugin.version).toBe("2.0.0");
    expect(s.tables.installedFull("@acme/pack")?.rootSha256).toBe(other.rootSha256);
    // Now it comes from the new root: updates follow it again.
    f.publish(other, { packages: ["@acme/pack@2.1.0"] });
    const plan = (await run(s.life.plan({ id: "@acme/pack", update: true }, CONSOLE))).plan;
    expect(plan.warnings?.map((w) => w.code) ?? []).not.toContain("registryChanged");
  });

  test("another_registry_named_by_the_person_warns", async () => {
    const s = await f.setup({ packages: ["@acme/pack@1.0.0"] });
    const second = f.registry({ packages: ["@acme/pack@1.1.0"] });
    await run(s.plugins.add({ cfg: { id: "second", path: second.dir }, root: { kind: "sha256", sha256: second.rootSha256 } }, CONSOLE));
    await run(s.plugins.refresh("second", CONSOLE));
    await install(s, "@acme/pack", { registry: "team" });
    expect(s.tables.installedFull("@acme/pack")?.registryId).toBe("team");
    f.publish(s.reg, { packages: ["@acme/pack@1.0.5"] });
    const same = (await run(s.life.plan({ id: "@acme/pack", update: true, registry: "team" }, CONSOLE))).plan;
    expect(same.warnings?.map((w) => w.code) ?? []).not.toContain("registryChanged");
    const other = (await run(s.life.plan({ id: "@acme/pack", update: true, registry: "second" }, CONSOLE))).plan;
    expect(other.warnings?.map((w) => w.code)).toContain("registryChanged");
    expect(s.tables.plan(other.planId)?.rootSha256).toBe(second.rootSha256);
  });

  test("plan_made_under_the_old_root_is_refused_after_the_swap", async () => {
    const s = await f.setup({ packages: ["@acme/pack@1.0.0"] });
    const plan = (await run(s.life.plan({ id: "@acme/pack" }, CONSOLE))).plan;
    await swapRegistry(s, f.registry({ packages: ["@acme/pack@1.0.0"] }));
    const e = await failure(s.life.install({ planId: plan.planId, digest: plan.digest }, CONSOLE));
    expect([e?.code, (e?.details as { reason?: string }).reason]).toEqual(["PLAN_MISMATCH", "registry"]);
    expect(s.tables.installed()).toEqual([]);
    expect(f.storeDirs("@acme/pack")).toEqual([]);
  });

  test("rows_made_before_the_root_column_take_their_registrys_root", async () => {
    const s = await f.setup({ packages: ["@acme/pack@1.0.0"] });
    await install(s, "@acme/pack");
    // The migration's backfill, as it ran on a row without the column.
    f.control.db.exec("UPDATE plugins SET rootSha256 = NULL");
    f.control.db.exec("UPDATE plugins SET rootSha256 = (SELECT r.rootSha256 FROM registries r WHERE r.id = plugins.registryId) WHERE registryId IS NOT NULL");
    expect(s.tables.installedFull("@acme/pack")?.rootSha256).toBe(s.reg.rootSha256);
    expect(s.tables.installedRootMatches("@acme/pack")).toBe(true);
  });
});

describe("M2: what non-human principals may cost", () => {
  test("agent_plan_leaves_nothing_staged", async () => {
    const s = await f.setup({ packages: ["@acme/pack@1.0.0"] });
    const staging = join(f.serverData, "plugins", "staging");
    for (const maker of [AGENT, LOCAL, MCP]) {
      const { plan } = await run(s.life.plan({ id: "@acme/pack" }, maker));
      expect(plan.plugin.id).toBe("@acme/pack");
      expect(existsSync(join(staging, plan.planId))).toBe(false);
      expect(s.tables.plan(plan.planId)).toMatchObject({ unusable: true, human: false });
    }
    expect(readdirOrEmpty(staging)).toEqual([]);
  });

  test("open_plans_are_capped_per_principal", async () => {
    const s = await f.setup({ packages: ["@acme/pack@1.0.0"] });
    for (let i = 0; i < MAX_OPEN_PLANS; i++) await run(s.life.plan({ id: "@acme/pack" }, CONSOLE));
    expect(await codeOf(s.life.plan({ id: "@acme/pack" }, CONSOLE))).toBe("TOO_MANY_PLANS");
    // Nobody else's plans count against it, and a refused one leaves no staging behind.
    await run(s.life.plan({ id: "@acme/pack" }, { kind: "device", id: "01device00000000000000000a", human: true }));
    expect(readdirOrEmpty(join(f.serverData, "plugins", "staging")).length).toBe(MAX_OPEN_PLANS + 1);
    // An agent is capped too, though its plans hold no files.
    for (let i = 0; i < MAX_OPEN_PLANS; i++) await run(s.life.plan({ id: "@acme/pack" }, AGENT));
    expect(await codeOf(s.life.plan({ id: "@acme/pack" }, AGENT))).toBe("TOO_MANY_PLANS");
    // Plans expire after ten minutes.
    f.now += 11 * MINUTE;
    await run(s.life.plan({ id: "@acme/pack" }, CONSOLE));
    await run(s.life.plan({ id: "@acme/pack" }, AGENT));
  });

  /** Review M3 of Tasks 10 and 12: the maker gives a plan up, and only the maker. */
  test("plan_discard_is_the_makers_and_frees_the_slot", async () => {
    const s = await f.setup({ packages: ["@acme/pack@1.0.0"] });
    const staging = join(f.serverData, "plugins", "staging");
    const plans: string[] = [];
    for (let i = 0; i < MAX_OPEN_PLANS; i++) plans.push((await run(s.life.plan({ id: "@acme/pack" }, CONSOLE))).plan.planId);
    expect(await codeOf(s.life.plan({ id: "@acme/pack" }, CONSOLE))).toBe("TOO_MANY_PLANS");
    // Anyone else, however privileged, gets the answer a missing plan gets (a plan id is not probed).
    const other = { kind: "device", id: "01device00000000000000000a", human: true };
    for (const stranger of [other, AGENT, LOCAL]) {
      expect(await codeOf(s.h["plugin.discard"]({ planId: plans[0]! }, as(stranger)))).toBe("PLAN_EXPIRED");
    }
    expect(s.tables.plan(plans[0]!)).toBeDefined();
    // The maker's discard deletes the row and the staged files, and a new plan fits.
    expect(await run(s.h["plugin.discard"]({ planId: plans[0]! }, as(CONSOLE)))).toEqual({ discarded: true });
    expect(s.tables.plan(plans[0]!)).toBeUndefined();
    expect(existsSync(join(staging, plans[0]!))).toBe(false);
    expect(readdirOrEmpty(staging).length).toBe(MAX_OPEN_PLANS - 1);
    expect(await codeOf(s.h["plugin.discard"]({ planId: plans[0]! }, as(CONSOLE)))).toBe("PLAN_EXPIRED");
    const again = (await run(s.life.plan({ id: "@acme/pack" }, CONSOLE))).plan;
    // A discarded plan installs nothing; the digest of a live one still does.
    expect(await codeOf(s.life.install({ planId: plans[0]!, digest: again.digest }, CONSOLE))).toBe("PLAN_EXPIRED");
    // An agent gives up its own plans as well.
    const mine = (await run(s.life.plan({ id: "@acme/pack" }, AGENT))).plan.planId;
    expect(await run(s.h["plugin.discard"]({ planId: mine }, as(AGENT)))).toEqual({ discarded: true });
    expect(s.tables.plan(mine)).toBeUndefined();
  });

  test("suggestions_are_deduplicated_and_capped", async () => {
    const s = f.service();
    const items = new ReviewStore(f.control);
    const P = "01aaaaaaaaaaaaaaaaaaaaaaaa";
    const Q = "01bbbbbbbbbbbbbbbbbbbbbbbb";
    const first = await run(s.life.suggest({ id: "@acme/pack", reason: "once", projectId: P }, AGENT));
    const again = await run(s.life.suggest({ id: "@acme/pack", reason: "twice", projectId: P }, AGENT));
    expect(again).toEqual(first);
    expect(items.list(P, { kind: "pluginSuggestion" }).length).toBe(1);
    // The same plugin for another project, or from another principal, is another suggestion.
    await run(s.life.suggest({ id: "@acme/pack", reason: "x", projectId: Q }, AGENT));
    await run(s.life.suggest({ id: "@acme/pack", reason: "x", projectId: P }, MCP));
    expect(items.list(P, { kind: "pluginSuggestion" }).length + items.list(Q, { kind: "pluginSuggestion" }).length).toBe(3);
    for (let i = 1; i < MAX_SUGGESTIONS; i++) await run(s.life.suggest({ id: `@acme/pack-${i + 10}`, reason: "x", projectId: P }, AGENT));
    expect(await codeOf(s.life.suggest({ id: "@acme/one-more", reason: "x", projectId: P }, AGENT))).toBe("TOO_MANY_SUGGESTIONS");
    await run(s.life.suggest({ id: "@acme/one-more", reason: "x", projectId: Q }, AGENT));
  });
});

describe("M3: an advisory between plan and install", () => {
  test("disable_advisory_after_the_plan_revokes_the_install", async () => {
    const s = await f.setup({ packages: ["@acme/pack@1.0.0"] });
    const plan = (await run(s.life.plan({ id: "@acme/pack" }, CONSOLE))).plan;
    expect(plan.warnings ?? []).toEqual([]);
    f.publish(s.reg, { advisories: [advisory("@acme/pack", "1.0.0")] });
    // Anyone's refresh brings the advisory in; nothing is installed yet to revoke.
    await run(s.h["registry.refresh"]({ id: "team" }, as(AGENT, ["plugin.read"])));
    const done = await run(s.life.install({ planId: plan.planId, digest: plan.digest }, CONSOLE));
    expect(done.plugin).toMatchObject({ id: "@acme/pack", version: "1.0.0", enabled: false, state: "revoked" });
    expect(s.tables.installedOne("@acme/pack")).toMatchObject({ enabled: false, state: "revoked" });
    expect(await codeOf(s.life.enable("@acme/pack"))).toBe("PLUGIN_REVOKED");
  });
});

describe("L2: no server paths for anyone but a person", () => {
  // A path inside JSON text has its backslashes escaped (Windows), so each path is also looked
  // for in its JSON form and with forward slashes.
  const forms = (p: string) => [p, JSON.stringify(p).slice(1, -1), p.replaceAll("\\", "/")];
  const leaks = (text: string, s: Setup) => [s.reg.dir, f.serverData, f.tmp].filter((p) => forms(p).some((x) => text.includes(x)));

  /** A registry whose config is corrupt: fin-registry's errors name its file, under the server data. */
  const corrupt = async () => {
    const s = await f.setup({ packages: ["@acme/pack@1.0.0"] });
    writeFileSync(join(f.serverData, "plugins", "registries", "team", "registry.json"), "{ not json");
    return s;
  };

  test("errors_name_no_location_for_non_humans", async () => {
    const s = await corrupt();
    const calls = (by: Parameters<typeof as>[0]) => ({
      refresh: s.h["registry.refresh"]({ id: "team" }, as(by, ["plugin.read"])),
      status: s.h["registry.status"]({ id: "team" }, as(by, ["plugin.read"])),
      plan: s.h["plugin.plan"]({ id: "@acme/pack", registry: "team" }, as(by, ["plugin.read"])),
    });
    // The person gets the error as it is: the file.
    for (const [name, call] of Object.entries(calls(CONSOLE))) {
      const e = await failure(call);
      expect(e?.code, name).toBe("REGISTRY_INVALID");
      expect(leaks(JSON.stringify(e), s), name).not.toEqual([]);
    }
    for (const who of [LOCAL, AGENT, MCP]) {
      for (const [name, call] of Object.entries(calls(who))) {
        const e = await failure(call);
        expect(e?.code, `${who.kind} ${name}`).toBe("REGISTRY_INVALID");
        expect(leaks(JSON.stringify(e), s), `${who.kind} ${name}`).toEqual([]);
        expect(e?.message).toMatch(/shown only to the server's owner/);
      }
      const all = await run(s.h["registry.refresh"]({}, as(who, ["plugin.read"])));
      expect(leaks(JSON.stringify(all), s), `${who.kind}: all`).toEqual([]);
    }
  });

  test("scrub_keeps_what_names_no_location", () => {
    const e = scrubParts({ code: "IO_ERROR", message: "open /srv/finisher/plugins/store/x: denied", details: { path: "/srv/x", reason: "denied", nested: { dir: "C:\\data\\x", n: 3 }, id: "@acme/pack" }, fix: "see C:\\data\\x" });
    expect(e.details).toEqual({ reason: "denied", nested: { n: 3 }, id: "@acme/pack" });
    expect(e.fix).toBeUndefined();
    expect(e.message).toMatch(/^IO_ERROR: /);
    const plain = scrubParts({ code: "PLUGIN_NOT_FOUND", message: "no registry lists @acme/pack", details: { id: "@acme/pack" }, fix: "Check the id" });
    expect(plain).toEqual({ code: "PLUGIN_NOT_FOUND", message: "no registry lists @acme/pack", details: { id: "@acme/pack" }, fix: "Check the id" });
  });

  test("a_corrupt_registry_is_listed_without_its_location", async () => {
    const s = await f.setup({ packages: ["@acme/pack@1.0.0"] });
    const file = join(f.serverData, "plugins", "registries", "team", "registry.json");
    expect(existsSync(file)).toBe(true);
    writeFileSync(file, "{ not json");
    const person = await run(s.h["registry.list"]({}, as(CONSOLE)));
    expect(person.problems.map((p) => p.id)).toEqual(["team"]);
    for (const who of [LOCAL, AGENT, MCP]) {
      const list = await run(s.h["registry.list"]({}, as(who, ["plugin.read"])));
      expect(list.problems.map((p) => [p.id, p.code]), who.kind).toEqual(person.problems.map((p) => [p.id, p.code]));
      expect(leaks(JSON.stringify(list), s), who.kind).toEqual([]);
    }
    expect(leaks(JSON.stringify(person), s).length).toBeGreaterThan(0);
  });
});

describe("L3 and L4: the store and the rows agree", () => {
  const dataDir = (id: string) => join(f.serverData, "plugins", "data", id.slice(0, id.indexOf("/")), id.slice(id.indexOf("/") + 1), "_");

  test("purge_removes_data_when_the_store_folder_is_gone", async () => {
    const s = await f.setup({ packages: ["@acme/pack@1.0.0"] });
    await install(s, "@acme/pack");
    mkdirSync(dataDir("@acme/pack"), { recursive: true });
    writeFileSync(join(dataDir("@acme/pack"), "state.json"), "{}");
    rmSync(join(f.serverData, "plugins", "store", "@acme"), { recursive: true });
    await run(s.life.remove("@acme/pack", true));
    expect(s.tables.installedOne("@acme/pack")).toBeUndefined();
    expect(existsSync(dataDir("@acme/pack"))).toBe(false);
  });

  test("an_orphan_folder_goes_at_start_and_stops_taking_font_ids", async () => {
    const s = await f.setup({ packages: [{ id: "@acme/fonts", version: "1.0.0", fonts: ["shared-sans"] }, { id: "@beta/fonts", version: "1.0.0", fonts: ["shared-sans"] }] });
    await install(s, "@acme/fonts");
    // A crash between placing the version and writing its row.
    f.control.db.exec("DELETE FROM plugins; DELETE FROM plugin_grants");
    expect(f.storeDirs("@acme/fonts").length).toBe(1);
    expect((await failure(s.life.plan({ id: "@beta/fonts" }, CONSOLE)))?.details).toMatchObject({ reason: "catalogIdTaken" });
    await run(s.plugins.reconcileStore());
    expect(f.storeDirs("@acme/fonts")).toEqual([]);
    await run(s.life.plan({ id: "@beta/fonts" }, CONSOLE));
  });

  test("a_row_without_its_folder_is_disabled_and_listed", async () => {
    const s = await f.setup({ packages: ["@acme/pack@1.0.0", "@acme/other@1.0.0"] });
    await install(s, "@acme/pack");
    await install(s, "@acme/other");
    rmSync(join(f.serverData, "plugins", "store", "@acme", "pack"), { recursive: true });
    await run(s.plugins.reconcileStore());
    expect(s.plugins.problems()).toEqual([{ id: "@acme/pack", version: "1.0.0", reason: "storeMissing" }]);
    expect(s.tables.installedOne("@acme/pack")).toMatchObject({ enabled: false, state: "disabled" });
    expect(s.tables.installedOne("@acme/other")).toMatchObject({ enabled: true, state: "active" });
    expect((await run(s.h["plugin.list"]())).problems).toEqual(s.plugins.problems());
    // It can still be removed.
    await run(s.life.remove("@acme/pack", false));
    expect(s.tables.installedOne("@acme/pack")).toBeUndefined();
  });
});

describe("L5: audit details", () => {
  test("automatic_updates_say_so_and_failed_installs_name_their_plan", async () => {
    const s = await f.setup({ packages: ["@acme/pack@1.0.0"] });
    await install(s, "@acme/pack", { autoUpdate: true });
    f.publish(s.reg, { packages: ["@acme/pack@1.1.0"] });
    await run(s.h["registry.refresh"]({}, as(CONSOLE)));
    const update = s.rows().find((r) => r.kind === "plugin.update");
    expect(update?.details).toMatchObject({ pluginId: "@acme/pack", version: "1.1.0", auto: true });
    f.publish(s.reg, { packages: ["@acme/tool@1.0.0"] });
    await run(s.plugins.refresh("team", CONSOLE));
    const plan = (await run(s.life.plan({ id: "@acme/tool" }, CONSOLE))).plan;
    await failure(s.h["plugin.install"]({ planId: plan.planId, digest: "0".repeat(64) }, as(CONSOLE)));
    const failed = s.rows().filter((r) => r.kind === "plugin.install" && r.outcome !== "ok");
    expect(failed.at(-1)?.details).toEqual({ code: "PLAN_MISMATCH", planId: plan.planId, pluginId: "@acme/tool" });
    expect(s.audit.failures()).toBe(0);
  });
});

describe("L6: the lifecycle lock and its rollbacks", () => {
  test("a_failed_row_transaction_takes_the_placed_version_back", async () => {
    const s = await f.setup({ packages: ["@acme/pack@1.0.0"] });
    const plan = (await run(s.life.plan({ id: "@acme/pack" }, CONSOLE))).plan;
    const before = f.snapshot();
    const tx = vi.spyOn(s.tables.store, "tx").mockImplementationOnce(() => {
      throw new Error("disk full");
    });
    const e = await failure(s.life.install({ planId: plan.planId, digest: plan.digest }, CONSOLE));
    expect(tx).toHaveBeenCalledTimes(1);
    expect(e?.code).toBe("INTERNAL");
    expect(f.snapshot()).toEqual(before);
    expect(f.storeDirs("@acme/pack")).toEqual([]);
    // Nothing is stuck: a new plan installs.
    await install(s, "@acme/pack");
    expect(f.storeDirs("@acme/pack").length).toBe(1);
    tx.mockRestore();
  });

  test("install_remove_and_refresh_interleave_under_the_lock", async () => {
    const s = await f.setup({ packages: ["@acme/alpha@1.0.0", "@acme/beta@1.0.0", "@acme/gamma@1.0.0"] });
    await install(s, "@acme/alpha");
    const pb = (await run(s.life.plan({ id: "@acme/beta" }, CONSOLE))).plan;
    const pc = (await run(s.life.plan({ id: "@acme/gamma" }, CONSOLE))).plan;
    f.publish(s.reg, { advisories: [advisory("@acme/gamma", "1.0.0")] });
    const act: Array<Effect.Effect<unknown, FinError>> = [
      s.life.install({ planId: pb.planId, digest: pb.digest }, CONSOLE),
      s.life.install({ planId: pb.planId, digest: pb.digest }, CONSOLE),
      s.life.remove("@acme/alpha", false),
      s.life.install({ planId: pc.planId, digest: pc.digest }, CONSOLE),
      s.plugins.refresh("team", CONSOLE),
      s.life.afterRefresh("team", CONSOLE),
    ];
    const exits = await Promise.all(act.map((e) => Effect.runPromiseExit(e)));
    const codes = exits.map((x) => (Exit.isSuccess(x) ? "ok" : ((Exit.findErrorOption(x) as { value?: { code?: string } }).value?.code ?? "failed")));
    expect(codes.filter((c) => c === "ok").length).toBeGreaterThanOrEqual(4);
    // Exactly one of the two installs of the same plan won; the other found it used up.
    const dup = [codes[0], codes[1]].sort();
    expect(dup).toEqual(["PLAN_EXPIRED", "ok"]);
    expect(s.tables.installed().map((p) => p.id)).toEqual(["@acme/beta", "@acme/gamma"]);
    expect(f.storeDirs("@acme/alpha")).toEqual([]);
    expect(f.storeDirs("@acme/beta").length).toBe(1);
    expect(f.storeDirs("@acme/gamma").length).toBe(1);
    // The advisory for c landed before or after its install: either way c ends up revoked.
    expect(s.tables.installedOne("@acme/gamma")).toMatchObject({ enabled: false, state: "revoked" });
    // Rows and folders agree.
    await run(s.plugins.reconcileStore());
    expect(s.plugins.problems()).toEqual([]);
    expect(f.storeDirs("@acme/alpha")).toEqual([]);
    expect(f.snapshot().grants).toEqual([]);
  });
});

function readdirOrEmpty(dir: string): string[] {
  return existsSync(dir) ? readdirSync(dir) : [];
}
