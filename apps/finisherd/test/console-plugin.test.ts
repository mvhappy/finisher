// The console's `plugin` and `registry` commands (plan 8A Task 12b): the install flow (plan →
// printed with its risks → `yes` → the server's typed code → install), the typed id for
// third-party code, a registry's root typed back (never trusted on first use), updates that show
// what they widen, downgrades, ambiguity questions, and the terminal gate. Real server (child
// process), real registries built by `fin-node`'s test hooks, and a scripted person.
import { spawn } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { testHooks } from "@finisher/fin-node";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { runConsole } from "../src/console/main.ts";
import type { Prompt } from "../src/console/prompt.ts";
import { type RunningServer, startServer } from "./support/ndjson-client.ts";

const hooks = testHooks!;
const MAIN = fileURLToPath(new URL("../src/main.ts", import.meta.url));
const ID = "@acme/tool";
const NET = { perm: "net", origins: ["https://api.example.org"] };
const code = (version: string, permissions: object[] = [{ perm: "project:read" }], id = ID) => ({ id, version, kind: "code", permissions });

let tmp: string;
let serverData: string;
let server: RunningServer | undefined;
let built = 0;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "fd-cp-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(async () => {
  await server?.stop();
  server = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

function env(): NodeJS.ProcessEnv {
  const { DO_NOT_TRACK: _d, FIN_DIAGNOSTICS: _f, FIN_TRAINING: _t, FIN_SERVER_DATA: _s, ...rest } = process.env;
  return rest;
}

interface Built {
  readonly dir: string;
  readonly keys: string;
  readonly rootSha256: string;
}

function registry(spec: object): Built {
  const dir = join(tmp, `registry-${++built}`);
  const keys = join(tmp, `keys-${built}`);
  const r = JSON.parse(hooks.buildRegistry(dir, keys, JSON.stringify(spec))) as { rootSha256: string };
  return { dir, keys, rootSha256: r.rootSha256 };
}
const publish = (r: Built, spec: object) => hooks.publishToRegistry(r.dir, r.keys, JSON.stringify(spec));

/** A person at a terminal: types the server's code when asked for it, else the next answer. */
function person(...answers: string[]): Prompt & { shown: string[]; asked: string[] } {
  const shown: string[] = [];
  const asked: string[] = [];
  let last = "";
  return {
    shown,
    asked,
    isInteractive: () => true,
    show: (text) => {
      shown.push(text);
      const m = /type this code within 60 seconds: ([0-9A-Z]{6})$/.exec(text);
      if (m) last = m[1]!;
    },
    ask: async (question) => {
      asked.push(question);
      return question === "Code: " ? last : (answers.shift() ?? "");
    },
  };
}

const NOT_A_TERMINAL: Prompt = { isInteractive: () => false, show: () => undefined, ask: async () => "" };

async function run(argv: string[], prompt: Prompt = person()): Promise<{ code: number; out: string; err: string }> {
  let out = "";
  let err = "";
  const exit = await runConsole([...argv, "--server-data", serverData], {
    prompt,
    output: { out: (t) => void (out += `${t}\n`), err: (t) => void (err += `${t}\n`) },
  });
  return { code: exit, out, err };
}

function spawned(argv: string[]): Promise<{ code: number | null; out: string; err: string }> {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [MAIN, ...argv, "--server-data", serverData], { stdio: ["pipe", "pipe", "pipe"], env: env() });
    let out = "";
    let err = "";
    child.stdout.setEncoding("utf8").on("data", (d: string) => (out += d));
    child.stderr.setEncoding("utf8").on("data", (d: string) => (err += d));
    child.stdin.end("yes\nyes\nyes\n");
    child.once("exit", (c) => resolve({ code: c, out, err }));
  });
}

const start = async () => (server = await startServer(serverData, 20_000, env()));

/** Adds `reg` as `team` by its root's hash; nothing else. */
async function addTeam(reg: Built, id = "team"): Promise<void> {
  const r = await run(["registry", "add", id, "--dir", reg.dir, "--root-sha256", reg.rootSha256]);
  expect(r.err, "registry add").toBe("");
  expect(r.code).toBe(0);
}

