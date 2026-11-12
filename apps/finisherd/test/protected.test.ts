// Protected branches (5E plan Task 6, Decision 16, spec §8.5): the `protectedBranches` policy
// rule refuses a write into a protected branch from a principal without `branch.protected`
// (`BRANCH_PROTECTED`), and gives such a principal's every write the protected ids as the
// guard's `denyBranches`, which the store checks again inside the write transaction. `local`
// keeps the scope (owner decision 5); minted `agent`/`mcp` credentials do not have it.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { defaultScopes, type FinError, HUMAN_ONLY, MINT_DEFAULT_SCOPES, type Principal, SCOPES, type Scope, WRITE_RPCS, ruleOf } from "@finisher/contracts";
import type { WriteGuard } from "@finisher/core-types";
import { Engine } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { chainOf, type PolicyRule, writeGuardOf } from "../src/chain/chain.ts";
import { DEFAULT_POLICIES } from "../src/chain/policy.ts";
import { protectedBranches } from "../src/chain/protected.ts";
import type { ChainProjects } from "../src/chain/resolve.ts";
import { LOCAL } from "../src/principal.ts";
import { chainHarness, codeOf, errorOf } from "./support/chain-harness.ts";
import { SAMPLE_PAYLOADS } from "./support/payloads.ts";

const A = "01aaaaaaaaaaaaaaaaaaaaaaaa";
const ITEM = "01iiiiiiiiiiiiiiiiiiiiiiii";
const AGENT_ID = "01agent000000000000000000a";
/** Every scope an agent may hold but the two that reach protected branches. */
const WIDE: Scope[] = SCOPES.filter((s) => !HUMAN_ONLY.has(s) && s !== "branch.protected" && s !== "review.decide");
const agent = (scopes: ReadonlyArray<Scope> = WIDE, more: Partial<Principal> = {}): Principal => ({
  kind: "agent",
  id: AGENT_ID,
  scopes,
  projects: [A],
  branches: "*",
  human: false,
  ...more,
});

const BRANCHES: Record<string, string> = { main: "main", feature: "br3", br3: "br3", release: "br4", br4: "br4", mine: "br5", br5: "br5" };
const fakeProjects: ChainProjects = {
  isOpen: () => true,
  dirOf: () => "/p",
  resolveBranch: (_p, ref) => {
    const id = BRANCHES[ref];
    return id === undefined ? Effect.fail({ code: "BRANCH_NOT_FOUND", message: ref, details: {} } as unknown as FinError) : Effect.succeed(id);
  },
  branches: () => Effect.succeed([{ id: "main", createdBy: "user" }, { id: "br3", createdBy: "user" }, { id: "br5", createdBy: `agent:${AGENT_ID}` }]),
};

/** The fake reviews: one item whose target is `itemTarget`; the project protects `protectedIds`. */
let itemTarget = "main";
let protectedIds: string[] = ["main"];
const reviews = { item: () => ({ projectId: A, source: "br5", target: itemTarget }), protectedBranches: () => protectedIds };

beforeEach(() => {
  itemTarget = "main";
  protectedIds = ["main"];
});

const harness = (p: Principal) => chainHarness({ 1: p }, { projects: fakeProjects, reviews });

/** A payload for `tag` that writes into (or acts on) the branch `ref`. */
function writeInto(tag: string, ref: string): Record<string, unknown> {
  const p: Record<string, unknown> = { ...SAMPLE_PAYLOADS[tag as keyof typeof SAMPLE_PAYLOADS], projectId: A };
  for (const f of ["branch", "into"]) if (f in p) p[f] = ref;
  if ("itemId" in p) p.itemId = ITEM;
  if (tag === "tx.commit") p.tx = { txId: "t", branch: ref, author: "a", ops: [] };
  return p;
}

/** The write RPCs and the RPCs that act on a branch without a record (`targets`). */
const CHECKED = [...WRITE_RPCS, "project.acknowledgeRewind"].sort();

