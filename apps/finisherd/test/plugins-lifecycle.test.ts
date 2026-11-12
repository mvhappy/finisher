// The plugin lifecycle (8A plan Task 9b): plans from verified registry targets, the owner's
// approval of a person's plan, updates (automatic only for a person's refresh of content that
// widens nothing), enable, disable, remove, advisories, suggestions and the audit log, over
// fin-node's `Registry` and the `plugins` chain of `control.db`.
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type FinError, type Principal, requiredScopes, SCOPES, type Scope } from "@finisher/contracts";
import { Registry, testHooks } from "@finisher/fin-node";
import { Effect, Exit } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { checkScopes } from "../src/chain/authorize.ts";
import { withChain } from "../src/chain/chain.ts";
import { Connections } from "../src/connections.ts";
import { Audit, type AuditInput } from "../src/control/audit.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { CONSOLE as CONSOLE_P, LOCAL as LOCAL_P } from "../src/principal.ts";
import { makePlugins, type MakePluginsOptions } from "../src/plugins/registries.ts";
import { pluginHandlers } from "../src/plugins/rpc.ts";
import { ReviewStore } from "../src/reviews/store.ts";
import { MAX_SUGGESTIONS, moveSuggestionRows, pluginSuggestions } from "../src/reviews/suggestions.ts";
import { PluginTables } from "../src/plugins/tables.ts";
import { SAMPLE_PAYLOADS } from "./support/payloads.ts";

const hooks = testHooks!;
const CONSOLE = { kind: "console", id: "console", human: true } as const;
/** An owner's paired device (5B): a person, holding the owner's scopes. */
const DEVICE = { kind: "device", id: "01device00000000000000000a", human: true } as const;
const LOCAL = { kind: "local", id: "local", human: false } as const;
const AGENT = { kind: "agent", id: "01agent000000000000000000a", human: false } as const;
const MCP = { kind: "mcp", id: "01mcp00000000000000000000a", human: false } as const;
type Who = { readonly kind: string; readonly id: string; readonly human: boolean };
const MINUTE = 60_000;

let tmp: string;
let serverData: string;
let control: ControlStore;
let now: number;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-lifecycle-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true });
  control = openControlStore(serverData, { serverId: "sid-1", checkpoint: false });
  now = Date.now();
});

afterEach(async () => {
  await control.close();
  rmSync(tmp, { recursive: true, force: true });
});

interface Built {
  readonly dir: string;
  readonly keys: string;
  readonly rootSha256: string;
}

let built = 0;
function registry(spec: object): Built {
  const dir = join(tmp, `registry-${++built}`);
  const keys = join(tmp, `keys-${built}`);
  const r = JSON.parse(hooks.buildRegistry(dir, keys, JSON.stringify(spec))) as { rootSha256: string };
  return { dir, keys, rootSha256: r.rootSha256 };
}
const publish = (r: Built, spec: object) => hooks.publishToRegistry(r.dir, r.keys, JSON.stringify(spec));

const code = (id: string, version: string, permissions: object[] = [{ perm: "project:read" }]) => ({ id, version, kind: "code", permissions });
const NET = { perm: "net", origins: ["https://api.example.org"] };
const advisory = (plugin: string, versions: string, action: "warn" | "disable" = "disable") => ({ id: `ADV-${plugin.length}-${versions.length}`, plugin, versions, severity: "high", action, summary: "a test advisory" });

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);
async function failure(e: Effect.Effect<unknown, FinError>): Promise<FinError | undefined> {
  const exit = await Effect.runPromiseExit(e);
  return Exit.isSuccess(exit) ? undefined : (Exit.findErrorOption(exit) as { value?: FinError }).value;
}
const codeOf = async (e: Effect.Effect<unknown, FinError>) => (await failure(e))?.code;

function service(options: MakePluginsOptions = {}) {
  const native = Registry.open(serverData);
  const tables = new PluginTables(control);
  const plugins = makePlugins(native, tables, () => now, options);
  const audit = new Audit(control, () => now);
  const h = pluginHandlers({ plugins, audit, guard: (_tag, self) => self });
  const rows = () =>
    (control.db.prepare("SELECT kind, principalKind, outcome, code, details FROM audit WHERE kind LIKE 'plugin.%' OR kind LIKE 'registry.%' ORDER BY id").all() as Array<{ kind: string; principalKind: string; outcome: string; code: string | null; details: string }>).map((r) => ({
      ...r,
      details: JSON.parse(r.details) as Record<string, unknown>,
    }));
  return { native, tables, plugins, life: plugins.lifecycle, audit, h, rows };
}
type Service = ReturnType<typeof service>;

/** A service with registry `team` built from `spec`, added by the owner and refreshed. */
async function setup(spec: object, options: MakePluginsOptions = {}): Promise<Service & { reg: Built }> {
  const reg = registry(spec);
  const s = service(options);
  await run(s.plugins.add({ cfg: { id: "team", path: reg.dir }, root: { kind: "sha256", sha256: reg.rootSha256 } }, CONSOLE));
  await run(s.plugins.refresh("team", CONSOLE));
  return { ...s, reg };
}

const as = (p: Who, scopes: ReadonlyArray<Scope> = []) => ({ headers: {}, guard: { denyBranches: [] }, principal: { ...p, scopes, projects: "*", branches: "*" } });

/** Plans `id` as `by` and installs it as the owner. */
async function install(s: Service, id: string, o: { by?: Who; accept?: object; autoUpdate?: boolean; update?: boolean; version?: string } = {}) {
  const plan = (await run(s.life.plan({ id, ...(o.update === true ? { update: true } : {}), ...(o.version === undefined ? {} : { version: o.version }) }, o.by ?? CONSOLE))).plan;
  return run(s.life.install({ planId: plan.planId, digest: plan.digest, ...(o.accept === undefined ? {} : { accept: o.accept }), ...(o.autoUpdate === undefined ? {} : { autoUpdate: o.autoUpdate }) }, CONSOLE));
}

