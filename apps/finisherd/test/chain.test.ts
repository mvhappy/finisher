// The middleware chain (plan 5A Task 5) in-process: fake handlers that record what reaches
// them, a fake project registry, principals bound by hand.
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { defaultScopes, type FinError, HUMAN_ONLY, type Principal, type Probe, type Scope, SCOPES, WRITE_RPCS } from "@finisher/contracts";
import { probe } from "@finisher/fin-node";
import { Effect, Exit, Stream } from "effect";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { unauthenticated } from "../src/auth/handshake.ts";
import { checkScopes } from "../src/chain/authorize.ts";
import { type ChainDeps, chainOf, type PolicyRule, withChain, writeGuardOf } from "../src/chain/chain.ts";
import { confirmationRequired, issueChallenge } from "../src/auth/challenges.ts";
import { approvalRequired, branchNotAllowed, consentUiOnly, humanOnlyFixes, pathNotAllowed, scopeDenied } from "../src/chain/fixes.ts";
import { canonicalPath, type ChainProjects, PROJECT_FILE_MAX, projectIdIn } from "../src/chain/resolve.ts";
import { ConnectionRegistry, Connections } from "../src/connections.ts";
import { probeFor } from "../src/probe.ts";
import type { Audit, AuditInput } from "../src/control/audit.ts";
import { ANONYMOUS, CONSOLE, LOCAL } from "../src/principal.ts";
import { SAMPLE_PAYLOADS } from "./support/payloads.ts";

const A = "01aaaaaaaaaaaaaaaaaaaaaaaa";
const B = "01bbbbbbbbbbbbbbbbbbbbbbbb";
const MINT: Scope[] = ["server.read", "project.read", "project.write", "review.write", "jobs.read", "jobs.submit", "library.read", "library.scan", "diagnostics.read", "diagnostics.record"];
const agent = (more: Partial<Principal> = {}): Principal => ({ kind: "agent", id: "01agent000000000000000000a", scopes: MINT, projects: [A], branches: "*", human: false, ...more });
const mcp = (): Principal => ({ ...agent(), kind: "mcp", id: "01mcp00000000000000000000a" });

let tmp: string;
let dirA: string;
/** Whether file symlinks can be made here (not on non-elevated Windows without Developer Mode). */
let fileLinks = true;
let dirB: string;
let open: Set<string> = new Set();
const BRANCHES: Record<string, string> = { main: "main", feature: "br3", "feature#br3": "br3", br3: "br3", mine: "br5", br5: "br5" };

const projects: ChainProjects = {
  isOpen: (id) => open.has(id),
  dirOf: (id) => (id === A ? dirA : id === B ? dirB : undefined),
  resolveBranch: (_p, ref) => {
    const id = BRANCHES[ref];
    return id === undefined ? Effect.fail({ code: "BRANCH_NOT_FOUND", message: ref, details: {} } as unknown as FinError) : Effect.succeed(id);
  },
  branches: () =>
    Effect.succeed([
      { id: "main", createdBy: "user" },
      { id: "br3", createdBy: "agent:someone-else" },
      { id: "br5", createdBy: "agent:01agent000000000000000000a" },
    ]),
};

interface Seen {
  tag: string;
  payload: unknown;
  options: Record<string, unknown>;
}

