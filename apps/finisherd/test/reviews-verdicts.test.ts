// `review.verdict` entries of the review workflow (5E plan Task 7, Decision 20) and the rule that
// review text never reaches the audit or the journal (but `reason`, training only).
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type CredentialFile, MINT_DEFAULT_SCOPES, type ReviewItem, type ReviewView } from "@finisher/contracts";
import type { BranchView } from "@finisher/core-types";
import { Effect, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { b64u, canonicalEndpoint, proveMac } from "../src/auth/crypto.ts";
import { settingsPath } from "../src/config.ts";
import { serve, StderrLogger } from "../src/main.ts";
import { journalReviewVerdicts, MAX_VERDICT_TX_IDS, verdictBy, verdictEntries } from "../src/reviews/verdicts.ts";
import { entries } from "./support/agent-journal.ts";
import { addAsset, commit, confirmed, connectProved, hello, NdjsonClient, ok, type ServerInfo, tx } from "./support/ndjson-client.ts";

const ITEM: ReviewItem = {
  id: "01jc00000000000000000item1",
  projectId: "p",
  kind: "branchReview",
  state: "open",
  source: "br1",
  target: "main",
  title: "t",
  requestedBy: { principal: "local:local" },
  createdAt: 1_000_000,
  updatedAt: 1_000_000,
  rederived: false,
};

describe("verdict entries", () => {
  test("verdict_entries_per_call: at most 64 ids each, counting their own; by; openS", () => {
    const ids = Array.from({ length: 130 }, (_, i) => `tx${i}`);
    const out = verdictEntries({ item: ITEM, verdict: "rejected", by: "user", txIds: ids, counts: "rejected", now: 1_000_000 + 90_500 });
    expect(out.map((e) => e.txIds.length)).toEqual([MAX_VERDICT_TX_IDS, MAX_VERDICT_TX_IDS, 2]);
    expect(out.map((e) => [e.rejected, e.accepted])).toEqual([
      [64, 0],
      [64, 0],
      [2, 0],
    ]);
    expect(out.flatMap((e) => e.txIds)).toEqual(ids);
    for (const e of out) expect(e).toMatchObject({ itemKind: "branchReview", verdict: "rejected", by: "user", openS: 90 });
    expect(out[0]).not.toHaveProperty("via");
    // No ids: one entry, no counts.
    expect(verdictEntries({ item: ITEM, verdict: "dismissed", by: "agent", txIds: [], counts: "none", sessionId: "s1", now: 0 })).toEqual([
      { itemKind: "branchReview", verdict: "dismissed", by: "agent", txIds: [], accepted: 0, rejected: 0, sessionId: "s1", openS: 0 },
    ]);
    expect(["console", "device", "local", "agent", "mcp"].map((kind) => verdictBy({ kind } as never))).toEqual(["user", "user", "user", "agent", "agent"]);
  });

  test("the sink keeps reason only under training, and records nothing off or for a `never` project", () => {
    const recorded: Array<{ ctx: string; payload: Record<string, unknown>; source: string }> = [];
    const journal = { enabled: true, training: false, recordsProject: (p: string) => p !== "never", record: (_k: string, _v: number, ctx: string | null | undefined, payload: string, source: string) => void recorded.push({ ctx: ctx ?? "", payload: JSON.parse(payload), source }) };
    const sink = journalReviewVerdicts(journal as never, () => 1);
    const entry = verdictEntries({ item: ITEM, verdict: "rejected", by: "user", txIds: ["a"], counts: "rejected", reason: "too loud", now: 0 });
    sink.emit("p", { kind: "user" }, entry);
    expect(recorded[0]).toMatchObject({ source: "finisherd", payload: { verdict: "rejected", txIds: ["a"] } });
    expect(recorded[0]!.payload).not.toHaveProperty("reason");
    expect(JSON.parse(recorded[0]!.ctx)).toEqual({ project: "p", actor: { kind: "user" } });
    journal.training = true;
    sink.emit("p", undefined, entry);
    expect(recorded[1]!.payload.reason).toBe("too loud");
    sink.emit("never", undefined, entry);
    journal.enabled = false;
    sink.emit("p", undefined, entry);
    expect(recorded).toHaveLength(2);
    // A journal that throws never fails the caller.
    const warned: string[] = [];
    journalReviewVerdicts({ ...journal, enabled: true, record: () => { throw new Error("EVENT_INVALID"); } } as never, () => 1, (m) => warned.push(m)).emit("p", undefined, entry);
    expect(warned).toHaveLength(1);
  });
});

// --- Through a real server ------------------------------------------------------------------

let tmp: string;
let serverData: string;
let running: { fiber: Fiber.Fiber<unknown, unknown>; info: ServerInfo } | undefined;
const clients: NdjsonClient[] = [];

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-verdicts-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  for (const k of ["DO_NOT_TRACK", "FIN_DIAGNOSTICS", "FIN_TRAINING"]) delete process.env[k];
});
afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.close().catch(() => undefined)));
  if (running !== undefined) await Effect.runPromise(Fiber.interrupt(running.fiber));
  running = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