/** `control.db`'s plugin rows and grants, for "nothing changed" checks. */
const snapshot = () => ({
  plugins: control.db.prepare("SELECT * FROM plugins ORDER BY id").all(),
  grants: control.db.prepare("SELECT pluginId, version, permission FROM plugin_grants ORDER BY rowid").all(),
});
const storeDirs = (id: string) => {
  const [pub, name] = id.slice(1).split("/");
  const d = join(serverData, "plugins", "store", `@${pub}`, name!);
  return existsSync(d) ? readdir(d) : [];
};
const readdir = (d: string) => readdirSync(d).filter((n) => !n.startsWith(".")).sort();

describe("plans", () => {
  test("plan_comes_from_a_verified_registry_target", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0", "@acme/pack@1.1.0"] });
    const { plan } = await run(s.life.plan({ id: "@acme/pack" }, CONSOLE));
    expect(plan).toMatchObject({ plugin: { id: "@acme/pack", version: "1.1.0", kind: "content" }, source: { kind: "registry", registryId: "team" }, action: "install" });
    expect(plan.targetSha256).toMatch(/^[0-9a-f]{64}$/);
    // An exact version, and one no registry signs.
    expect((await run(s.life.plan({ id: "@acme/pack", version: "1.0.0" }, CONSOLE))).plan.plugin.version).toBe("1.0.0");
    expect(await codeOf(s.life.plan({ id: "@acme/pack", version: "9.9.9" }, CONSOLE))).toBe("PLUGIN_NOT_FOUND");
    expect(await codeOf(s.life.plan({ id: "@acme/none" }, CONSOLE))).toBe("PLUGIN_NOT_FOUND");
    expect(await codeOf(s.life.plan({ id: "@acme/pack", registry: "nope" }, CONSOLE))).toBe("REGISTRY_NOT_FOUND");
    // A target whose bytes were swapped in the folder after it was signed is refused.
    const info = JSON.parse(await s.native.info("@acme/pack")) as Array<{ versions: Array<{ version: string; target: string; sha256: string }> }>;
    const v = info[0]!.versions.find((x) => x.version === "1.0.0")!;
    const [dir, base] = [v.target.slice(0, v.target.lastIndexOf("/")), v.target.slice(v.target.lastIndexOf("/") + 1)];
    const file = join(s.reg.dir, "targets", dir, `${v.sha256}.${base}`);
    const bytes = readFileSync(file);
    bytes[bytes.length - 1] = bytes[bytes.length - 1]! ^ 0xff;
    writeFileSync(file, bytes);
    expect(await codeOf(s.life.plan({ id: "@acme/pack", version: "1.0.0" }, CONSOLE))).toBe("TUF_TARGET_MISMATCH");
  });

  test("plan_names_its_registry_when_several_list_it", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0"] });
    const other = registry({ packages: ["@acme/pack@2.0.0"] });
    await run(s.plugins.add({ cfg: { id: "other", path: other.dir }, root: { kind: "sha256", sha256: other.rootSha256 } }, CONSOLE));
    await run(s.plugins.refresh("other", CONSOLE));
    const e = await failure(s.life.plan({ id: "@acme/pack" }, CONSOLE));
    expect([e?.code, e?.details]).toEqual(["REGISTRY_AMBIGUOUS", { id: "@acme/pack", registries: ["other", "team"] }]);
    expect((await run(s.life.plan({ id: "@acme/pack", registry: "other" }, CONSOLE))).plan.plugin.version).toBe("2.0.0");
    // Installed from one, its updates come from that one.
    const fromTeam = (await run(s.life.plan({ id: "@acme/pack", registry: "team" }, CONSOLE))).plan;
    await run(s.life.install({ planId: fromTeam.planId, digest: fromTeam.digest }, CONSOLE));
    expect(await codeOf(s.life.plan({ id: "@acme/pack", update: true }, CONSOLE))).toBe("PLUGIN_ALREADY_INSTALLED");
  });

  test("plan_maker_recorded_and_enforced", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0"] });
    const before = snapshot();
    for (const maker of [AGENT, LOCAL, MCP]) {
      const { plan } = await run(s.life.plan({ id: "@acme/pack" }, maker));
      expect(s.tables.plan(plan.planId)).toMatchObject({ byKind: maker.kind, byId: maker.id, human: false, base: null, pluginId: "@acme/pack", action: "install" });
      // A person submitting it does not make it a person's plan: refused before any commit.
      const e = await failure(s.life.install({ planId: plan.planId, digest: plan.digest }, CONSOLE));
      expect([e?.code, (e?.details as { reason?: string }).reason]).toEqual(["HUMAN_ONLY", "planMaker"]);
      expect(snapshot()).toEqual(before);
      // The row is kept so an approval is refused for what it is; the staged files are gone (M2).
      expect(s.tables.plan(plan.planId)).toMatchObject({ unusable: true });
      expect(await s.native.stagedPlan(plan.planId).then(() => "staged", (e: { code?: string }) => e.code)).toBe("PLAN_EXPIRED");
    }
    expect(readdirSync(join(serverData, "plugins", "staging"))).toEqual([]);
    const mine = (await run(s.life.plan({ id: "@acme/pack" }, CONSOLE))).plan;
    expect(s.tables.plan(mine.planId)).toMatchObject({ byKind: "console", byId: "console", human: true });
    expect((await run(s.life.install({ planId: mine.planId, digest: mine.digest }, CONSOLE))).plugin).toMatchObject({ id: "@acme/pack", version: "1.0.0", installedBy: "console:console" });
    expect(s.tables.plan(mine.planId)).toBeUndefined();
  });

  test("agent_plan_refused_for_human_submit", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0"] });
    const plan = await run(s.h["plugin.plan"]({ id: "@acme/pack" }, as(AGENT, ["plugin.read"])));
    const e = await failure(s.h["plugin.install"]({ planId: plan.planId, digest: plan.digest }, as(CONSOLE)));
    expect(e?.code).toBe("HUMAN_ONLY");
    expect(e?.fix).toMatch(/plan you made/);
    expect(s.tables.installed()).toEqual([]);
    expect(s.rows().filter((r) => r.kind === "plugin.install")).toEqual([{ kind: "plugin.install", principalKind: "console", outcome: "denied", code: "HUMAN_ONLY", details: { code: "HUMAN_ONLY", planId: plan.planId, pluginId: "@acme/pack" } }]);
  });

  test("digest_must_match", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0"] });
    const { plan } = await run(s.life.plan({ id: "@acme/pack" }, CONSOLE));
    const e = await failure(s.life.install({ planId: plan.planId, digest: "0".repeat(64) }, CONSOLE));
    expect([e?.code, (e?.details as { reason?: string }).reason]).toEqual(["PLAN_MISMATCH", "digest"]);
    expect(s.tables.installed()).toEqual([]);
    // The plan is still there for its own digest, and is used up by it.
    await run(s.life.install({ planId: plan.planId, digest: plan.digest }, CONSOLE));
    expect(await codeOf(s.life.install({ planId: plan.planId, digest: plan.digest }, CONSOLE))).toBe("PLAN_EXPIRED");
  });

  test("plan_expires", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0"] });
    const { plan } = await run(s.life.plan({ id: "@acme/pack" }, CONSOLE));
    const other = (await run(s.life.plan({ id: "@acme/pack" }, CONSOLE))).plan;
    now += 11 * MINUTE;
    const e = await failure(s.life.install({ planId: plan.planId, digest: plan.digest }, CONSOLE));
    expect([e?.code, (e?.details as { reason?: string }).reason]).toEqual(["PLAN_EXPIRED", "expired"]);
    expect(s.tables.plan(plan.planId)).toBeUndefined();
    expect(existsSync(join(serverData, "plugins", "staging", plan.planId))).toBe(false);
    // The sweep takes the other expired plan's row (its staging expires on fin-registry's clock).
    expect(s.tables.plan(other.planId)).toBeDefined();
    await run(s.life.sweep());
    expect(s.tables.plan(other.planId)).toBeUndefined();
    expect(s.tables.installed()).toEqual([]);
  });

  test("downgrade_needs_named_version_and_human", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0", "@acme/pack@1.1.0"] });
    await install(s, "@acme/pack");
    expect(s.tables.installedOne("@acme/pack")?.version).toBe("1.1.0");
    expect(await codeOf(s.life.plan({ id: "@acme/pack", update: true, version: "1.0.0" }, CONSOLE))).toBe("PLUGIN_DOWNGRADE_REFUSED");
    expect(await codeOf(s.life.plan({ id: "@acme/pack", update: true, allowDowngrade: true }, CONSOLE))).toBe("PLUGIN_DOWNGRADE_REFUSED");
    expect(await codeOf(s.life.plan({ id: "@acme/pack", update: true, allowDowngrade: true, version: "1.0.0" }, LOCAL))).toBe("HUMAN_ONLY");
    // Through the chain: a downgrade needs the owner's scope, which only a person holds.
    expect(requiredScopes("plugin.plan", { id: "@acme/pack", allowDowngrade: true, version: "1.0.0" })).toEqual(["plugin.read", "plugin.install"]);
    expect(checkScopes("plugin.plan", requiredScopes("plugin.plan", { allowDowngrade: true }), LOCAL_P)?.code).toBe("HUMAN_ONLY");
    expect(requiredScopes("plugin.plan", { id: "@acme/pack", version: "1.0.0" })).toEqual(["plugin.read"]);
    const { plan } = await run(s.life.plan({ id: "@acme/pack", update: true, allowDowngrade: true, version: "1.0.0" }, CONSOLE));
    expect([plan.action, plan.from, plan.plugin.version]).toEqual(["downgrade", "1.1.0", "1.0.0"]);
    expect((await run(s.life.install({ planId: plan.planId, digest: plan.digest }, CONSOLE))).action).toBe("downgrade");
    expect(s.tables.installedOne("@acme/pack")?.version).toBe("1.0.0");
    expect(storeDirs("@acme/pack").map((d) => d.split("-")[0])).toEqual(["1.0.0"]);
  });
});