/** A chain over recording handlers; `principal` is bound to client 1. */
function harness(principal: Principal, more: Partial<ChainDeps> = {}, real: Record<string, unknown> = {}) {
  const seen: Seen[] = [];
  const audited: AuditInput[] = [];
  const connections = new Connections("socket", () => "/sd/run/fin.sock");
  connections.attach(1, { close() {}, closeAfterReply() {}, terminate() {} });
  connections.get(1)!.principal = principal;
  const unary = (tag: string, answer: (p: Record<string, unknown>) => unknown = () => ({ ok: true })) => (payload: unknown, options: Record<string, unknown>) =>
    Effect.sync(() => {
      seen.push({ tag, payload, options });
      return answer(payload as Record<string, unknown>);
    });
  const handlers: Record<string, unknown> = {
    "project.create": unary("project.create"),
    "project.open": unary("project.open", (p) => ({ projectId: String(p.dir).endsWith("b") ? B : A, rev: 0 })),
    "project.takeover": unary("project.takeover"),
    "asset.probe": unary("asset.probe"),
    "tx.commit": unary("tx.commit"),
    "branch.create": unary("branch.create"),
    "fonts.status": unary("fonts.status"),
    "server.hello": unary("server.hello"),
    "ops.subscribe": (payload: unknown, options: Record<string, unknown>) => {
      seen.push({ tag: "ops.subscribe", payload, options });
      return Stream.make(1, 2);
    },
  };
  // Every writing RPC, and a read by id, record what reaches them too.
  for (const t of [...WRITE_RPCS, "snapshot.get", "project.acknowledgeRewind", "diagnostics.consent", "diagnostics.purge", "diagnostics.project", "diagnostics.export"]) handlers[t] ??= unary(t);
  Object.assign(handlers, real);
  const audit = { append: (e: AuditInput) => audited.push(e) } as unknown as Audit;
  const chained = withChain(handlers, { connections, projects, audit, ...more }) as unknown as Record<string, (p: unknown, o: unknown) => unknown>;
  const call = (tag: string, payload: unknown) =>
    Effect.runPromiseExit(chained[tag]!(payload, { client: { id: 1 }, headers: {} }) as Effect.Effect<unknown, FinError>);
  const stream = (tag: string, payload: unknown) =>
    Effect.runPromiseExit(Stream.runCollect(chained[tag]!(payload, { client: { id: 1 }, headers: {} }) as Stream.Stream<unknown, FinError>));
  /** A fresh `console.challenge` code of client 1. */
  const challenge = () => issueChallenge(connections.get(1)!).code;
  return { seen, audited, call, stream, challenge };
}

const code = (exit: Exit.Exit<unknown, FinError>): string | undefined =>
  Exit.isSuccess(exit) ? undefined : (Exit.findErrorOption(exit) as { value?: FinError }).value?.code;
const error = (exit: Exit.Exit<unknown, FinError>): FinError => (Exit.findErrorOption(exit) as { value: FinError }).value;
const commit = (branch: string) => ({ projectId: A, tx: { txId: "t1", branch, author: "a", ops: [] } });

beforeAll(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "finisherd-chain-")));
  dirA = join(tmp, "a");
  dirB = join(tmp, "b");
  for (const [d, id] of [[dirA, A], [dirB, B]] as const) {
    mkdirSync(join(d, "workspace"), { recursive: true });
    writeFileSync(join(d, "finisher.project.json"), JSON.stringify({ id, name: "n", schemaVersion: 1 }));
  }
  writeFileSync(join(dirA, "clip.png"), "x");
  writeFileSync(join(tmp, "outside.png"), "x");
  try {
    symlinkSync(join(tmp, "outside.png"), join(dirA, "workspace", "sneaky.png"));
  } catch (e) {
    // A file symlink on Windows needs SeCreateSymbolicLinkPrivilege (elevation or Developer
    // Mode); a non-elevated run skips the cases that need one.
    if (!WIN || (e as NodeJS.ErrnoException).code !== "EPERM") throw e;
    fileLinks = false;
    console.warn("chain.test: cannot create file symlinks here (non-elevated Windows); skipping the symlinked-file cases");
  }
});

afterAll(() => rmSync(tmp, { recursive: true, force: true }));

