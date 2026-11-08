// The "plugins" scenario (plan 8A Task 13): the plugin lifecycle end to end, over a real
// finisherd, `fin` and `fin-plugin`, with local-folder registries (one a plain folder, one a git
// clone the run commits to and resets). Nothing touches the network.
//   1. the bundled pack is there, installed without a plan;
//   2. `fin-plugin` builds a registry `team` (publisher acme); the console adds it by
//      its root's SHA-256 (a wrong hash is refused; `fin registry add` is `HUMAN_ONLY`);
//   3. refresh (the owner's and `fin`'s, with and without an agent session), search and info;
//   4. `fin plugin install` is `HUMAN_ONLY`; so is a person's install of a plan `fin` made;
//   5. a person plans and installs a content pack through the console's typed code, and
//      `fin style add --from` copies its style (and the bundled pack's);
//   6. a font pack merges into `fonts.catalog`; a pack whose face is named like a built-in
//      family cannot be packed (review H1; the server's plan refuses it with the same code);
//   7. a code plugin: the `thirdPartyCode` warning, `THIRD_PARTY_CODE_NOT_ACCEPTED` until
//      accepted, installed switched off, `PLUGIN_RUNTIME_UNAVAILABLE` on enable;
//   8. plans are the maker's to discard and capped (review M3);
//   9. an update that adds `net` needs the acceptance again; an agent's refresh installs nothing,
//      a person's applies the automatic update;
//  10. a signed `disable` advisory revokes a pack on any refresh; text that could rewrite a
//      terminal is refused at signing (review M1); a person removes a pack, not a bundled one;
//  11. a git clone reset to an older commit is refused (`TUF_ROLLBACK`), installed packs keep
//      working, and removing and re-adding the registry keeps its trust;
//  12. restarts: an owner-disabled bundled pack stays off through a tampered-and-repaired
//      resources folder (review M2);
//  13. the journal holds no plugin id, publisher or registry path, and every step above has its
//      audit row.
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { ROOT, assertEqual, consoleCall, fin, finRaw, live, log, removeTmp, scenarioTmp, socketCall, startServer, stopServer, storedEntries } from "./lib.mjs";
import { filesUnder, leaks } from "./diagnostics-lib.mjs";
import { auditRows, finPlugin, fontCatalog, git, makeRegistry, writePlugin } from "./plugins-lib.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);
const SESSION = "01JC0000000000000000000P1";
const SWITCHES = ["FIN_DIAGNOSTICS", "FIN_TRAINING", "DO_NOT_TRACK"];
const BUNDLED = "@finisher/captions-basic";
const need = (cond, what) => {
  if (!cond) throw new Error(what);
};

/** Runs `call` (an RPC promise) and returns the error text of its failure; throws when it succeeds. */
async function failure(call, code) {
  try {
    await call;
  } catch (e) {
    const text = e instanceof Error ? e.message : String(e);
    const got = /"code":"([A-Z_]+)"/.exec(text)?.[1];
    if (got !== code) throw new Error(`expected ${code}, got ${got}: ${text}`);
    return text;
  }
  throw new Error(`expected ${code}, but the call succeeded`);
}

