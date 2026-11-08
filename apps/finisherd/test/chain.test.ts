// The middleware chain (plan 5A Task 5) in-process: fake handlers that record what reaches
// them, a fake project registry, principals bound by hand.
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { defaultScopes, type FinError, HUMAN_ONLY, type Principal, type Scope, SCOPES } from "@finisher/contracts";
import { Effect, Exit, Stream } from "effect";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { unauthenticated } from "../src/auth/handshake.ts";
import { checkScopes } from "../src/chain/authorize.ts";
import { type ChainDeps, chainOf, type PolicyRule, withChain } from "../src/chain/chain.ts";
import { approvalRequired, branchNotAllowed, humanOnlyFixes, pathNotAllowed, scopeDenied } from "../src/chain/fixes.ts";
import type { ChainProjects } from "../src/chain/resolve.ts";
import { Connections } from "../src/connections.ts";
import type { Audit, AuditInput } from "../src/control/audit.ts";
import { ANONYMOUS, CONSOLE, LOCAL } from "../src/principal.ts";

const A = "01aaaaaaaaaaaaaaaaaaaaaaaa";
const B = "01bbbbbbbbbbbbbbbbbbbbbbbb";
const MINT: Scope[] = ["server.read", "project.read", "project.write", "review.write", "jobs.read", "jobs.submit", "library.read", "library.scan", "diagnostics.read", "diagnostics.record"];
const agent = (more: Partial<Principal> = {}): Principal => ({ kind: "agent", id: "01agent000000000000000000a", scopes: MINT, projects: [A], branches: "*", human: false, ...more });
const mcp = (): Principal => ({ ...agent(), kind: "mcp", id: "01mcp00000000000000000000a" });

let tmp: string;
let dirA: string;
let dirB: string;
let open: Set<string>;
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
function harness(principal: Principal, more: Partial<ChainDeps> = {}) {
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
  const handlers = {
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
  const audit = { append: (e: AuditInput) => audited.push(e) } as unknown as Audit;
  const chained = withChain(handlers, { connections, projects, audit, ...more }) as unknown as Record<string, (p: unknown, o: unknown) => unknown>;
  const call = (tag: string, payload: unknown) =>
    Effect.runPromiseExit(chained[tag]!(payload, { client: { id: 1 }, headers: {} }) as Effect.Effect<unknown, FinError>);
  const stream = (tag: string, payload: unknown) =>
    Effect.runPromiseExit(Stream.runCollect(chained[tag]!(payload, { client: { id: 1 }, headers: {} }) as Stream.Stream<unknown, FinError>));
  return { seen, audited, call, stream };
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
  symlinkSync(join(tmp, "outside.png"), join(dirA, "workspace", "sneaky.png"));
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
    const escalations: Array<[string, unknown]> = [
      ["project.takeover", { projectId: A, confirm: true }],
      ["project.open", { dir: dirA, relocate: true }],
    ];
    for (const [tag, payload] of escalations) {
      for (const p of kinds) {
        const h = harness(p);
        const got = code(await h.call(tag, payload));
        const want = p.kind === "console" ? undefined : p.kind === "anonymous" ? "UNAUTHENTICATED" : "HUMAN_ONLY";
        expect(got, `${tag} × ${p.kind}`).toBe(want);
        expect(h.seen.length, `${tag} × ${p.kind}`).toBe(want === undefined ? 1 : 0);
      }
    }
    // Without the escalation, local may.
    expect(code(await harness(LOCAL).call("project.takeover", { projectId: A, confirm: false }))).toBeUndefined();
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
    expect(code(await h.call("project.open", { dir: dirB }))).toBe("PROJECT_NOT_FOUND");
    expect(code(await h.call("project.open", { dir: join(tmp, "no-project-here") }))).toBe("PROJECT_NOT_FOUND");
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
    expect(code(await h.call("asset.probe", { path: join(dirA, "workspace", "later.png") }))).toBeUndefined();
    expect(code(await h.call("asset.probe", { path: join(tmp, "outside.png") }))).toBe("PATH_NOT_ALLOWED");
    // A symlink inside the project is followed: its target is outside.
    expect(code(await h.call("asset.probe", { path: join(dirA, "workspace", "sneaky.png") }))).toBe("PATH_NOT_ALLOWED");
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
      for (const s of h.seen) expect(chainOf(s.options)?.guard).toEqual({ denyBranches: [], allowBranches: ["br3"] });
    }
  });

  test("agent_branch_limits", async () => {
    const h = harness(agent({ branches: ["own"] }));
    expect(error(await h.call("tx.commit", commit("main")))).toMatchObject({ code: "BRANCH_NOT_ALLOWED", details: { branch: "main" } });
    expect(code(await h.call("tx.commit", commit("feature")))).toBe("BRANCH_NOT_ALLOWED");
    expect(code(await h.call("tx.commit", commit("mine")))).toBeUndefined();
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
    ];
    for (const e of all) {
      expect(e.fix, e.code).toBeDefined();
      expect(e.fix, e.code).not.toMatch(/`|\bfinisherd [a-z-]+|\bfin [a-z-]+/);
    }
    for (const e of humanOnlyFixes()) {
      expect(e.fix).toMatch(/^Only you, the owner of this Finisher server, can /);
      expect(e.details).toMatchObject({ where: "console" });
    }
    expect(defaultScopes("local")).not.toContain("project.confirm");
  });
});