describe("the policy stage", () => {
  test("protected_branches_runs_after_agent_branch_limits", () => {
    expect(DEFAULT_POLICIES).toContain(protectedBranches);
    expect(DEFAULT_POLICIES.indexOf(protectedBranches)).toBe(1);
    // Owner decision 5: `local` keeps both scopes; a minted credential has neither by default.
    expect(defaultScopes("local")).toEqual(expect.arrayContaining(["branch.protected", "review.decide"]));
    for (const s of ["branch.protected", "review.decide"] as const) expect(MINT_DEFAULT_SCOPES).not.toContain(s);
  });

  test("local_writes_main", async () => {
    const h = harness(LOCAL);
    for (const tag of CHECKED) {
      expect(codeOf(await h.call(1, tag, writeInto(tag, "main"))), tag).toBeUndefined();
    }
    // Nothing is denied inside the transaction either.
    for (const s of h.seen) expect(chainOf(s.options)?.guard, s.tag).toEqual({ denyBranches: [] });
  });

  test("every_write_rpc_checked", async () => {
    const h = harness(agent());
    for (const tag of CHECKED) {
      const rule = ruleOf(tag as never);
      const exit = await h.call(1, tag, writeInto(tag, "main"));
      if (rule.writes?.length === 0) {
        // A new branch, a recovery: no existing branch is named, the guard limits what it changes.
        expect(codeOf(exit), tag).toBeUndefined();
      } else if (rule.protectedNeeds !== undefined) {
        // Their scope stands in for `branch.protected`: without it they stop at stage 4.
        expect(codeOf(exit), tag).toBe("SCOPE_DENIED");
      } else {
        expect(errorOf(exit), tag).toMatchObject({ code: "BRANCH_PROTECTED", details: { tag, branch: "main" } });
      }
    }
    expect(h.seen.map((s) => s.tag).sort()).toEqual(CHECKED.filter((t) => ruleOf(t as never).writes?.length === 0));
    // Refusals at stage 5 are audited with the rule's name, never the payload.
    const denied = h.audited.filter((e) => e.code === "BRANCH_PROTECTED");
    expect(denied.length).toBeGreaterThan(0);
    for (const e of denied) expect(e).toMatchObject({ kind: "auth.denied", details: { rule: "protectedBranches" } });
    // Every write elsewhere goes ahead, carrying the protected ids for the store to check again.
    itemTarget = "br3";
    const ok = harness(agent([...WIDE, "review.decide"]));
    for (const tag of CHECKED) {
      expect(codeOf(await ok.call(1, tag, writeInto(tag, "feature"))), tag).toBeUndefined();
    }
    expect(ok.seen).toHaveLength(CHECKED.length);
    for (const s of ok.seen) expect(chainOf(s.options)?.guard, s.tag).toEqual({ denyBranches: ["main"], credential: `agent:${AGENT_ID}` });
  });

  test("an mcp credential is refused like an agent", async () => {
    const h = harness(agent(WIDE, { kind: "mcp", id: "01mcp00000000000000000000a" }));
    expect(codeOf(await h.call(1, "tx.commit", writeInto("tx.commit", "main")))).toBe("BRANCH_PROTECTED");
    // A credential that was given `branch.protected` writes there.
    const given = harness(agent([...WIDE, "branch.protected"]));
    expect(codeOf(await given.call(1, "tx.commit", writeInto("tx.commit", "main")))).toBeUndefined();
    expect(chainOf(given.seen[0]!.options)?.guard).toEqual({ denyBranches: [], credential: `agent:${AGENT_ID}` });
  });

  test("the project's policy names what is protected, by id", async () => {
    protectedIds = ["main", "br4"];
    const h = harness(agent());
    for (const ref of ["release", "br4"]) expect(codeOf(await h.call(1, "tx.commit", writeInto("tx.commit", ref))), ref).toBe("BRANCH_PROTECTED");
    expect(codeOf(await h.call(1, "tx.commit", writeInto("tx.commit", "feature")))).toBeUndefined();
    expect(chainOf(h.seen[0]!.options)?.guard.denyBranches).toEqual(["main", "br4"]);
    // A project that protects nothing: no refusal and nothing denied.
    protectedIds = [];
    const open = harness(agent());
    expect(codeOf(await open.call(1, "tx.commit", writeInto("tx.commit", "main")))).toBeUndefined();
    expect(chainOf(open.seen[0]!.options)?.guard.denyBranches).toEqual([]);
  });

  test("branch_set_archive_of_main_refused_for_agent", async () => {
    protectedIds = ["main", "br4"];
    const h = harness(agent());
    const set = (branch: string, more: object) => ({ projectId: A, txId: "s1", author: "a", branch, ...more });
    expect(codeOf(await h.call(1, "branch.set", set("main", { archived: true })))).toBe("BRANCH_PROTECTED");
    expect(codeOf(await h.call(1, "branch.set", set("release", { archived: true })))).toBe("BRANCH_PROTECTED");
    expect(codeOf(await h.call(1, "branch.set", set("release", { name: "old-release" })))).toBe("BRANCH_PROTECTED");
    expect(h.seen).toEqual([]);
    // A person's `fin` (local) archives or renames it.
    const local = harness(LOCAL);
    expect(codeOf(await local.call(1, "branch.set", set("release", { archived: true })))).toBeUndefined();
  });

  test("review_merge_and_drop_need_review_decide_on_protected", async () => {
    protectedIds = ["main", "br4"];
    const reviewer = harness(agent([...WIDE, "review.decide"]));
    expect(codeOf(await reviewer.call(1, "history.drop", writeInto("history.drop", "main")))).toBeUndefined();
    for (const tag of ["review.decide", "review.merge", "review.close"]) {
      expect(codeOf(await reviewer.call(1, tag, writeInto(tag, "main"))), tag).toBeUndefined();
    }
    // The target is let through; the other protected branches stay denied in the transaction.
    for (const s of reviewer.seen) expect(chainOf(s.options)?.guard.denyBranches, s.tag).toEqual(["br4"]);
    // `review.decide` does not stand in for `branch.protected` on any other write.
    expect(codeOf(await reviewer.call(1, "tx.commit", writeInto("tx.commit", "main")))).toBe("BRANCH_PROTECTED");
    expect(codeOf(await reviewer.call(1, "history.merge", writeInto("history.merge", "main")))).toBe("BRANCH_PROTECTED");
    // `branch.protected` alone does not reach them: the review scope is asked at stage 4.
    const writer = harness(agent([...WIDE, "branch.protected"]));
    for (const tag of ["history.drop", "review.decide", "review.merge", "review.close"]) {
      expect(codeOf(await writer.call(1, tag, writeInto(tag, "main"))), tag).toBe("SCOPE_DENIED");
    }
    expect(codeOf(await writer.call(1, "tx.commit", writeInto("tx.commit", "main")))).toBeUndefined();
    // A drop into an unprotected branch needs `project.write` only.
    const plain = harness(agent());
    expect(codeOf(await plain.call(1, "history.drop", writeInto("history.drop", "feature")))).toBeUndefined();
    expect(chainOf(plain.seen[0]!.options)?.guard.denyBranches).toEqual(["main", "br4"]);
  });
});