describe("approval", () => {
  test("commit_rechecks_installed_state", async () => {
    const s = await setup({
      packages: ["@acme/pack@1.0.0", code("@acme/tool", "1.0.0"), code("@acme/tool", "1.1.0"), { id: "@acme/fonts", version: "1.0.0", fonts: ["shared-sans"] }, { id: "@beta/fonts", version: "1.0.0", fonts: ["shared-sans"] }],
    });
    const third = { thirdPartyCode: true };
    // The installed version changed: another plan of the same install went first.
    const a = (await run(s.life.plan({ id: "@acme/pack" }, CONSOLE))).plan;
    const b = (await run(s.life.plan({ id: "@acme/pack" }, CONSOLE))).plan;
    await run(s.life.install({ planId: a.planId, digest: a.digest }, CONSOLE));
    let before = snapshot();
    let e = await failure(s.life.install({ planId: b.planId, digest: b.digest }, CONSOLE));
    expect([e?.code, (e?.details as { reason?: string }).reason]).toEqual(["PLAN_MISMATCH", "installed"]);
    expect(snapshot()).toEqual(before);
    // The grants changed between the update's plan and its approval.
    await install(s, "@acme/tool", { accept: third, version: "1.0.0" });
    const upd = (await run(s.life.plan({ id: "@acme/tool", update: true }, CONSOLE))).plan;
    s.tables.setGrants("@acme/tool", "1.0.0", [JSON.stringify(NET)], "console:console", now);
    before = snapshot();
    e = await failure(s.life.install({ planId: upd.planId, digest: upd.digest, accept: third }, CONSOLE));
    expect([e?.code, (e?.details as { reason?: string }).reason]).toEqual(["PLAN_MISMATCH", "installed"]);
    expect(snapshot()).toEqual(before);
    expect(storeDirs("@acme/tool").map((d) => d.split("-")[0])).toEqual(["1.0.0"]);
    // A font id was taken by a pack installed since the plan was made.
    const fa = (await run(s.life.plan({ id: "@acme/fonts" }, CONSOLE))).plan;
    const fb = (await run(s.life.plan({ id: "@beta/fonts" }, CONSOLE))).plan;
    await run(s.life.install({ planId: fa.planId, digest: fa.digest }, CONSOLE));
    before = snapshot();
    e = await failure(s.life.install({ planId: fb.planId, digest: fb.digest }, CONSOLE));
    expect([e?.code, e?.details]).toEqual(["PLAN_MISMATCH", { reason: "installed", field: "catalogIds", rule: "catalogIdTaken", entry: "shared-sans" }]);
    expect(snapshot()).toEqual(before);
    expect(storeDirs("@beta/fonts")).toEqual([]);
    // Planned now, it is refused at plan time.
    expect((await failure(s.life.plan({ id: "@beta/fonts" }, CONSOLE)))?.details).toMatchObject({ reason: "catalogIdTaken" });
  });

  test("approval_matrix", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0", code("@acme/tool", "1.0.0")] });
    const owner = (p: Who): Principal => ({ ...p, kind: p.kind as Principal["kind"], scopes: p.human ? [...SCOPES] : [...SCOPES].filter((x) => x !== "plugin.install" && x !== "plugin.admin"), projects: "*", branches: "*" });
    const approvers: Who[] = [CONSOLE, DEVICE, LOCAL, AGENT];
    const makers: Who[] = [CONSOLE, AGENT];
    const risks = [
      { id: "@acme/pack", risk: "low", accept: {} },
      { id: "@acme/tool", risk: "high", accept: { thirdPartyCode: true } },
    ] as const;
    const outcomes: string[] = [];
    for (const r of risks) {
      for (const maker of makers) {
        for (const approver of approvers) {
          // Plans a person did not use stay open for ten minutes: the cap is tested on its own.
          control.db.exec("DELETE FROM plugin_plans");
          const { plan } = await run(s.life.plan({ id: r.id }, maker));
          expect(plan.risk).toBe(r.risk);
          // The chain refuses a non-human approver first; the lifecycle refuses it again.
          const chain = checkScopes("plugin.install", requiredScopes("plugin.install", { planId: plan.planId, digest: plan.digest }), owner(approver))?.code;
          const got = await failure(s.life.install({ planId: plan.planId, digest: plan.digest, accept: r.accept }, approver));
          outcomes.push(`${r.risk} ${maker.kind}→${approver.kind}: ${chain ?? "pass"}/${got?.code ?? "ok"}`);
          if (got === undefined) await run(s.life.remove(r.id, false));
        }
      }
    }
    expect(outcomes).toEqual(
      ["low", "high"].flatMap((risk) => [
        `${risk} console→console: pass/ok`,
        `${risk} console→device: pass/ok`,
        `${risk} console→local: HUMAN_ONLY/HUMAN_ONLY`,
        `${risk} console→agent: HUMAN_ONLY/HUMAN_ONLY`,
        `${risk} agent→console: pass/HUMAN_ONLY`,
        `${risk} agent→device: pass/HUMAN_ONLY`,
        `${risk} agent→local: HUMAN_ONLY/HUMAN_ONLY`,
        `${risk} agent→agent: HUMAN_ONLY/HUMAN_ONLY`,
      ]),
    );
  });

  test("fin_install_is_human_only_for_local_and_agent", async () => {
    const tags = ["plugin.install", "plugin.remove", "plugin.enable", "plugin.disable"] as const;
    for (const tag of tags) {
      for (const p of [LOCAL_P, { ...LOCAL_P, kind: "agent" as const, id: AGENT.id, scopes: [...SCOPES] }, { ...LOCAL_P, kind: "mcp" as const, id: MCP.id, scopes: [...SCOPES] }]) {
        const seen: string[] = [];
        const audited: AuditInput[] = [];
        const connections = new Connections("socket", () => "/sd/run/fin.sock");
        connections.attach(1, { close() {}, closeAfterReply() {}, terminate() {} });
        connections.get(1)!.principal = p;
        const audit = { append: (e: AuditInput) => audited.push(e) } as unknown as Audit;
        const projects = { isOpen: () => false, dirOf: () => undefined, resolveBranch: () => Effect.succeed("main"), branches: () => Effect.succeed([]) };
        const chained = withChain({ [tag]: () => Effect.sync(() => (seen.push(tag), {})) }, { connections, projects, audit }) as unknown as Record<string, (p: unknown, o: unknown) => Effect.Effect<unknown, FinError>>;
        expect(await codeOf(chained[tag]!(SAMPLE_PAYLOADS[tag], { client: { id: 1 }, headers: {} })), `${tag} × ${p.kind}`).toBe("HUMAN_ONLY");
        expect(seen).toEqual([]);
      }
    }
    // The console's credential passes the scopes of each.
    for (const tag of tags) expect(checkScopes(tag, requiredScopes(tag, SAMPLE_PAYLOADS[tag]), CONSOLE_P)).toBeUndefined();
  });

  test("third_party_code_needs_acceptance", async () => {
    const s = await setup({ packages: [code("@acme/tool", "1.0.0")] });
    const plan = await run(s.h["plugin.plan"]({ id: "@acme/tool" }, as(CONSOLE)));
    expect(plan.risk).toBe("high");
    expect(plan.warnings?.[0]).toEqual({ code: "thirdPartyCode", details: { confined: false } });
    for (const accept of [undefined, {}, { copyleft: true }, { thirdPartyCode: false }]) {
      const e = await failure(s.h["plugin.install"]({ planId: plan.planId, digest: plan.digest, ...(accept === undefined ? {} : { accept }) }, as(CONSOLE)));
      expect(e?.code).toBe("THIRD_PARTY_CODE_NOT_ACCEPTED");
      expect(s.tables.installed()).toEqual([]);
    }
    const done = await run(s.h["plugin.install"]({ planId: plan.planId, digest: plan.digest, accept: { thirdPartyCode: true } }, as(CONSOLE)));
    expect(done).toMatchObject({ action: "install", plugin: { id: "@acme/tool", kind: "code" } });
    const installs = s.rows().filter((r) => r.kind === "plugin.install");
    expect(installs.map((r) => [r.outcome, r.code])).toEqual([...Array(4).fill(["denied", "THIRD_PARTY_CODE_NOT_ACCEPTED"]), ["ok", null]]);
    expect(installs.at(-1)?.details).toEqual({ pluginId: "@acme/tool", version: "1.0.0", registryId: "team", permissionsAdded: 1, risk: "high", thirdPartyCode: { accepted: true, confined: false } });
    expect(s.rows().find((r) => r.kind === "plugin.grant")?.details).toEqual({ pluginId: "@acme/tool", version: "1.0.0", registryId: "team", permissionsAdded: 1 });
  });

  test("code_plugin_installs_disabled", async () => {
    const s = await setup({ packages: [code("@acme/tool", "1.0.0"), "@acme/pack@1.0.0"] });
    const tool = await install(s, "@acme/tool", { accept: { thirdPartyCode: true }, autoUpdate: true });
    expect(tool.plugin).toMatchObject({ kind: "code", enabled: false, state: "disabled", autoUpdate: false });
    expect(s.tables.grants("@acme/tool")).toEqual(['{"perm":"project:read"}']);
    // A content pack is active at install.
    expect((await install(s, "@acme/pack")).plugin).toMatchObject({ kind: "content", enabled: true, state: "active" });
  });

  test("enable_code_plugin_is_runtime_unavailable", async () => {
    const s = await setup({ packages: [code("@acme/tool", "1.0.0"), "@acme/pack@1.0.0"] });
    await install(s, "@acme/tool", { accept: { thirdPartyCode: true } });
    await install(s, "@acme/pack");
    const e = await failure(s.h["plugin.enable"]({ id: "@acme/tool" }, as(CONSOLE)));
    expect(e?.code).toBe("PLUGIN_RUNTIME_UNAVAILABLE");
    expect(JSON.stringify(e)).not.toContain("SANDBOX");
    expect(s.tables.installedOne("@acme/tool")).toMatchObject({ enabled: false, state: "disabled" });
    // Content packs turn off and on; an unknown id is not installed.
    expect((await run(s.h["plugin.disable"]({ id: "@acme/pack" }, as(CONSOLE)))).plugin).toMatchObject({ enabled: false, state: "disabled" });
    expect((await run(s.h["plugin.enable"]({ id: "@acme/pack" }, as(CONSOLE)))).plugin).toMatchObject({ enabled: true, state: "active" });
    expect(await codeOf(s.h["plugin.enable"]({ id: "@acme/none" }, as(CONSOLE)))).toBe("PLUGIN_NOT_INSTALLED");
    expect(s.rows().filter((r) => r.kind === "plugin.enable" || r.kind === "plugin.disable").map((r) => [r.kind, r.outcome, r.code])).toEqual([
      ["plugin.enable", "failed", "PLUGIN_RUNTIME_UNAVAILABLE"],
      ["plugin.disable", "ok", null],
      ["plugin.enable", "ok", null],
      ["plugin.enable", "failed", "PLUGIN_NOT_INSTALLED"],
    ]);
  });

  test("remove_deletes_the_store_directory_and_refuses_bundled", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0"] });
    await install(s, "@acme/pack");
    const data = join(serverData, "plugins", "data", "@acme", "pack");
    mkdirSync(data, { recursive: true });
    expect(storeDirs("@acme/pack")).toHaveLength(1);
    expect(await run(s.h["plugin.remove"]({ id: "@acme/pack" }, as(CONSOLE)))).toEqual({ removed: true });
    expect(s.tables.installedOne("@acme/pack")).toBeUndefined();
    expect(s.tables.grants("@acme/pack")).toEqual([]);
    expect(storeDirs("@acme/pack")).toEqual([]);
    expect(existsSync(data)).toBe(true);
    await install(s, "@acme/pack");
    await run(s.h["plugin.remove"]({ id: "@acme/pack", purgeData: true }, as(CONSOLE)));
    expect(existsSync(data)).toBe(false);
    expect(await codeOf(s.h["plugin.remove"]({ id: "@acme/pack" }, as(CONSOLE)))).toBe("PLUGIN_NOT_INSTALLED");
    s.tables.addPlugin({ id: "@finisher/captions-basic", version: "1.0.0", source: "bundled", treeHash: "h", dir: "store/x", kind: "content", installedBy: "system:bundled", installedAt: now });
    expect(await codeOf(s.h["plugin.remove"]({ id: "@finisher/captions-basic" }, as(CONSOLE)))).toBe("PLUGIN_BUNDLED");
    expect(await codeOf(s.life.plan({ id: "@finisher/captions-basic", update: true }, CONSOLE))).toBe("PLUGIN_BUNDLED");
  });
});