async function start(privacy: object): Promise<void> {
  writeFileSync(settingsPath(serverData), JSON.stringify({ privacy }));
  let ready!: (i: ServerInfo) => void;
  const info = new Promise<ServerInfo>((r) => (ready = r));
  const fiber = Effect.runFork(serve({ serverData, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)));
  running = { fiber, info: await info };
}

async function connect(name: "local" | "console"): Promise<NdjsonClient> {
  const c = await connectProved({ info: running!.info, serverData }, name);
  clients.push(c);
  return c;
}

async function agentClient(file: CredentialFile): Promise<NdjsonClient> {
  const info = running!.info;
  const client = await NdjsonClient.connect(info.socket);
  clients.push(client);
  const { clientNonce, exit } = await hello(client, { env: file.env, endpoint: canonicalEndpoint(info.socket), credential: { kind: file.kind, id: file.id } });
  const answer = ok<{ envId: string; endpoint: string; serverNonce: string }>(exit);
  const mac = proveMac(b64u.decode(file.key)!, answer.envId, answer.endpoint, b64u.decode(answer.serverNonce)!, clientNonce);
  ok(await client.call("auth.prove", { mac: b64u.encode(mac) }));
  return client;
}

const edit = (txId: string) => ({ ...addAsset(`$${txId}`), path: `/media/${txId}.mov`, fingerprint: txId });

/** Review text the tests look for everywhere it must not be. */
const TITLE = "Secret-Title-Words";
const SUMMARY = "Secret-Summary-Words";
const COMMENT = "Secret-Comment-Words";
const REASON = "Secret-Reason-Words";

async function scenario(local: NdjsonClient): Promise<{ projectId: string; items: ReviewItem[] }> {
  const { projectId } = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "p"), name: "Demo", rate: "30" }));
  ok(await commit(local, projectId, tx("m-asset", 0, [addAsset()])));
  const items: ReviewItem[] = [];
  for (const name of ["one", "two"]) {
    const { branch } = ok<{ branch: BranchView }>(await local.call("branch.create", { projectId, txId: `b-${name}`, author: "ann", name, from: "main" }));
    for (const t of ["1", "2", "3"]) {
      const head = ok<{ branches: BranchView[] }>(await local.call("branch.list", { projectId })).branches.find((b) => b.id === branch.id)!.head;
      ok(await commit(local, projectId, { ...tx(`${name}${t}`, head, [edit(`${name}${t}`)]), branch: branch.id }));
    }
    const { item } = ok<{ item: ReviewItem }>(await local.call("review.request", { projectId, source: branch.id, title: TITLE, summary: SUMMARY }));
    ok(await local.call("review.comment", { itemId: item.id, body: COMMENT, txId: `${name}1` }));
    items.push(item);
  }
  return { projectId, items };
}

const verdicts = () => entries(serverData, "events").filter((e) => e.kind === "review.verdict");