describe("inside the write transaction", () => {
  let tmp: string;
  let engine: Engine | undefined;
  beforeEach(() => {
    tmp = mkdtempSync(join(tmpdir(), "finisherd-protected-"));
  });
  afterEach(async () => {
    await engine?.close().catch(() => undefined);
    engine = undefined;
    rmSync(tmp, { recursive: true, force: true });
  });

  const addAsset = { op: "asset.add", id: "$a", path: "/media/a.mov", duration: "600", video: { width: 1920, height: 1080, rate: "30" }, audio: null, fingerprint: "fp" };
  const insert = { op: "clip.insert", id: "$c", track: "t1", at: "0", asset: "a1", sourceIn: "0", duration: "5" };

  /** A project with `feature` (br1) and `release` (br2); the chain resolves through its store. */
  async function project(): Promise<{ e: Engine; projects: ChainProjects }> {
    const e = await Engine.create(join(tmp, "sd"), join(tmp, "p"), "Demo", "30");
    engine = e;
    await e.commit(JSON.stringify({ txId: "t0", author: "u", branch: "main", baseRev: 0, schemaVersion: 2, ops: [addAsset] }));
    for (const [txId, name] of [["b1", "feature"], ["b2", "release"]]) await e.branchCreate(JSON.stringify({ txId, author: "u", name, from: "main" }));
    const projects: ChainProjects = {
      ...fakeProjects,
      resolveBranch: (_p, ref) => Effect.tryPromise({ try: async () => JSON.parse(await e.resolveBranch(ref)) as string, catch: (x) => x as FinError }),
    };
    return { e, projects };
  }

  /** `tx.commit` through the store, with the guard the chain gave the call. */
  const committing = (e: Engine) => ({
    "tx.commit": (payload: { tx: Record<string, unknown> }, options: object) =>
      Effect.flatMap(writeGuardOf(options), (guard) =>
        Effect.tryPromise({
          try: async () => JSON.parse(await e.commit(JSON.stringify({ ...payload.tx, baseRev: await e.head(String(payload.tx.branch)).catch(() => 0), schemaVersion: 2 }), undefined, JSON.stringify({ guard }))),
          catch: (x) => x as FinError,
        }),
      ),
  });
  const commitTo = (branch: string, txId: string) => ({ projectId: A, tx: { txId, branch, author: "a", ops: [insert] } });
  const refusal = async (p: Promise<unknown>) => p.then(() => undefined, (x: { code?: string; details?: unknown }) => x);

  test("agent_commit_to_main_refused_at_stage_and_in_txn", async () => {
    const { e, projects } = await project();
    const h = chainHarness({ 1: agent() }, { projects, reviews: { item: () => undefined, protectedBranches: () => ["main"] } }, () => committing(e));
    const before = await e.head("main");
    // Stage 5 refuses it: the handler is not reached.
    expect(errorOf(await h.call(1, "tx.commit", commitTo("main", "w1")))).toMatchObject({ code: "BRANCH_PROTECTED", details: { branch: "main" } });
    expect(await e.head("main")).toBe(before);
    // An allowed write carries `denyBranches`...
    let guard: WriteGuard | undefined;
    const seeing = chainHarness({ 1: agent() }, { projects, reviews: { item: () => undefined, protectedBranches: () => ["main"] } }, () => ({
      "tx.commit": (_: unknown, options: object) => Effect.map(writeGuardOf(options), (g) => void (guard = g)),
    }));
    expect(codeOf(await seeing.call(1, "tx.commit", commitTo("feature", "w2")))).toBeUndefined();
    expect(guard).toEqual({ denyBranches: ["main"], credential: `agent:${AGENT_ID}` });
    // ...so with the stage bypassed, the store refuses the write into `main` in its transaction.
    const tx = { txId: "w3", author: "a", branch: "main", baseRev: before, schemaVersion: 2, ops: [insert] };
    expect(await refusal(e.commit(JSON.stringify(tx), undefined, JSON.stringify({ guard })))).toMatchObject({ code: "BRANCH_PROTECTED" });
    expect(await e.head("main")).toBe(before);
    // `local` writes it (owner decision 5).
    const local = chainHarness({ 1: LOCAL }, { projects, reviews: { item: () => undefined, protectedBranches: () => ["main"] } }, () => committing(e));
    expect(codeOf(await local.call(1, "tx.commit", commitTo("main", "w4")))).toBeUndefined();
    expect(await e.head("main")).toBeGreaterThan(before);
  });

  test("rename_between_policy_and_write_caught", async () => {
    const { e, projects } = await project();
    const release = JSON.parse(await e.resolveBranch("release")) as string;
    const feature = JSON.parse(await e.resolveBranch("feature")) as string;
    // After the protected rule allowed a write into `feature`, `release` takes its name.
    const rename: PolicyRule = () =>
      Effect.promise(async () => {
        await e.branchSet(JSON.stringify({ txId: "rn1", author: "u", branch: feature, name: "old-feature" }));
        await e.branchSet(JSON.stringify({ txId: "rn2", author: "u", branch: release, name: "feature" }));
        return undefined;
      });
    const h = chainHarness(
      { 1: agent() },
      { projects, reviews: { item: () => undefined, protectedBranches: () => ["main", release] }, policies: [...DEFAULT_POLICIES, rename] },
      () => committing(e),
    );
    const before = await e.head(release);
    expect(errorOf(await h.call(1, "tx.commit", commitTo("feature", "w1")))).toMatchObject({ code: "BRANCH_PROTECTED", details: { branch: release } });
    expect(await e.head(release)).toBe(before);
  });
});