describe("updates", () => {
  test("widening_update_needs_approval", async () => {
    const s = await setup({ packages: [code("@acme/tool", "1.0.0")] });
    await install(s, "@acme/tool", { accept: { thirdPartyCode: true } });
    publish(s.reg, { packages: [code("@acme/tool", "1.1.0", [{ perm: "project:read" }, NET])] });
    // A person's refresh installs nothing that widens; the old version keeps running.
    const refreshed = await run(s.h["registry.refresh"]({ id: "team" }, as(CONSOLE)));
    expect(refreshed.applied).toEqual({ revoked: [], updated: [] });
    expect(s.tables.installedOne("@acme/tool")?.version).toBe("1.0.0");
    const plan = await run(s.h["plugin.plan"]({ id: "@acme/tool", update: true }, as(CONSOLE)));
    expect([plan.action, plan.from]).toEqual(["update", "1.0.0"]);
    expect(plan.diff.added).toEqual([NET]);
    await run(s.h["plugin.install"]({ planId: plan.planId, digest: plan.digest, accept: { thirdPartyCode: true } }, as(CONSOLE)));
    expect(s.tables.installedOne("@acme/tool")?.version).toBe("1.1.0");
    expect(s.tables.grants("@acme/tool").sort()).toEqual(['{"origins":["https://api.example.org"],"perm":"net"}', '{"perm":"project:read"}']);
    expect(storeDirs("@acme/tool").map((d) => d.split("-")[0])).toEqual(["1.1.0"]);
    const update = s.rows().find((r) => r.kind === "plugin.update");
    expect(update?.details).toMatchObject({ pluginId: "@acme/tool", version: "1.1.0", permissionsAdded: 1, thirdPartyCode: { accepted: true, confined: false } });
  });

  test("third_party_code_update_needs_acceptance_again", async () => {
    const s = await setup({ packages: [code("@acme/tool", "1.0.0")] });
    await install(s, "@acme/tool", { accept: { thirdPartyCode: true } });
    publish(s.reg, { packages: [code("@acme/tool", "1.0.1")] });
    await run(s.plugins.refresh("team", CONSOLE));
    const { plan } = await run(s.life.plan({ id: "@acme/tool", update: true }, CONSOLE));
    expect(plan.diff).toEqual({ added: [], widened: [], removed: [], riskUp: false });
    expect(await codeOf(s.life.install({ planId: plan.planId, digest: plan.digest }, CONSOLE))).toBe("THIRD_PARTY_CODE_NOT_ACCEPTED");
    expect(s.tables.installedOne("@acme/tool")?.version).toBe("1.0.0");
    await run(s.life.install({ planId: plan.planId, digest: plan.digest, accept: { thirdPartyCode: true } }, CONSOLE));
    expect(s.tables.installedOne("@acme/tool")?.version).toBe("1.0.1");
  });

  test("content_autoupdate_only_when_nothing_widens", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0", "@acme/manual@1.0.0"] });
    await install(s, "@acme/pack", { autoUpdate: true });
    await install(s, "@acme/manual");
    publish(s.reg, { packages: ["@acme/pack@1.1.0", "@acme/manual@1.1.0"] });
    const first = await run(s.h["registry.refresh"]({}, as(CONSOLE)));
    expect(first.applied.updated).toEqual([{ id: "@acme/pack", from: "1.0.0", to: "1.1.0" }]);
    expect(s.tables.installedOne("@acme/pack")).toMatchObject({ version: "1.1.0", autoUpdate: true, enabled: true, state: "active" });
    // Automatic updates are off by default.
    expect(s.tables.installedOne("@acme/manual")?.version).toBe("1.0.0");
    expect(s.rows().filter((r) => r.kind === "plugin.update").map((r) => [r.principalKind, r.details])).toEqual([["console", { pluginId: "@acme/pack", version: "1.1.0", registryId: "team", permissionsAdded: 0, risk: "low", auto: true }]]);
    // 1.2 turns into code with network access: that widens, so it waits for a person.
    publish(s.reg, { packages: [code("@acme/pack", "1.2.0", [NET])] });
    const second = await run(s.h["registry.refresh"]({ id: "team" }, as(CONSOLE)));
    expect(second.applied.updated).toEqual([]);
    expect(s.tables.installedOne("@acme/pack")?.version).toBe("1.1.0");
    // Nothing of the declined plan is left behind.
    expect(control.db.prepare("SELECT COUNT(*) AS n FROM plugin_plans").get()).toEqual({ n: 0 });
  });

  test("agent_facing_kind_never_autoupdates", async () => {
    const s = await setup({ packages: ["@acme/prompts@1.0.0", "@acme/pack@1.0.0"] }, { testAgentFacing: (id) => id === "@acme/prompts" });
    await install(s, "@acme/prompts", { autoUpdate: true });
    await install(s, "@acme/pack", { autoUpdate: true });
    publish(s.reg, { packages: ["@acme/prompts@1.1.0", "@acme/pack@1.1.0"] });
    const r = await run(s.plugins.refresh("team", CONSOLE));
    expect(r.applied.updated.map((u) => u.id)).toEqual(["@acme/pack"]);
    expect(s.tables.installedOne("@acme/prompts")?.version).toBe("1.0.0");
    // A person may still update it, from a plan that says it is agent-facing.
    const { plan } = await run(s.life.plan({ id: "@acme/prompts", update: true }, CONSOLE));
    expect(plan.agentFacing).toBe(true);
    await run(s.life.install({ planId: plan.planId, digest: plan.digest }, CONSOLE));
    expect(s.tables.installedOne("@acme/prompts")?.version).toBe("1.1.0");
  });

  test("agent_refresh_installs_nothing", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0", "@acme/bad@1.0.0"] });
    await install(s, "@acme/pack", { autoUpdate: true });
    await install(s, "@acme/bad", { autoUpdate: true });
    const trusted = (await run(s.plugins.status("team"))).trusted;
    publish(s.reg, { packages: ["@acme/pack@1.1.0"], advisories: [advisory("@acme/bad", "<1.1.0")] });
    for (const by of [AGENT, LOCAL]) {
      const r = await run(s.h["registry.refresh"]({ id: "team" }, as(by, ["plugin.read"])));
      expect(r.byHuman).toBe(false);
      expect(r.applied.updated).toEqual([]);
    }
    // The metadata moved and the advisory applied; the update waits for a person.
    expect((await run(s.plugins.status("team"))).trusted.snapshot).toBeGreaterThan(trusted.snapshot!);
    expect(s.tables.installedOne("@acme/bad")).toMatchObject({ state: "revoked", enabled: false });
    expect(s.tables.installedOne("@acme/pack")).toMatchObject({ version: "1.0.0", state: "active" });
    expect(s.rows().filter((r) => r.kind === "plugin.revoke").map((r) => [r.principalKind, r.code, r.details])).toEqual([["agent", "PLUGIN_REVOKED", { pluginId: "@acme/bad", version: "1.0.0", registryId: "team", code: "PLUGIN_REVOKED" }]]);
    // A person's refresh installs it.
    expect((await run(s.h["registry.refresh"]({ id: "team" }, as(CONSOLE)))).applied.updated).toEqual([{ id: "@acme/pack", from: "1.0.0", to: "1.1.0" }]);
  });

  test("advisory_disable_on_refresh", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0", "@acme/other@1.0.0"] });
    await install(s, "@acme/pack");
    await install(s, "@acme/other");
    publish(s.reg, { advisories: [advisory("@acme/pack", "<1.1.0"), advisory("@acme/other", ">=1.0.0", "warn")] });
    const r = await run(s.h["registry.refresh"]({}, as(CONSOLE)));
    expect(r.applied.revoked).toEqual([{ id: "@acme/pack", version: "1.0.0", advisory: "ADV-10-6" }]);
    expect(s.tables.installedOne("@acme/pack")).toMatchObject({ state: "revoked", enabled: false });
    // A `warn` advisory changes nothing.
    expect(s.tables.installedOne("@acme/other")).toMatchObject({ state: "active", enabled: true });
    expect(await codeOf(s.life.enable("@acme/pack"))).toBe("PLUGIN_REVOKED");
    expect((await run(s.life.disable("@acme/pack"))).state).toBe("revoked");
    // A fixed version is installable again and active.
    publish(s.reg, { packages: ["@acme/pack@1.1.0"] });
    const { plan } = await run(s.life.plan({ id: "@acme/pack", update: true }, CONSOLE));
    await run(s.life.install({ planId: plan.planId, digest: plan.digest }, CONSOLE));
    expect(s.tables.installedOne("@acme/pack")).toMatchObject({ version: "1.1.0", state: "active", enabled: true });
    // A plan's own refresh applies advisories too (audited by the plan's caller).
    publish(s.reg, { advisories: [advisory("@acme/other", ">=1.0.0")] });
    await run(s.h["plugin.plan"]({ id: "@acme/pack", version: "1.0.0", update: true, allowDowngrade: true }, as(CONSOLE)));
    expect(s.tables.installedOne("@acme/other")?.state).toBe("revoked");
    expect(s.rows().filter((x) => x.kind === "plugin.revoke").map((x) => x.details.pluginId)).toEqual(["@acme/pack", "@acme/other"]);
  });

  test("refresh_failure_keeps_installed_plugins_running", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0"] });
    await install(s, "@acme/pack", { autoUpdate: true });
    const before = snapshot();
    const other = registry({ packages: ["@acme/pack@9.0.0"] });
    writeFileSync(join(s.reg.dir, "metadata", "timestamp.json"), readFileSync(join(other.dir, "metadata", "timestamp.json")));
    expect(await codeOf(s.h["registry.refresh"]({ id: "team" }, as(CONSOLE)))).toMatch(/^(TUF_|REGISTRY_)/);
    const all = await run(s.h["registry.refresh"]({}, as(CONSOLE)));
    expect(all.results[0]?.ok).toBe(false);
    expect(all.applied).toEqual({ revoked: [], updated: [] });
    expect(snapshot()).toEqual(before);
    expect(s.tables.installedOne("@acme/pack")).toMatchObject({ enabled: true, state: "active" });
    // A plan from it fails with the refresh's error.
    expect(await codeOf(s.life.plan({ id: "@acme/pack", update: true }, CONSOLE))).toMatch(/^(TUF_|REGISTRY_)/);
  });
});