describe("verdicts through the server", () => {
  test("verdict_entries_per_call: decide batches, changes requested, merge and close; by user and agent", async () => {
    await start({ diagnostics: "on" });
    const local = await connect("local");
    const { items } = await scenario(local);
    const [one, two] = items as [ReviewItem, ReviewItem];
    const decided = ok<ReviewView>(
      await local.call("review.decide", {
        itemId: one.id,
        expect: one.snapshot,
        decisions: [
          { txId: "one1", decision: "accept" },
          { txId: "one2", decision: "accept" },
          { txId: "one3", decision: "reject", reason: REASON },
        ],
        requestChanges: { reason: REASON },
      }),
    );
    ok(await local.call("review.merge", { itemId: one.id, expect: decided.item.snapshot, idemKey: "m1" }));
    // An agent with `review.decide` closes the second item.
    const console = await connect("console");
    const { file } = ok<{ file: CredentialFile }>(await confirmed(console, "auth.credential.mint", { kind: "agent", label: "t", scopes: [...MINT_DEFAULT_SCOPES, "review.decide"] }));
    const agent = await agentClient(file);
    ok(await agent.call("review.close", { itemId: two.id, state: "rejected" }));
    ok(await local.call("diagnostics.status", {}));
    const seen: Array<Record<string, any>> = verdicts().map((e) => e.payload); // eslint-disable-line @typescript-eslint/no-explicit-any
    const shape = seen.map((p) => [p.verdict, p.by, p.accepted, p.rejected, p.itemKind]);
    expect(shape).toEqual([
      ["accepted", "user", 2, 0, "branchReview"],
      ["rejected", "user", 0, 1, "branchReview"],
      ["changesRequested", "user", 0, 0, "branchReview"],
      ["merged", "user", 2, 0, "branchReview"],
      ["rejected", "agent", 0, 3, "branchReview"],
    ]);
    expect(seen[0]!.txIds).toHaveLength(2);
    expect(seen[4]!.txIds).toHaveLength(3);
    expect(seen[4]).toHaveProperty("sessionId");
    for (const p of seen) {
      expect(p).not.toHaveProperty("reason");
      expect(p).not.toHaveProperty("via");
      expect(typeof p.openS).toBe("number");
    }
  });

  test("reason is journalled only while training is on", async () => {
    await start({ diagnostics: "on", training: "on" });
    const local = await connect("local");
    const { items } = await scenario(local);
    const one = items[0]!;
    ok(await local.call("review.decide", { itemId: one.id, expect: one.snapshot, decisions: [{ txId: "one1", decision: "reject", reason: REASON }] }));
    ok(await local.call("diagnostics.status", {}));
    expect(verdicts().map((e) => e.payload.reason)).toEqual([REASON]);
  });

  test("review_text_never_audited_or_journalled", async () => {
    await start({ diagnostics: "on" });
    const local = await connect("local");
    const { items } = await scenario(local);
    const [one, two] = items as [ReviewItem, ReviewItem];
    const decided = ok<ReviewView>(
      await local.call("review.decide", { itemId: one.id, expect: one.snapshot, decisions: [{ txId: "one1", decision: "reject", reason: REASON }, { txId: "one2", decision: "accept", reason: REASON }], requestChanges: { reason: REASON } }),
    );
    ok(await local.call("review.merge", { itemId: one.id, expect: decided.item.snapshot, idemKey: "m1", acceptPending: true }));
    ok(await local.call("review.close", { itemId: two.id, state: "rejected" }));
    // A stale expect's error carries the change list, never the item's text.
    const stale = await local.call("review.decide", { itemId: two.id, expect: { ...two.snapshot!, sourceSeq: 0 }, decisions: [] });
    expect(stale.ok).toBe(false);
    ok(await local.call("diagnostics.status", {}));
    const console = await connect("console");
    const audit = ok<{ entries: Array<{ kind: string; details: Record<string, unknown> }> }>(await console.call("auth.audit.list", { limit: 1000 }));
    expect(audit.entries.filter((e) => e.kind === "history.drop").map((e) => e.details.item)).toEqual([two.id, one.id]);
    expect(audit.entries.some((e) => e.kind === "review.merge" && e.details.item === one.id && e.details.protected === true)).toBe(true);
    const everything = JSON.stringify([audit.entries, entries(serverData, "events"), stale]);
    for (const secret of [TITLE, SUMMARY, COMMENT, REASON]) expect(everything).not.toContain(secret);
  });
});