export async function run() {
  const started = Date.now();
  const saved = Object.fromEntries(SWITCHES.map((k) => [k, process.env[k]]));
  for (const k of SWITCHES) delete process.env[k];
  const tmp = scenarioTmp("finisher-e2e-plug-");
  live.tmp = tmp;
  const sd = join(tmp, "s");
  const work = join(tmp, "w");
  const bundledDir = join(tmp, "bundled");
  const project = join(tmp, "film");
  mkdirSync(work, { recursive: true });
  cpSync(join(ROOT, "resources", "plugins", "bundled"), bundledDir, { recursive: true });
  let server;
  let calls = 0;
  const start = async () => (server = await startServer(sd, { FIN_BUNDLED_PLUGINS_DIR: bundledDir }));
  const agent = () => ({ FIN_SESSION: SESSION, FIN_CALL: `call-${++calls}` });
  const f = (args, env) => fin(sd, project, args, env);
  const raw = (args, env) => finRaw(sd, project, args, env);
  const console_ = (tag, payload) => socketCall(server, tag, payload, "console");
  const asPerson = (tag, payload) => consoleCall(server, tag, payload);
  const humanOnly = (args, env) => {
    const r = raw(args, env);
    assertEqual([r.status, r.json?.error?.code], [7, "HUMAN_ONLY"], `fin ${args.join(" ")}`);
  };
  const list = () => console_("plugin.list", {});
  const row = async (id) => (await list()).plugins.find((p) => p.id === id);
  const audit = async (kind, pick = () => true) => (await auditRows(server, kind)).filter(pick);
  /** A person's plan and install: the console's calls, with the typed code on the install. */
  const install = async (id, extra = {}, accept = {}) => {
    const plan = await console_("plugin.plan", { id, ...extra });
    const done = await asPerson("plugin.install", { planId: plan.planId, digest: plan.digest, accept });
    return { plan, done };
  };

  try {
    await start();
    fin(sd, tmp, ["project", "create", project, "--name", "Film"]);

    step(1, "the bundled pack is installed at start, without a plan");
    const first = f(["plugin", "ls"]);
    assertEqual(first.plugins.map((p) => [p.id, p.source, p.enabled, p.state]), [[BUNDLED, "bundled", true, "active"]], "plugin ls");
    assertEqual([first.bundled.integrity, first.bundled.plugins.map((p) => [p.id, p.ok])], ["hashesOnly", [[BUNDLED, true]]], "bundled status");
    need((await audit("plugin.install")).some((r) => r.principalKind === "system" && r.details.pluginId === BUNDLED), "no audit row for the bundled install");

    step(2, "a registry by `fin-plugin`; the console adds it by its root's hash");
    const teamDir = join(work, "registry-team");
    const team = makeRegistry(work, "team", teamDir, ["acme"]);
    team.publish({ id: "@acme/captions", version: "1.0.0", kind: "content", styles: ["punchy", "calm"] });
    team.publish({ id: "@acme/fonts", version: "1.0.0", kind: "content", fonts: [fontCatalog("acme-sans")] });
    team.publish({ id: "@acme/tool", version: "1.0.0", kind: "code", permissions: [{ perm: "project:read" }] });
    team.publish({ id: "@acme/extra", version: "1.0.0", kind: "content", styles: ["extra"] });
    const add = { cfg: { id: "team", path: teamDir }, root: { kind: "sha256", sha256: team.rootSha256 } };
    humanOnly(["registry", "add", "team", "--dir", teamDir, "--root-sha256", team.rootSha256]);
    humanOnly(["registry", "add", "team", "--dir", teamDir, "--root-sha256", team.rootSha256], agent());
    await failure(asPerson("registry.add", { ...add, root: { kind: "sha256", sha256: "0".repeat(64) } }), "REGISTRY_UNTRUSTED");
    need((await console_("registry.list", {})).registries.length === 0, "a wrong root added a registry");
    const added = await asPerson("registry.add", add);
    assertEqual(added.rootSha256, team.rootSha256, "the trusted root");
    need((await audit("registry.trust")).some((r) => r.outcome === "ok" && r.details.rootSha256 === team.rootSha256), "no registry.trust row");
    need((await audit("registry.add")).some((r) => r.outcome === "denied" || r.outcome === "failed"), "no row for the refused add");

    step(3, "refresh, search and info");
    const person = await console_("registry.refresh", { id: "team" });
    assertEqual([person.byHuman, person.results.map((r) => r.ok)], [true, [true]], "a person's refresh");
    assertEqual(f(["registry", "refresh"]).byHuman, false, "fin's refresh");
    assertEqual(f(["registry", "refresh"], agent()).byHuman, false, "an agent's refresh");
    need(f(["registry", "ls"]).registries.every((r) => r.path === undefined), "fin saw a registry's folder");
    need(f(["plugin", "search", "captions"]).plugins.some((p) => p.id === "@acme/captions"), "search finds the pack");
    assertEqual(f(["plugin", "search", "--kind", "code"]).plugins.map((p) => p.id), ["@acme/tool"], "search --kind code");
    assertEqual(f(["plugin", "info", "@acme/captions"]).registries.map((r) => r.registryId), ["team"], "plugin info");

    step(4, "installing is a person's: `fin`, an agent, and a person submitting a plan `fin` made are refused");
    humanOnly(["plugin", "install", "@acme/captions"]);
    humanOnly(["plugin", "install", "@acme/captions"], agent());
    const local = await socketCall(server, "plugin.plan", { id: "@acme/captions" }, "local");
    await failure(asPerson("plugin.install", { planId: local.planId, digest: local.digest }), "HUMAN_ONLY");
    await socketCall(server, "plugin.discard", { planId: local.planId }, "local");
    assertEqual((await list()).plugins.length, 1, "nothing was installed");
    need((await audit("plugin.install")).some((r) => r.outcome === "denied"), "no denied install row");

    step(5, "a person installs a content pack; `fin style add --from` copies its style");
    const captions = await install("@acme/captions", {}, {});
    assertEqual([captions.plan.permissions.length, captions.plan.risk, captions.done.plugin.enabled], [0, "low", true], "the pack's plan");
    need(!(captions.plan.warnings ?? []).some((w) => w.code === "thirdPartyCode"), "a content pack carries the code warning");
    const mine = f(["style", "add", "--name", "Mine", "--from", "@acme/captions:punchy"]);
    assertEqual(mine.resolved.pack, { id: "@acme/captions", version: "1.0.0", name: "punchy" }, "the style's source");
    assertEqual(f(["style", "add", "--name", "Preset", "--from", `${BUNDLED}:bold`]).resolved.pack.id, BUNDLED, "the bundled pack's style");
    assertEqual(raw(["style", "add", "--name", "X", "--from", "@acme/captions:nope"]).json.error.code, "NOT_FOUND", "an unknown style");

    step(6, "a font pack merges into the catalog; a pack that shadows a built-in family is not packed");
    const before = (await console_("fonts.catalog", {})).entries.map((e) => e.id);
    need(!before.includes("acme-sans"), "the font is there before its pack");
    await install("@acme/fonts");
    const merged = await console_("fonts.catalog", {});
    assertEqual(merged.entries.map((e) => e.id), [...before, "acme-sans"], "the merged catalog");
    // The publisher's tool holds a pack to the host's rule (the server's plan applies it too, with
    // the same code, to a package built by hand).
    const inter = writePlugin(work, { id: "@acme/inter", version: "1.0.0", kind: "content", fonts: [fontCatalog("acme-inter", "Inter")] });
    const shadow = finPlugin(["pack", inter.src, "-o", inter.out]);
    need(shadow.status === 3 && JSON.stringify(shadow.json).includes("catalogIdTaken"), `the shadowing pack was packed: ${shadow.text}`);

    step(7, "a code plugin: warning, explicit acceptance, installed switched off");
    const toolPlan = await console_("plugin.plan", { id: "@acme/tool" });
    need(toolPlan.warnings.some((w) => w.code === "thirdPartyCode"), "no thirdPartyCode warning");
    assertEqual(toolPlan.risk, "high", "a code plugin's risk");
    await failure(asPerson("plugin.install", { planId: toolPlan.planId, digest: toolPlan.digest, accept: {} }), "THIRD_PARTY_CODE_NOT_ACCEPTED");
    const tool = await asPerson("plugin.install", { planId: toolPlan.planId, digest: toolPlan.digest, accept: { thirdPartyCode: true } });
    assertEqual([tool.plugin.kind, tool.plugin.enabled], ["code", false], "the code plugin's row");
    await failure(asPerson("plugin.enable", { id: "@acme/tool" }), "PLUGIN_RUNTIME_UNAVAILABLE");
    need((await audit("plugin.install")).some((r) => r.details.pluginId === "@acme/tool" && r.details.thirdPartyCode?.accepted === true), "no acceptance on the install row");

    step(8, "plans are the maker's to discard, and capped");
    const plans = [];
    for (let i = 0; i < 4; i++) plans.push((await console_("plugin.plan", { id: "@acme/extra" })).planId);
    await failure(console_("plugin.plan", { id: "@acme/extra" }), "TOO_MANY_PLANS");
    await failure(socketCall(server, "plugin.discard", { planId: plans[0] }, "local"), "PLAN_EXPIRED");
    assertEqual((await console_("plugin.discard", { planId: plans[0] })).discarded, true, "the maker's discard");
    plans.shift();
    plans.push((await console_("plugin.plan", { id: "@acme/extra" })).planId);
    for (const id of plans) await console_("plugin.discard", { planId: id });

    step(9, "an update that adds `net` needs the acceptance again; an agent's refresh installs nothing");
    team.publish({ id: "@acme/tool", version: "1.1.0", kind: "code", permissions: [{ perm: "project:read" }, { perm: "net", origins: ["https://api.example.org"] }] });
    await console_("registry.refresh", { id: "team" });
    const upPlan = await console_("plugin.plan", { id: "@acme/tool", update: true });
    assertEqual([upPlan.action, upPlan.diff.added.length, upPlan.diff.riskUp], ["update", 1, true], "the update's diff");
    await failure(asPerson("plugin.install", { planId: upPlan.planId, digest: upPlan.digest }), "THIRD_PARTY_CODE_NOT_ACCEPTED");
    await asPerson("plugin.install", { planId: upPlan.planId, digest: upPlan.digest, accept: { thirdPartyCode: true } });
    assertEqual((await row("@acme/tool")).version, "1.1.0", "the updated version");
    need((await audit("plugin.grant")).some((r) => r.details.pluginId === "@acme/tool" && r.details.permissionsAdded === 1), "no grant row for the widening");
    // The pack was installed without automatic updates; a person turns them on with an update.
    team.publish({ id: "@acme/captions", version: "1.0.1", kind: "content", styles: ["punchy", "calm"] });
    const up = await console_("plugin.plan", { id: "@acme/captions", update: true });
    await asPerson("plugin.install", { planId: up.planId, digest: up.digest, autoUpdate: true });
    assertEqual((await row("@acme/captions")).autoUpdate, true, "automatic updates on");
    team.publish({ id: "@acme/captions", version: "1.0.2", kind: "content", styles: ["punchy", "calm"] });
    const viaAgent = f(["registry", "refresh"], agent());
    assertEqual([viaAgent.applied.updated, (await row("@acme/captions")).version], [[], "1.0.1"], "an agent's refresh installs nothing");
    const viaPerson = await console_("registry.refresh", { id: "team" });
    assertEqual([viaPerson.applied.updated.map((u) => [u.id, u.from, u.to]), (await row("@acme/captions")).version], [[["@acme/captions", "1.0.1", "1.0.2"]], "1.0.2"], "a person's refresh updates it");
    need((await audit("plugin.update")).some((r) => r.details.pluginId === "@acme/captions" && r.details.auto === true), "no automatic-update row");

    step(10, "advisories revoke; signed text cannot rewrite a terminal; a person removes");
    const hostile = team.advise({ plugin: "@acme/fonts", versions: "<2.0.0", action: "disable", summary: "update now\u001b[2J", id: "E2E-BAD" });
    need(hostile.status !== 0 && hostile.json?.code === "REGISTRY_INVALID", `a control character was signed: ${hostile.text}`);
    const advised = team.advise({ plugin: "@acme/fonts", versions: "<2.0.0", action: "disable", summary: "withdrawn in the e2e run", id: "E2E-1" });
    assertEqual(advised.status, 0, "the advisory is signed");
    const revoking = f(["registry", "refresh"], agent());
    assertEqual(revoking.applied.revoked.map((r) => [r.id, r.advisory]), [["@acme/fonts", "E2E-1"]], "an agent's refresh applies a disable advisory");
    assertEqual((await row("@acme/fonts")).state, "revoked", "the pack is revoked");
    need(!(await console_("fonts.catalog", {})).entries.some((e) => e.id === "acme-sans"), "a revoked pack's font is still offered");
    await failure(asPerson("plugin.enable", { id: "@acme/fonts" }), "PLUGIN_REVOKED");
    need((await audit("plugin.revoke")).some((r) => r.details.pluginId === "@acme/fonts" && r.code === "PLUGIN_REVOKED"), "no revoke row");
    humanOnly(["plugin", "rm", "@acme/fonts"]);
    await asPerson("plugin.remove", { id: "@acme/fonts" });
    need((await row("@acme/fonts")) === undefined, "the pack is still listed");
    await failure(asPerson("plugin.remove", { id: BUNDLED }), "PLUGIN_BUNDLED");
    need((await audit("plugin.remove")).some((r) => r.outcome === "ok" && r.details.pluginId === "@acme/fonts"), "no remove row");

    step(11, "a git clone reset to an older commit is refused; trust outlives the registry's removal");
    const gitDir = join(work, "registry-git");
    const gitReg = makeRegistry(work, "git", gitDir, ["gitco"]);
    gitReg.publish({ id: "@gitco/notes", version: "1.0.0", kind: "content", styles: ["notes"] });
    git(gitDir, "init", "-q");
    git(gitDir, "add", "-A");
    git(gitDir, "commit", "-q", "-m", "1.0.0");
    const c1 = git(gitDir, "rev-parse", "HEAD");
    const gitAdd = { cfg: { id: "gitreg", path: gitDir }, root: { kind: "sha256", sha256: gitReg.rootSha256 } };
    await asPerson("registry.add", gitAdd);
    await console_("registry.refresh", { id: "gitreg" });
    await install("@gitco/notes");
    gitReg.publish({ id: "@gitco/notes", version: "1.1.0", kind: "content", styles: ["notes"] });
    git(gitDir, "add", "-A");
    git(gitDir, "commit", "-q", "-m", "1.1.0");
    const c2 = git(gitDir, "rev-parse", "HEAD");
    await console_("registry.refresh", { id: "gitreg" });
    assertEqual(f(["plugin", "info", "@gitco/notes"]).registries[0].versions.map((v) => v.version).sort(), ["1.0.0", "1.1.0"], "the clone's newer commit is trusted");
    git(gitDir, "reset", "-q", "--hard", c1);
    const rolled = raw(["registry", "refresh", "gitreg"]);
    assertEqual([rolled.status, rolled.json?.error?.code], [7, "TUF_ROLLBACK"], "a reset clone");
    await failure(console_("registry.refresh", { id: "gitreg" }), "TUF_ROLLBACK");
    need((await audit("registry.verifyFail")).some((r) => r.details.registryId === "gitreg" && r.code === "TUF_ROLLBACK"), "no verifyFail row");
    assertEqual((await row("@gitco/notes")).enabled, true, "installed packs keep working");
    assertEqual(f(["style", "add", "--name", "Notes", "--from", "@gitco/notes:notes"]).resolved.pack.id, "@gitco/notes", "a style from the pack");
    await asPerson("registry.remove", { id: "gitreg" });
    await asPerson("registry.add", gitAdd);
    await failure(console_("registry.refresh", { id: "gitreg" }), "TUF_ROLLBACK"); // the trusted versions came back with the root
    git(gitDir, "reset", "-q", "--hard", c2);
    assertEqual((await console_("registry.refresh", { id: "gitreg" })).results[0].ok, true, "the clone caught up");

    step(12, "restarts: the owner's switch on a bundled pack survives a tampered and repaired resources folder");
    await stopServer(server);
    await start();
    assertEqual((await row("@acme/captions")).version, "1.0.2", "registry packs survive a restart");
    await asPerson("plugin.disable", { id: BUNDLED });
    await stopServer(server);
    const style = join(bundledDir, "captions-basic", "styles", "bold.json");
    const original = readFileSync(style);
    writeFileSync(style, original.toString("utf8").replace('"maxChars": 24', '"maxChars": 240'));
    await start();
    const torn = await list();
    assertEqual([torn.bundled.plugins[0].ok, torn.bundled.plugins[0].code, (await row(BUNDLED)).state], [false, "PLUGIN_BUNDLED_MISMATCH", "revoked"], "a tampered bundled pack");
    assertEqual(raw(["style", "add", "--name", "Torn", "--from", `${BUNDLED}:bold`]).json.error.code, "PLUGIN_BUNDLED_MISMATCH", "a revoked pack serves nothing");
    await stopServer(server);
    writeFileSync(style, original);
    await start();
    const repaired = await row(BUNDLED);
    assertEqual([repaired.enabled, repaired.state], [false, "disabled"], "the repair does not switch it on");

    step(13, "the journal holds no free text, and each step has its audit row");
    const entries = await storedEntries(server);
    const kinds = new Set(entries.map((e) => e.kind));
    for (const k of ["plugin.lifecycle", "registry.refresh"]) need(kinds.has(k), `no ${k} entry in the journal`);
    const needles = ["acme", "gitco", "punchy", "registry-team", "registry-git", tmp, teamDir, gitDir, "withdrawn in the e2e run", "E2E-1"];
    const found = leaks(filesUnder(join(sd, "events")), needles);
    need(found.length === 0, `free text in the journal: ${JSON.stringify(found.slice(0, 3))}`);
    need(!JSON.stringify(entries).match(/@acme|@gitco|registry-(team|git)|e2e run/), "free text in the stored entries");
    const rows = await auditRows(server);
    const has = (kind, outcome) => rows.some((r) => r.kind === kind && r.outcome === outcome);
    for (const [kind, outcome] of [
      ["registry.add", "ok"], ["registry.add", "denied"], ["registry.trust", "ok"], ["registry.remove", "ok"], ["registry.verifyFail", "failed"],
      ["plugin.install", "ok"], ["plugin.install", "denied"], ["plugin.update", "ok"], ["plugin.grant", "ok"], ["plugin.revoke", "ok"], ["plugin.remove", "ok"],
    ]) need(has(kind, outcome), `no ${outcome} audit row of ${kind}`);
    need(existsSync(join(sd, "plugins", "store")) && readdirSync(join(sd, "plugins", "store")).length > 0, "no store");
    log(`plugins: ok in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    if (server) await stopServer(server);
    for (const k of SWITCHES) if (saved[k] === undefined) delete process.env[k];
    else process.env[k] = saved[k];
    removeTmp(tmp);
  }
}