describe("the chain", () => {
  test("streams_pass_the_chain", async () => {
    const anon = harness(ANONYMOUS);
    expect(code(await anon.stream("ops.subscribe", { projectId: A, branch: "main", fromRev: 0 }))).toBe("UNAUTHENTICATED");
    const reader = harness(agent({ scopes: ["server.read"] }));
    expect(code(await reader.stream("ops.subscribe", { projectId: A, branch: "main", fromRev: 0 }))).toBe("SCOPE_DENIED");
    expect([...anon.seen, ...reader.seen]).toEqual([]);
    const ok = harness(LOCAL);
    const items = await ok.stream("ops.subscribe", { projectId: A, branch: "main", fromRev: 0 });
    expect(Exit.isSuccess(items)).toBe(true);
    expect(chainOf(ok.seen[0]!.options)?.principal.kind).toBe("local");
  });

  test("human_only_matrix", async () => {
    const kinds: Principal[] = [ANONYMOUS, LOCAL, agent({ scopes: SCOPES.filter((s) => !HUMAN_ONLY.has(s)) }), mcp(), CONSOLE];
    // Every human-only scope: only a human principal passes, whatever else it holds.
    for (const scope of HUMAN_ONLY) {
      for (const p of kinds) {
        const refused = checkScopes("test.rpc", [scope], { ...p, scopes: [...SCOPES] });
        expect(refused?.code, `${scope} × ${p.kind}`).toBe(p.human ? undefined : "HUMAN_ONLY");
      }
    }
    // Every escalation, through the chain.
    open = new Set([A]);
    const escalations: Array<[string, Record<string, unknown>]> = [
      ["project.takeover", { projectId: A, confirm: true }],
      ["project.open", { dir: dirA, relocate: true }],
      // 5A Task 7: the diagnostics operations a person keeps.
      ["diagnostics.consent", { purpose: "diagnostics", value: "off" }],
      ["diagnostics.consent", { purpose: "training", value: "unset" }],
      ["diagnostics.purge", { selector: "all" }],
      ["diagnostics.project", { projectId: A, collect: "default" }],
      ["diagnostics.export", { content: true }],
    ];
    for (const [tag, payload] of escalations) {
      for (const p of kinds) {
        const h = harness(p);
        // The console types its connection's code back (Decision 30).
        const got = code(await h.call(tag, p.kind === "console" ? { ...payload, challenge: h.challenge() } : payload));
        const want = p.kind === "console" ? undefined : p.kind === "anonymous" ? "UNAUTHENTICATED" : "HUMAN_ONLY";
        expect(got, `${tag} × ${p.kind}`).toBe(want);
        expect(h.seen.length, `${tag} × ${p.kind}`).toBe(want === undefined ? 1 : 0);
      }
    }
    // Without the escalation, local may.
    expect(code(await harness(LOCAL).call("project.takeover", { projectId: A, confirm: false }))).toBeUndefined();
    expect(code(await harness(LOCAL).call("diagnostics.project", { projectId: A, collect: "never" }))).toBeUndefined();
    expect(code(await harness(LOCAL).call("diagnostics.export", {}))).toBeUndefined();
  });

  test("all_listed_scopes_are_required", async () => {
    open = new Set();
    // Loading a project needs project.read and project.write: holding one is not enough.
    const h = harness(agent({ scopes: ["project.write"] }));
    const refused = error(await h.call("project.open", { dir: dirA }));
    expect(refused).toMatchObject({ code: "SCOPE_DENIED", details: { scopes: ["project.read"] } });
  });

  test("open_needs_write_unless_attached", async () => {
    const reader = agent({ scopes: ["project.read"] });
    open = new Set([A]);
    expect(code(await harness(reader).call("project.open", { dir: dirA }))).toBeUndefined();
    open = new Set();
    expect(error(await harness(reader).call("project.open", { dir: dirA }))).toMatchObject({ code: "SCOPE_DENIED", details: { scopes: ["project.write"] } });
  });

  test("create_and_open_reach_by_directory", async () => {
    open = new Set([A, B]);
    const h = harness(agent({ scopes: [...MINT, "project.create"] }));
    // Another's project and no project at all answer the same, without an id (review of Task 6, 5).
    const theirs = error(await h.call("project.open", { dir: dirB }));
    const none = error(await h.call("project.open", { dir: join(tmp, "no-project-here") }));
    expect(theirs.code).toBe("PROJECT_NOT_FOUND");
    expect([theirs.message, theirs.details, theirs.fix]).toEqual([none.message, none.details, none.fix]);
    expect(JSON.stringify(theirs)).not.toContain(B);
    expect(code(await h.call("project.create", { dir: join(tmp, "new"), name: "N", rate: "30" }))).toBe("PATH_NOT_ALLOWED");
    expect(code(await h.call("project.create", { dir: join(dirA, "workspace", "new"), name: "N", rate: "30" }))).toBeUndefined();
    expect(h.seen.map((s) => s.tag)).toEqual(["project.create"]);
    // Unrestricted principals reach every folder.
    expect(code(await harness(LOCAL).call("project.open", { dir: dirB }))).toBeUndefined();
  });

  test("probe_outside_project_is_path_not_allowed", async () => {
    open = new Set([A]);
    const h = harness(agent());
    expect(code(await h.call("asset.probe", { path: join(dirA, "clip.png") }))).toBeUndefined();
    // A file that does not exist yet is its nearest existing folder's real path plus the rest.
    expect(code(await h.call("asset.probe", { path: join(dirA, "workspace", "later.png") }))).toBeUndefined();
    expect(code(await h.call("asset.probe", { path: join(tmp, "outside.png") }))).toBe("PATH_NOT_ALLOWED");
    // A symlink inside the project is followed: its target is outside.
    if (fileLinks) expect(code(await h.call("asset.probe", { path: join(dirA, "workspace", "sneaky.png") }))).toBe("PATH_NOT_ALLOWED");
    expect(code(await h.call("asset.probe", { path: "relative.png" }))).toBe("INVALID_PATH");
    // The handler gets the canonical path.
    expect(h.seen[0]!.payload).toMatchObject({ path: join(dirA, "clip.png") });
    expect(code(await harness(LOCAL).call("asset.probe", { path: join(tmp, "outside.png") }))).toBeUndefined();
  });

  test("name_and_id_resolve_the_same", async () => {
    for (const limit of [["br3"], ["feature"]]) {
      const h = harness(agent({ branches: limit }));
      for (const ref of ["feature", "feature#br3", "br3"]) {
        expect(code(await h.call("tx.commit", commit(ref))), `${limit} ${ref}`).toBeUndefined();
      }
      expect(code(await h.call("tx.commit", commit("main")))).toBe("BRANCH_NOT_ALLOWED");
      // The handler gets the ids the store must re-check inside its write transaction.
      for (const s of h.seen) expect(chainOf(s.options)?.guard).toEqual({ denyBranches: [], allowBranches: ["br3"], credential: "agent:01agent000000000000000000a" });
    }
  });

  test("agent_branch_limits", async () => {
    const h = harness(agent({ branches: ["own"] }));
    expect(error(await h.call("tx.commit", commit("main")))).toMatchObject({ code: "BRANCH_NOT_ALLOWED", details: { branch: "main" } });
    expect(code(await h.call("tx.commit", commit("feature")))).toBe("BRANCH_NOT_ALLOWED");
    expect(code(await h.call("tx.commit", commit("mine")))).toBeUndefined();
    // Acknowledging a rewind acts on the branch: limited the same way (review of Task 6, 7).
    const ack = (branch: string) => ({ projectId: A, branch, generation: 1 });
    expect(code(await h.call("project.acknowledgeRewind", ack("main")))).toBe("BRANCH_NOT_ALLOWED");
    expect(code(await h.call("project.acknowledgeRewind", ack("mine")))).toBeUndefined();
    // Making a branch is always the writer's own.
    expect(code(await h.call("branch.create", { projectId: A, txId: "t", author: "a", from: "main" }))).toBeUndefined();
    // No limits: an open guard.
    const local = harness(LOCAL);
    await local.call("tx.commit", commit("main"));
    expect(chainOf(local.seen[0]!.options)?.guard).toEqual({ denyBranches: [] });
  });

  test("ask_answers_approval_required", async () => {
    const approvals = new Map();
    const ask: PolicyRule = () => Effect.succeed({ ask: { reason: "testRule" } });
    const h = harness(LOCAL, { policies: [ask], approvals });
    const refused = error(await h.call("fonts.status", {}));
    expect(refused.code).toBe("APPROVAL_REQUIRED");
    const id = (refused.details as { approvalId: string }).approvalId;
    expect(id).toMatch(/^[0-9a-z]{26}$/i);
    expect(approvals.get(id)).toMatchObject({ tag: "fonts.status", principal: "local:local" });
    expect(h.seen).toEqual([]);
  });

  test("check_order_is_fixed", async () => {
    // Unauthenticated before resolving: a malformed path from `anonymous` is UNAUTHENTICATED.
    expect(code(await harness(ANONYMOUS).call("asset.probe", { path: "relative" }))).toBe("UNAUTHENTICATED");
    // Resolving before authorising: INVALID_PATH, then the scopes.
    const noScopes = agent({ scopes: [] });
    expect(code(await harness(noScopes).call("asset.probe", { path: "relative" }))).toBe("INVALID_PATH");
    expect(code(await harness(noScopes).call("asset.probe", { path: join(dirA, "clip.png") }))).toBe("SCOPE_DENIED");
    // Human-only before the other scopes; scopes before reach; reach before policy.
    expect(code(await harness(agent({ scopes: [] })).call("project.takeover", { projectId: B, confirm: true }))).toBe("HUMAN_ONLY");
    expect(code(await harness(agent({ scopes: [] })).call("tx.commit", { ...commit("main"), projectId: B }))).toBe("SCOPE_DENIED");
    expect(code(await harness(agent({ branches: ["own"] })).call("tx.commit", { ...commit("main"), projectId: B }))).toBe("PROJECT_NOT_FOUND");
    // The public hello passes for anonymous.
    expect(code(await harness(ANONYMOUS).call("server.hello", {}))).toBeUndefined();
  });

  test("denials_are_audited", async () => {
    const h = harness(agent({ branches: ["own"] }));
    await h.call("project.takeover", { projectId: A, confirm: true });
    await h.call("tx.commit", commit("main"));
    await h.call("asset.probe", { path: join(tmp, "outside.png") });
    await h.call("asset.probe", { path: "relative" });
    expect(h.audited.map((e) => [e.kind, e.code, e.details?.rule, e.details?.rpc, e.principal.kind])).toEqual([
      ["auth.denied", "HUMAN_ONLY", "humanOnly", "project.takeover", "agent"],
      ["auth.denied", "BRANCH_NOT_ALLOWED", "agentBranchLimits", "tx.commit", "agent"],
      ["auth.denied", "PATH_NOT_ALLOWED", "reach", "asset.probe", "agent"],
    ]);
    expect(h.audited[0]!.details).toMatchObject({ scope: "project.confirm", transport: "socket" });
    // Never the payload.
    expect(JSON.stringify(h.audited)).not.toContain(tmp);
    expect(JSON.stringify(h.audited)).not.toContain("outside");
  });

  test("fix_texts_contain_no_command", () => {
    const p = agent();
    const all = [
      ...humanOnlyFixes(),
      scopeDenied("tx.commit", ["project.write"], p),
      pathNotAllowed("asset.probe", "/x"),
      branchNotAllowed("tx.commit", "main"),
      approvalRequired("tx.commit", "01approval0000000000000000", "testRule"),
      unauthenticated("unproved"),
      consentUiOnly("diagnostics.consent", p),
      confirmationRequired("diagnostics.purge", "missing"),
    ];
    for (const e of all) {
      expect(e.fix, e.code).toBeDefined();
      expect(e.fix, e.code).not.toMatch(/`|\bfinisherd [a-z-]+|\bfin [a-z-]+/);
    }
    for (const e of [...humanOnlyFixes(), consentUiOnly("diagnostics.consent", p)]) {
      expect(e.fix).toMatch(/^Only you, the owner of this Finisher server, can /);
      expect(["console", "app"]).toContain((e.details as { where?: unknown }).where);
    }
    expect(defaultScopes("local")).not.toContain("project.confirm");
  });
});

const WIN = process.platform === "win32";

describe("review of Task 5", () => {
  const limited = (more: Partial<Principal> = {}) => agent({ scopes: SCOPES.filter((x) => !HUMAN_ONLY.has(x)), branches: ["own"], ...more });
  /** A payload for `tag` whose write targets are the agent's own branch (`mine`, br5). */
  const ownWrite = (tag: string): Record<string, unknown> => {
    const p: Record<string, unknown> = { ...SAMPLE_PAYLOADS[tag as keyof typeof SAMPLE_PAYLOADS], projectId: A, author: "someone-else" };
    for (const f of ["branch", "into"]) if (f in p) p[f] = "mine";
    if (tag === "tx.commit") p.tx = { txId: "t", branch: "mine", author: "a", ops: [] };
    return p;
  };

  test("H1: a branch-limited principal's every write carries allowBranches", async () => {
    open = new Set([A]);
    for (const tag of WRITE_RPCS) {
      const h = harness(limited());
      expect(code(await h.call(tag, ownWrite(tag))), tag).toBeUndefined();
      expect(chainOf(h.seen[0]!.options)?.guard, tag).toEqual({ denyBranches: [], allowBranches: ["br5"], credential: "agent:01agent000000000000000000a" });
    }
    // Unlimited principals: no allow-list.
    const local = harness(LOCAL);
    await local.call("project.recover", ownWrite("project.recover"));
    expect(chainOf(local.seen[0]!.options)?.guard).toEqual({ denyBranches: [] });
  });

  test("H1/L3: a write outside the chain, or limited without an allow-list, is a defect", async () => {
    // A defect: a failure with no typed error (the handler's `defectsAsErrors` makes it INTERNAL).
    const die = async (options: object) => {
      const exit = await Effect.runPromiseExit(writeGuardOf(options));
      return Exit.isFailure(exit) && Exit.findErrorOption(exit)._tag === "None";
    };
    expect(await die({})).toBe(true);
    expect(await die({ guard: { denyBranches: [] }, principal: limited() })).toBe(true);
    const own = { denyBranches: [], allowBranches: [], credential: "agent:01agent000000000000000000a" };
    expect(await Effect.runPromise(writeGuardOf({ guard: own, principal: limited() }))).toEqual(own);
    // Task 9: a credential's write names that credential, or it is a defect too.
    expect(await die({ guard: { denyBranches: [], allowBranches: [] }, principal: limited() })).toBe(true);
    expect(await die({ guard: { ...own, credential: "agent:01someoneelse000000000000" }, principal: limited() })).toBe(true);
    expect(await die({ guard: { denyBranches: [], credential: "agent:x" }, principal: LOCAL })).toBe(true);
    expect(await Effect.runPromise(writeGuardOf({ guard: { denyBranches: [] }, principal: LOCAL }))).toEqual({ denyBranches: [] });
  });

  test("M4: a branch-limited principal's new branches carry its own author", async () => {
    open = new Set([A]);
    for (const tag of ["branch.create", "project.recover"]) {
      const h = harness(limited());
      await h.call(tag, ownWrite(tag));
      expect(h.seen[0]!.payload, tag).toMatchObject({ author: "agent:01agent000000000000000000a" });
      const local = harness(LOCAL);
      await local.call(tag, ownWrite(tag));
      expect(local.seen[0]!.payload, tag).toMatchObject({ author: "someone-else" });
    }
  });

  test("review of Tasks 8–9, M1: an agent without branch limits cannot plant a branch for another", async () => {
    open = new Set([A]);
    const free = limited({ branches: "*" });
    for (const p of [free, { ...free, kind: "mcp" as const, id: "01mcp00000000000000000000a" }, { ...free, kind: "device" as const, id: "01dev00000000000000000000a" }]) {
      for (const tag of ["branch.create", "project.recover"]) {
        const h = harness(p);
        await h.call(tag, { ...ownWrite(tag), author: "agent:01victim0000000000000000a" });
        expect(h.seen[0]!.payload, `${p.kind} ${tag}`).toMatchObject({ author: `${p.kind}:${p.id}` });
      }
    }
  });

  test("H2: project.open and project.create get the canonical folder and what was checked", async () => {
    const link = join(tmp, "link-to-a");
    symlinkSync(dirA, link, WIN ? "junction" : "dir");
    open = new Set();
    const h = harness(LOCAL);
    await h.call("project.open", { dir: link });
    expect(h.seen[0]!.payload).toMatchObject({ dir: dirA });
    expect(chainOf(h.seen[0]!.options)).toMatchObject({ openedProject: A, facts: { attached: false } });
    open = new Set([A]);
    await h.call("project.open", { dir: link });
    expect(chainOf(h.seen[1]!.options)).toMatchObject({ openedProject: A, facts: { attached: true } });
    await h.call("project.create", { dir: join(link, "workspace", "fresh", "x"), name: "N", rate: "30" });
    expect(h.seen[2]!.payload).toMatchObject({ dir: join(dirA, "workspace", "fresh", "x") });
  });

  test("M1: serving a project that is not open (a reopen) needs project.write", async () => {
    const reader = agent({ scopes: ["project.read"] });
    open = new Set([A]);
    expect(code(await harness(reader).call("snapshot.get", { projectId: A, branch: "main" }))).toBeUndefined();
    open = new Set();
    expect(error(await harness(reader).call("snapshot.get", { projectId: A, branch: "main" }))).toMatchObject({ code: "SCOPE_DENIED", details: { scopes: ["project.write"] } });
    expect(code(await harness(agent()).call("snapshot.get", { projectId: A, branch: "main" }))).toBeUndefined();
  });

  test("L1: reach is checked before the open state, so it leaks nothing", async () => {
    const reader = agent({ scopes: ["project.read"] });
    for (const state of [new Set<string>(), new Set([B])]) {
      open = state;
      expect(code(await harness(reader).call("project.open", { dir: dirB })), `B open: ${state.has(B)}`).toBe("PROJECT_NOT_FOUND");
      expect(code(await harness(reader).call("snapshot.get", { projectId: B, branch: "main" })), `B open: ${state.has(B)}`).toBe("PROJECT_NOT_FOUND");
    }
  });

  test("M2: a symlinked folder above a missing file is followed", async () => {
    open = new Set([A]);
    const outside = join(tmp, "elsewhere");
    mkdirSync(outside, { recursive: true });
    symlinkSync(outside, join(dirA, "workspace", "out"), WIN ? "junction" : "dir");
    const h = harness(agent());
    expect(code(await h.call("asset.probe", { projectId: A, path: join(dirA, "workspace", "out", "later.png") }))).toBe("PATH_NOT_ALLOWED");
    expect(await canonicalPath(join(dirA, "workspace", "out", "a", "b.png"))).toBe(join(outside, "a", "b.png"));
    expect(await canonicalPath(join(dirA, "clip.png"))).toBe(join(dirA, "clip.png"));
    // Protocol 2: only the probe's own project's folder counts.
    open = new Set([A, B]);
    const both = harness(agent({ projects: [A, B] }));
    expect(code(await both.call("asset.probe", { projectId: B, path: join(dirA, "clip.png") }))).toBe("PATH_NOT_ALLOWED");
    expect(code(await both.call("asset.probe", { projectId: A, path: join(dirA, "clip.png") }))).toBeUndefined();
  });

  // The path check cannot see through a dangling symlink (it canonicalises to itself), and a
  // symlink may be swapped in after it: the real probe opens beneath the root, through none.
  test.skipIf(WIN)("a dangling or swapped symlink cannot redirect a probe", async () => {
    open = new Set([A]);
    const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="12" height="8"/>';
    const secret = join(tmp, "secret.svg");
    writeFileSync(secret, svg);
    writeFileSync(join(dirA, "workspace", "ok.svg"), svg);
    const link = join(dirA, "workspace", "flip.svg");
    symlinkSync("/nonexistent/secret.svg", link);
    // Run between the chain's check and the probe: the swap an attacker races for.
    let swap = () => {};
    const reached: string[] = [];
    const real = {
      "asset.probe": (p: { projectId?: string; path: string }, o: object) =>
        Effect.suspend(() => {
          reached.push(p.path);
          swap();
          return probeFor(chainOf(o)?.principal, projects, p.projectId, p.path);
        }),
    };
    const h = harness(agent(), {}, real);
    const probed = async (path: string) => h.call("asset.probe", { projectId: A, path });
    const ok = await probed(join(dirA, "workspace", "ok.svg"));
    expect(Exit.isSuccess(ok) && (ok.value as Probe).fingerprint).toMatch(/^fp1:/);
    // Dangling: the chain passes it (inside the project), the open refuses it.
    const dangling = await probed(link);
    expect(error(dangling)).toMatchObject({ code: "PATH_NOT_ALLOWED", details: { path: link } });
    expect(reached).toEqual([join(dirA, "workspace", "ok.svg"), link]);
    // Swapped to a file outside after the check: refused, no fingerprint or size of it.
    swap = () => {
      rmSync(link);
      symlinkSync(secret, link);
    };
    const swapped = await probed(link);
    expect(error(swapped).code).toBe("PATH_NOT_ALLOWED");
    expect(JSON.stringify(error(swapped))).not.toContain("fp1:");
    expect(JSON.stringify(error(swapped))).not.toContain(secret);
    // What the swap would have read with a plain probe.
    expect((JSON.parse(await probe(link)) as Probe).fingerprint).toMatch(/^fp1:/);
    // Unrestricted principals keep the plain probe, which follows a symlink.
    swap = () => {};
    const local = harness(LOCAL, {}, real);
    const followed = await local.call("asset.probe", { projectId: A, path: link });
    expect(Exit.isSuccess(followed) && (followed.value as Probe).image).toEqual({ width: 12, height: 8 });
  });

  test("M3: finisher.project.json is read only as a small regular file", async () => {
    const dir = join(tmp, "m3");
    mkdirSync(dir);
    const file = join(dir, "finisher.project.json");
    writeFileSync(file, JSON.stringify({ id: A }));
    expect(await projectIdIn(dir)).toBe(A);
    writeFileSync(file, JSON.stringify({ id: A, pad: "x".repeat(PROJECT_FILE_MAX) }));
    expect(await projectIdIn(dir)).toBeUndefined();
    rmSync(file);
    mkdirSync(file);
    expect(await projectIdIn(dir)).toBeUndefined();
    rmSync(file, { recursive: true });
    if (!WIN) {
      symlinkSync(join(dirA, "finisher.project.json"), file);
      expect(await projectIdIn(dir)).toBeUndefined();
      rmSync(file);
      if (spawnSync("mkfifo", [file]).status === 0) {
        const t0 = Date.now();
        expect(await projectIdIn(dir)).toBeUndefined();
        expect(Date.now() - t0).toBeLessThan(2_000);
      }
    }
  });

  test("M5: stream refusals are counted in rpc.stats", async () => {
    const counted: string[] = [];
    const timed = <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.tapError(self, (e) => Effect.sync(() => counted.push(`${tag}:${e.code}`)));
    const h = harness(agent({ scopes: ["server.read"] }), { timed });
    await h.stream("ops.subscribe", { projectId: A, branch: "main", fromRev: 0 });
    await h.call("tx.commit", commit("main"));
    expect(counted).toEqual(["ops.subscribe:SCOPE_DENIED", "tx.commit:SCOPE_DENIED"]);
  });
});

describe("revocation (review of Task 7, M3)", () => {
  test("a request on a connection whose credential was revoked is CREDENTIAL_REVOKED", async () => {
    const registry = new ConnectionRegistry();
    const connections = registry.add(new Connections("socket", () => "/sd/run/fin.sock"));
    const me = agent({ scopes: ["server.read"] });
    connections.attach(1, { close() {}, closeAfterReply() {}, terminate() {} }, me);
    const audited: AuditInput[] = [];
    const reached: string[] = [];
    const chained = withChain(
      { "fonts.status": () => Effect.sync(() => void reached.push("fonts.status")) },
      { connections, projects, audit: { append: (e: AuditInput) => audited.push(e) } as unknown as Audit },
    ) as unknown as Record<string, (p: unknown, o: unknown) => Effect.Effect<unknown, FinError>>;
    const call = () => Effect.runPromiseExit(chained["fonts.status"]!({}, { client: { id: 1 }, headers: {} }));
    expect(code(await call())).toBeUndefined();
    registry.revoke(me);
    expect(code(await call())).toBe("CREDENTIAL_REVOKED");
    expect(reached).toEqual(["fonts.status"]);
    expect(audited.at(-1)).toMatchObject({ kind: "auth.denied", code: "CREDENTIAL_REVOKED", details: { rule: "revoked" } });
  });
});

describe("review of Tasks 8–9", () => {
  test("L6: a principal's root from a stale index entry (the folder no longer names its project) is no root", async () => {
    const moved = join(tmp, "moved-away");
    mkdirSync(join(moved, "workspace"), { recursive: true });
    writeFileSync(join(moved, "finisher.project.json"), JSON.stringify({ id: "01someoneelse000000000000x", name: "n", schemaVersion: 1 }));
    open = new Set();
    const creator = agent({ scopes: ["project.create"], projects: [B] });
    const stale = harness(creator, { projects: { ...projects, isOpen: () => false, dirOf: () => undefined, indexedDirOf: () => moved } });
    expect(code(await stale.call("project.create", { dir: join(moved, "workspace", "new"), name: "N", rate: "30" }))).toBe("PATH_NOT_ALLOWED");
    // The index still right: the folder names the project, so it is a root.
    const fresh = harness(creator, { projects: { ...projects, isOpen: () => false, dirOf: () => undefined, indexedDirOf: () => dirB } });
    expect(code(await fresh.call("project.create", { dir: join(dirB, "workspace", "new"), name: "N", rate: "30" }))).toBeUndefined();
  });

  test("Task 8: server.stop needs no code from the console, but a device with auth.admin must confirm it", async () => {
    const stop = { "server.stop": (_: unknown) => Effect.succeed({ stopping: true }) };
    expect(code(await harness(CONSOLE, {}, stop).call("server.stop", {}))).toBeUndefined();
    const device: Principal = { kind: "device", id: "01device00000000000000000a", scopes: ["auth.admin", "server.read"], projects: "*", branches: "*", human: true };
    const h = harness(device, {}, stop);
    expect(code(await h.call("server.stop", {}))).toBe("CONFIRMATION_REQUIRED");
    expect(h.audited.map((e) => e.kind)).toEqual(["console.refused"]);
  });
});