describe("audit and suggestions", () => {
  test("audit_details_allow_list", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0", code("@acme/tool", "1.0.0", [NET])] });
    await run(s.h["plugin.install"]({ ...(await run(s.h["plugin.plan"]({ id: "@acme/pack" }, as(CONSOLE)))), accept: {} }, as(CONSOLE)).pipe(Effect.ignore));
    const plan = await run(s.h["plugin.plan"]({ id: "@acme/tool" }, as(CONSOLE)));
    await Effect.runPromiseExit(s.h["plugin.install"]({ planId: plan.planId, digest: plan.digest }, as(CONSOLE)));
    await run(s.h["plugin.install"]({ planId: plan.planId, digest: plan.digest, accept: { thirdPartyCode: true } }, as(CONSOLE)));
    await Effect.runPromiseExit(s.h["plugin.enable"]({ id: "@acme/tool" }, as(CONSOLE)));
    publish(s.reg, { advisories: [advisory("@acme/tool", "*")] });
    await run(s.h["registry.refresh"]({ id: "team" }, as(LOCAL)));
    await run(s.h["plugin.remove"]({ id: "@acme/tool", purgeData: true }, as(CONSOLE)));
    const rows = s.rows();
    expect(new Set(rows.map((r) => r.kind))).toEqual(new Set(["plugin.install", "plugin.grant", "plugin.enable", "plugin.revoke", "plugin.remove"]));
    const allowed = new Set(["pluginId", "version", "registryId", "rootSha256", "permissionsAdded", "risk", "code", "thirdPartyCode", "auto", "planId"]);
    for (const r of rows) for (const k of Object.keys(r.details)) expect(allowed, `${r.kind}.${k}`).toContain(k);
    // Nothing was dropped by the closed schema, and no path or publisher name was written.
    expect(s.audit.failures()).toBe(0);
    const text = JSON.stringify(rows);
    expect(text).not.toContain(s.reg.dir);
    expect(text).not.toContain(serverData);
    expect(text).not.toContain("Test");
  });

  test("suggest_records_plugin_suggestion", async () => {
    const store = new ReviewStore(control, () => now);
    const s = service({ suggestions: pluginSuggestions(store) });
    const P = "01aaaaaaaaaaaaaaaaaaaaaaaa";
    const r = await run(s.h["plugin.suggest"]({ id: "@acme/pack", reason: "captions in the house style", projectId: P }, as(AGENT, ["plugin.suggest", "project.read"])));
    expect(r.suggestionId).toMatch(/^[0-9a-z]{26}$/i);
    // An item now, not a row.
    expect(s.tables.suggestions()).toEqual([]);
    expect(store.list(P)).toMatchObject([
      { id: r.suggestionId, kind: "pluginSuggestion", state: "open", title: "@acme/pack", summary: "captions in the house style", requestedBy: { principal: `agent:${AGENT.id}` }, subject: { pluginId: "@acme/pack" }, createdAt: now },
    ]);
    // It installs nothing and is no approval.
    expect(s.tables.installed()).toEqual([]);
    expect(control.db.prepare("SELECT COUNT(*) AS n FROM plugin_plans").get()).toEqual({ n: 0 });
    // Its scopes: the suggestion's own and reading the project it is about.
    expect(requiredScopes("plugin.suggest", SAMPLE_PAYLOADS["plugin.suggest"])).toEqual(["plugin.suggest", "project.read"]);
  });

  test("suggestion_is_item: one open item per plugin, project and principal, capped per principal", async () => {
    const store = new ReviewStore(control, () => now);
    const s = service({ suggestions: pluginSuggestions(store) });
    const P = "01aaaaaaaaaaaaaaaaaaaaaaaa";
    const first = await run(s.life.suggest({ id: "@acme/pack", reason: "once", projectId: P }, AGENT));
    expect(await run(s.life.suggest({ id: "@acme/pack", reason: "twice", projectId: P }, AGENT))).toEqual(first);
    expect(store.list(P).length).toBe(1);
    await run(s.life.suggest({ id: "@acme/pack", reason: "x", projectId: P }, MCP));
    for (let i = 1; i < MAX_SUGGESTIONS; i++) await run(s.life.suggest({ id: `@acme/pack-${i + 10}`, reason: "x", projectId: P }, AGENT));
    expect(await codeOf(s.life.suggest({ id: "@acme/one-more", reason: "x", projectId: P }, AGENT))).toBe("TOO_MANY_SUGGESTIONS");
    // Closing one by a person frees a slot, and the plugin may be suggested again.
    store.setState(first.suggestionId, "dismissed", "console:console");
    expect((await run(s.life.suggest({ id: "@acme/pack", reason: "again", projectId: P }, AGENT))).suggestionId).not.toBe(first.suggestionId);
  });

  test("install_supersedes_suggestion", async () => {
    const store = new ReviewStore(control, () => now);
    const s = await setup({ packages: ["@acme/pack@1.0.0", "@acme/other@1.0.0"] }, { suggestions: pluginSuggestions(store) });
    const P = "01aaaaaaaaaaaaaaaaaaaaaaaa";
    const Q = "01bbbbbbbbbbbbbbbbbbbbbbbb";
    const a = await run(s.life.suggest({ id: "@acme/pack", reason: "x", projectId: P }, AGENT));
    const b = await run(s.life.suggest({ id: "@acme/pack", reason: "x", projectId: Q }, MCP));
    const other = await run(s.life.suggest({ id: "@acme/other", reason: "x", projectId: P }, AGENT));
    await install(s, "@acme/pack");
    expect(store.get(a.suggestionId)?.state).toBe("superseded");
    expect(store.get(b.suggestionId)?.state).toBe("superseded");
    expect(store.get(other.suggestionId)?.state).toBe("open");
  });

  test("suggestion_rows_moved_once: a crash between the insert and the delete moves nothing twice", () => {
    const store = new ReviewStore(control, () => now);
    const P = "01aaaaaaaaaaaaaaaaaaaaaaaa";
    const add = control.db.prepare("INSERT INTO plugin_suggestions (id, pluginId, projectId, reason, byKind, byId, createdAt) VALUES (?, ?, ?, ?, 'agent', ?, ?)");
    add.run("01sugg000000000000000000a1", "@acme/pack", P, "one", AGENT.id, now - 5);
    add.run("01sugg000000000000000000a2", "@acme/other", P, "two", AGENT.id, now - 4);
    // A first move that stopped after its insert: the item is there, the row too.
    store.adopt({ id: "01sugg000000000000000000a1", createdAt: now - 5, projectId: P, kind: "pluginSuggestion", title: "@acme/pack", summary: "one", requestedBy: { principal: `agent:${AGENT.id}` }, subject: { pluginId: "@acme/pack" } });
    expect(moveSuggestionRows(control, store)).toBe(2);
    expect(control.db.prepare("SELECT COUNT(*) AS n FROM plugin_suggestions").get()).toEqual({ n: 0 });
    expect(store.list(P).map((i) => [i.id, i.createdAt]).sort()).toEqual([
      ["01sugg000000000000000000a1", now - 5],
      ["01sugg000000000000000000a2", now - 4],
    ]);
    expect(store.events("01sugg000000000000000000a1").length).toBe(1);
    // A second start moves nothing.
    expect(moveSuggestionRows(control, store)).toBe(0);
    expect(store.list(P).length).toBe(2);
  });
});