async function plugins(): Promise<Array<{ id: string; version: string; enabled: boolean; autoUpdate: boolean }>> {
  const r = await run(["plugin", "ls", "--json"], NOT_A_TERMINAL);
  return (JSON.parse(r.out) as { plugins: Array<{ id: string; version: string; enabled: boolean; autoUpdate: boolean }> }).plugins;
}

/** The console's audit rows of `kind` (the bundled pack's own install is the server's). */
async function audited(kind: string): Promise<Array<{ outcome: string; details: Record<string, unknown> }>> {
  const r = await run(["audit", "--kind", kind, "--json"], NOT_A_TERMINAL);
  return (JSON.parse(r.out) as { entries: Array<{ principalKind: string; outcome: string; details: Record<string, unknown> }> }).entries.filter((e) => e.principalKind === "console");
}

describe("the terminal gate", () => {
  const MUTATING = [
    ["registry", "add", "team", "--dir", "/nowhere", "--root-sha256", "a".repeat(64)],
    ["registry", "rm", "team"],
    ["registry", "forget", "--root", "a".repeat(64)],
    ["plugin", "install", ID],
    ["plugin", "update", ID],
    ["plugin", "rm", ID],
    ["plugin", "enable", ID],
    ["plugin", "disable", ID],
  ];

  test("non_tty_mutations_are_console_needs_tty_and_reads_run_anywhere", { timeout: 90_000 }, async () => {
    await start();
    const reg = registry({ packages: ["@acme/pack@1.0.0"] });
    await addTeam(reg);
    for (const argv of MUTATING) {
      const r = await spawned(argv);
      expect(r.code, argv.join(" ")).toBe(7);
      expect(r.err, argv.join(" ")).toMatch(/^finisherd: CONSOLE_NEEDS_TTY: /);
      expect(NOT_A_TERMINAL.isInteractive()).toBe(false);
      expect((await run(argv, NOT_A_TERMINAL)).code, argv.join(" ")).toBe(7);
    }
    expect(await plugins()).toEqual(expect.not.arrayContaining([expect.objectContaining({ id: "@acme/pack" })]));
    // Reading needs no terminal and no code.
    for (const argv of [["registry", "ls"], ["registry", "status", "team"], ["registry", "refresh"], ["plugin", "search", "acme"], ["plugin", "info", "@acme/pack"], ["plugin", "ls"]]) {
      const r = await spawned(argv);
      expect(r.code, `${argv.join(" ")}: ${r.err}`).toBe(0);
    }
    const found = await run(["plugin", "search", "acme"], NOT_A_TERMINAL);
    expect(found.out).toContain("@acme/pack 1.0.0");
  });
});

describe("registry add", () => {
  test("console_add_without_root_requires_typed_hash", { timeout: 60_000 }, async () => {
    await start();
    const reg = registry({ packages: ["@acme/pack@1.0.0"] });
    // Nothing typed, then something else typed: no registry is added, nothing is trusted. The
    // hash is asked for before it is shown (review L1): nothing of the folder's root appears
    // on a screen that is not matched, neither its hash nor its key ids.
    for (const typed of ["", "0".repeat(64), "not a hash"]) {
      const p = person(typed);
      const r = await run(["registry", "add", "team", "--dir", reg.dir], p);
      expect(r.code, typed).toBe(7);
      expect(r.err).toContain("CONFIRMATION_REQUIRED");
      expect(p.shown.join("\n")).not.toContain(reg.rootSha256);
      expect(p.shown.join("\n")).not.toMatch(/root keys|[0-9a-f]{64}/);
      expect(r.err + r.out).not.toContain(reg.rootSha256);
      expect(p.asked, typed).toEqual(["Root SHA-256: "]);
    }
    expect((await run(["registry", "ls", "--json"], NOT_A_TERMINAL)).out).toContain('"registries":[]');
    expect(await audited("registry.trust")).toEqual([]);
    // The hash typed back (upper case too) adds it, after the server's code, and lists its plugins.
    const p = person(reg.rootSha256.toUpperCase());
    const r = await run(["registry", "add", "team", "--dir", reg.dir], p);
    expect(r.err).toBe("");
    expect(r.code).toBe(0);
    // Only now, matched, the root is described.
    expect(p.shown.join("\n")).toContain("That matches this folder's first root:");
    expect(p.shown.join("\n")).toMatch(/root keys: [0-9a-f]{64}.*\(1 must sign\)/);
    expect(r.out).toContain(`Added registry team (root ${reg.rootSha256})`);
    expect(r.out).toContain("It lists 1 plugins");
    expect((await run(["registry", "ls"], NOT_A_TERMINAL)).out).toContain(`root ${reg.rootSha256}`);
    expect(await audited("registry.trust")).toMatchObject([{ outcome: "ok", details: { registryId: "team", rootSha256: reg.rootSha256 } }]);
  });

  test("a_wrong_root_flag_adds_nothing_and_a_root_file_works", { timeout: 60_000 }, async () => {
    await start();
    const reg = registry({ packages: ["@acme/pack@1.0.0"] });
    const bad = await run(["registry", "add", "team", "--dir", reg.dir, "--root-sha256", "0".repeat(64)]);
    expect(bad.code).toBe(7);
    expect(bad.err).toContain("REGISTRY_UNTRUSTED");
    // A root file inside the registry's folder is warned about (review L1), and works.
    const inside = person();
    expect((await run(["registry", "add", "team", "--dir", reg.dir, "--root", join(reg.dir, "metadata", "1.root.json")], inside)).code).toBe(0);
    expect(inside.shown.join("\n")).toContain("is inside the registry's folder");
    expect((await run(["registry", "add", "team", "--dir", reg.dir, "--root-sha256", reg.rootSha256])).err).toContain("REGISTRY_EXISTS");
    // Usage mistakes are caught before anything connects.
    expect((await run(["registry", "add", "team"])).code).toBe(2);
    expect((await run(["registry", "add", "team", "--dir", reg.dir, "--root", "x", "--root-sha256", reg.rootSha256])).code).toBe(2);
    expect((await run(["registry", "add", "Team", "--dir", reg.dir])).code).toBe(2);
  });

  test("a_root_file_outside_the_folder_is_not_warned_about", { timeout: 60_000 }, async () => {
    await start();
    const reg = registry({ packages: ["@acme/pack@1.0.0"] });
    const outside = join(tmp, "publisher-root.json");
    copyFileSync(join(reg.dir, "metadata", "1.root.json"), outside);
    const p = person();
    expect((await run(["registry", "add", "team", "--dir", reg.dir, "--root", outside], p)).code).toBe(0);
    expect(p.shown.join("\n")).not.toContain("inside the registry's folder");
  });

  test("registry_rm_and_forget_keep_and_drop_trust", { timeout: 60_000 }, async () => {
    await start();
    const reg = registry({ packages: ["@acme/pack@1.0.0"] });
    await addTeam(reg);
    const gone = await run(["registry", "rm", "team"]);
    expect(gone.code).toBe(0);
    expect((await run(["registry", "status", "team"], NOT_A_TERMINAL)).code).toBe(3);
    // Adding it again resumes; forgetting its root drops what was trusted.
    await addTeam(reg);
    await run(["registry", "rm", "team"]);
    const p = person();
    expect((await run(["registry", "forget", "--root", reg.rootSha256], p)).code).toBe(0);
    expect(p.shown.join("\n")).toContain(reg.rootSha256);
    expect((await audited("registry.forget")).map((e) => e.details.rootSha256)).toEqual([reg.rootSha256]);
  });
});