describe("registry paths", () => {
  test("registry_path_shown_to_people_only", async () => {
    const s = await setup({ packages: ["@acme/pack@1.0.0"] });
    for (const by of [CONSOLE, DEVICE]) {
      expect((await run(s.h["registry.list"]({}, as(by)))).registries[0]?.path, by.kind).toBe(s.reg.dir);
      expect((await run(s.h["registry.status"]({ id: "team" }, as(by)))).path, by.kind).toBe(s.reg.dir);
      const r = (await run(s.h["registry.refresh"]({ id: "team" }, as(by)))).results[0];
      expect(r?.ok && r.status.path, by.kind).toBe(s.reg.dir);
    }
    // `local` is not a person (Decision 20): an agent in a shell authenticates as it (L1).
    for (const by of [LOCAL, AGENT, MCP]) {
      const list = await run(s.h["registry.list"]({}, as(by, ["plugin.read"])));
      expect(list.registries[0], by.kind).not.toHaveProperty("path");
      expect(list.registries[0]?.id).toBe("team");
      expect(await run(s.h["registry.status"]({ id: "team" }, as(by, ["plugin.read"])))).not.toHaveProperty("path");
      const refreshed = await run(s.h["registry.refresh"]({ id: "team" }, as(by, ["plugin.read"])));
      const r = refreshed.results[0];
      expect(r?.ok && r.status, by.kind).not.toHaveProperty("path");
      expect(JSON.stringify(refreshed)).not.toContain(s.reg.dir);
      expect(JSON.stringify(list)).not.toContain(s.reg.dir);
    }
  });
});