describe("plugin install", () => {
  test("console_install_prints_plan_and_needs_yes", { timeout: 60_000 }, async () => {
    await start();
    await addTeam(registry({ packages: ["@acme/pack@1.0.0"] }));
    // Anything but yes cancels, after the plan and before the code.
    const no = person("no");
    const refused = await run(["plugin", "install", "@acme/pack"], no);
    expect(refused.code).toBe(7);
    expect(refused.err).toContain("CONFIRMATION_REQUIRED");
    expect(no.asked).toEqual(["Type yes to install @acme/pack 1.0.0: "]);
    expect(no.shown.join("\n")).not.toContain("type this code");
    expect(await plugins()).toEqual(expect.not.arrayContaining([expect.objectContaining({ id: "@acme/pack" })]));
    // yes, then the code.
    const yes = person("yes");
    const r = await run(["plugin", "install", "@acme/pack"], yes);
    expect(r.err).toBe("");
    expect(r.code).toBe(0);
    const shown = yes.shown.join("\n");
    expect(shown).toContain("Install @acme/pack 1.0.0 from registry team");
    expect(shown).toContain("a content pack");
    expect(shown).toMatch(/Licence: \S+ \(permissive\)/);
    expect(shown).toContain("Risk: low");
    expect(shown).toContain("Permissions: none");
    expect(shown).not.toContain("THIRD-PARTY CODE");
    expect(yes.asked).toEqual(["Type yes to install @acme/pack 1.0.0: ", "Code: "]);
    expect(r.out).toContain("Installed @acme/pack 1.0.0 (enabled)");
    expect(await plugins()).toContainEqual(expect.objectContaining({ id: "@acme/pack", version: "1.0.0", enabled: true, autoUpdate: false }));
    expect(await audited("plugin.install")).toMatchObject([{ outcome: "ok", details: { pluginId: "@acme/pack", version: "1.0.0", registryId: "team" } }]);
    // Again: it is installed already; the plan says so and nothing is asked.
    const again = await run(["plugin", "install", "@acme/pack"], person("yes"));
    expect(again.code).not.toBe(0);
    expect(again.err).toContain("PLUGIN_ALREADY_INSTALLED");
  });

  /** Review M3: a plan made and not installed gave up its slot (a principal has 4 open at most). */
  test("every_exit_after_a_plan_discards_it", { timeout: 90_000 }, async () => {
    await start();
    await addTeam(registry({ packages: ["@acme/pack@1.0.0", { id: "@acme/gpl", version: "1.0.0", license: "MPL-2.0" }, code("1.0.0", [{ perm: "project:read" }])] }));
    // Eight abandoned plans: cancelled with `no`, refused for want of --accept-copyleft, and the
    // third-party code's id typed wrong. Without the discard the fifth would be TOO_MANY_PLANS.
    for (let i = 0; i < 3; i++) expect((await run(["plugin", "install", "@acme/pack"], person("no"))).err).toContain("CONFIRMATION_REQUIRED");
    for (let i = 0; i < 3; i++) expect((await run(["plugin", "install", "@acme/gpl"], person("yes"))).err).toContain("--accept-copyleft");
    for (let i = 0; i < 2; i++) expect((await run(["plugin", "install", ID], person("wrong"))).err).toContain("CONFIRMATION_REQUIRED");
    const r = await run(["plugin", "install", "@acme/pack"], person("yes"));
    expect(r.err).toBe("");
    expect(r.code).toBe(0);
    expect(await plugins()).toContainEqual(expect.objectContaining({ id: "@acme/pack", enabled: true }));
  });

  test("auto_update_flag_is_sent_and_shown", { timeout: 60_000 }, async () => {
    await start();
    await addTeam(registry({ packages: ["@acme/pack@1.0.0"] }));
    const r = await run(["plugin", "install", "@acme/pack", "--auto-update"], person("yes"));
    expect(r.code).toBe(0);
    expect(r.out).toContain("updates itself");
    expect(await plugins()).toContainEqual(expect.objectContaining({ id: "@acme/pack", autoUpdate: true }));
  });

  test("console_third_party_code_needs_typed_id", { timeout: 60_000 }, async () => {
    await start();
    await addTeam(registry({ packages: [code("1.0.0", [{ perm: "project:read" }, { perm: "gpu" }, NET])] }));
    // `yes` is not enough: nothing installs.
    const yes = person("yes");
    const refused = await run(["plugin", "install", ID], yes);
    expect(refused.code).toBe(7);
    expect(refused.err).toContain("CONFIRMATION_REQUIRED");
    expect(yes.asked).toEqual([`Type the plugin's id (${ID}) to accept third-party code and continue: `]);
    expect(await plugins()).toEqual(expect.not.arrayContaining([expect.objectContaining({ id: ID })]));
    // The warning comes before the plan, and each permission has its own line.
    const lines = yes.shown.flatMap((s) => s.split("\n"));
    const warning = lines.findIndex((l) => l.includes("THIRD-PARTY CODE"));
    const perms = lines.map((l, i) => [l, i] as const).filter(([l]) => /^ {2}\[(low|medium|high)\] /.test(l));
    expect(warning).toBe(0);
    expect(perms.map(([l]) => l.replace(/^ {2}\[\w+\] /, ""))).toEqual(["project:read", "gpu", "net (origins: https://api.example.org)"]);
    expect(perms).toHaveLength(3);
    expect(perms.every(([, i]) => i > warning)).toBe(true);
    expect(lines.some((l) => l.trim() === "Risk: high")).toBe(true);
    // The id typed back installs it, switched off, and the log says it was accepted.
    const typed = person(ID);
    const r = await run(["plugin", "install", ID], typed);
    expect(r.err).toBe("");
    expect(r.code).toBe(0);
    expect(r.out).toContain(`Installed ${ID} 1.0.0 (switched off`);
    expect(await plugins()).toContainEqual(expect.objectContaining({ id: ID, enabled: false }));
    expect(await audited("plugin.install")).toMatchObject([{ outcome: "ok", details: { pluginId: ID, thirdPartyCode: { accepted: true, confined: false } } }]);
  });

  test("copyleft_needs_its_flag", { timeout: 60_000 }, async () => {
    await start();
    await addTeam(registry({ packages: [{ id: "@acme/gpl", version: "1.0.0", license: "MPL-2.0" }] }));
    const p = person("yes");
    const refused = await run(["plugin", "install", "@acme/gpl"], p);
    expect(refused.code).toBe(2);
    expect(refused.err).toContain("--accept-copyleft");
    expect(p.asked).toEqual([]);
    expect((await run(["plugin", "install", "@acme/gpl", "--accept-copyleft"], person("yes"))).code).toBe(0);
  });

  test("ambiguity_asks_for_a_registry", { timeout: 60_000 }, async () => {
    await start();
    await addTeam(registry({ packages: ["@acme/pack@1.0.0"] }), "one");
    await addTeam(registry({ packages: ["@acme/pack@2.0.0"] }), "two");
    const p = person("yes");
    const r = await run(["plugin", "install", "@acme/pack"], p);
    expect(r.code).toBe(2);
    expect(r.err).toContain("one, two");
    expect(r.err).toContain("--registry <id>");
    expect(p.asked).toEqual([]);
    const ok = await run(["plugin", "install", "@acme/pack", "--registry", "two"], person("yes"));
    expect(ok.code).toBe(0);
    expect(ok.out).toContain("Installed @acme/pack 2.0.0");
  });
});

describe("plugin update", () => {
  test("console_update_shows_widening", { timeout: 60_000 }, async () => {
    await start();
    const reg = registry({ packages: [code("1.0.0")] });
    await addTeam(reg);
    expect((await run(["plugin", "install", ID], person(ID))).code).toBe(0);
    publish(reg, { packages: [code("1.1.0", [{ perm: "project:read" }, NET])] });
    expect((await run(["registry", "refresh"], NOT_A_TERMINAL)).code).toBe(0);
    const p = person(ID);
    const r = await run(["plugin", "update", ID], p);
    expect(r.err).toBe("");
    expect(r.code).toBe(0);
    const shown = p.shown.join("\n");
    expect(shown).toContain(`Update ${ID} 1.0.0 → 1.1.0 from registry team`);
    expect(shown).toContain("Compared with 1.0.0:");
    expect(shown).toContain("+ adds net (origins: https://api.example.org)");
    expect(shown).toContain("The overall risk goes up.");
    expect(r.out).toContain(`Updated ${ID} 1.1.0`);
    expect(await audited("plugin.update")).toMatchObject([{ outcome: "ok", details: { pluginId: ID, version: "1.1.0", permissionsAdded: 1 } }]);
    // Nothing newer: the plan is refused and nothing is asked.
    const none = person(ID);
    expect((await run(["plugin", "update", ID], none)).code).not.toBe(0);
    expect(none.asked).toEqual([]);
  });

  test("downgrade_needs_the_flag_and_the_version_and_two_codes", { timeout: 60_000 }, async () => {
    await start();
    const reg = registry({ packages: ["@acme/pack@1.0.0", "@acme/pack@1.1.0"] });
    await addTeam(reg);
    expect((await run(["plugin", "install", "@acme/pack@1.1.0"], person("yes"))).code).toBe(0);
    // Without the flag the server refuses; the flag without a version is a usage error.
    expect((await run(["plugin", "update", "@acme/pack@1.0.0"], person("yes"))).err).toContain("PLUGIN_DOWNGRADE_REFUSED");
    expect((await run(["plugin", "update", "@acme/pack", "--allow-downgrade"])).code).toBe(2);
    expect((await run(["plugin", "install", "@acme/pack@1.0.0", "--allow-downgrade"])).code).toBe(2);
    const p = person("yes");
    const r = await run(["plugin", "update", "@acme/pack@1.0.0", "--allow-downgrade"], p);
    expect(r.err).toBe("");
    expect(r.code).toBe(0);
    expect(p.shown.join("\n")).toContain("DOWNGRADE @acme/pack 1.1.0 → 1.0.0");
    // The plan and the install each ask for the server's code.
    expect(p.asked).toEqual(["Code: ", "Type yes to downgrade @acme/pack 1.0.0: ", "Code: "]);
    expect(r.out).toContain("Downgraded @acme/pack 1.0.0");
  });

  test("root_changed_asks_for_a_registry_and_warns_when_named", { timeout: 60_000 }, async () => {
    await start();
    await addTeam(registry({ packages: [code("1.0.0")] }));
    expect((await run(["plugin", "install", ID], person(ID))).code).toBe(0);
    // Another folder under the same registry id: another root.
    expect((await run(["registry", "rm", "team"])).code).toBe(0);
    await addTeam(registry({ packages: [code("1.2.0")] }));
    const p = person(ID);
    const r = await run(["plugin", "update", ID], p);
    expect(r.code).toBe(2);
    expect(r.err).toContain("another root");
    expect(r.err).toContain("--registry <id>");
    expect(p.asked).toEqual([]);
    // Named, the plan carries the warning, first.
    const named = person(ID);
    const done = await run(["plugin", "update", ID, "--registry", "team"], named);
    expect(done.code).toBe(0);
    const lines = named.shown.flatMap((s) => s.split("\n"));
    const changed = lines.findIndex((l) => l.includes("REGISTRY CHANGED"));
    expect(changed).toBeGreaterThanOrEqual(0);
    expect(changed).toBeLessThan(lines.findIndex((l) => l.startsWith("Update ")));
  });
});

describe("plugin rm, enable and disable", () => {
  test("they_change_the_plugin_after_the_code", { timeout: 60_000 }, async () => {
    await start();
    await addTeam(registry({ packages: ["@acme/pack@1.0.0"] }));
    expect((await run(["plugin", "install", "@acme/pack"], person("yes"))).code).toBe(0);
    const off = await run(["plugin", "disable", "@acme/pack"]);
    expect(off.out).toContain("@acme/pack is now disabled");
    expect(await plugins()).toContainEqual(expect.objectContaining({ id: "@acme/pack", enabled: false }));
    expect((await run(["plugin", "enable", "@acme/pack"])).out).toContain("is now enabled");
    const p = person();
    const gone = await run(["plugin", "rm", "@acme/pack", "--purge-data"], p);
    expect(gone.code).toBe(0);
    expect(p.shown.join("\n")).toContain("deletes the data it stored");
    expect(await plugins()).toEqual(expect.not.arrayContaining([expect.objectContaining({ id: "@acme/pack" })]));
    // The wrong code does not remove anything that is not there; a missing plugin is an error.
    expect((await run(["plugin", "rm", "@acme/pack"])).code).not.toBe(0);
    expect((await run(["plugin", "rm", "not-an-id"])).code).toBe(2);
  });
});
