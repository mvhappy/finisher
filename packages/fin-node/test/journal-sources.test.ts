// Edit-path sources in fin-node (plan D, Task D5): `log.commit` for every write path,
// classified from typed results and the store's write note; `storage.state`, `eval.stats`,
// `fonts.fetch` and `error.internal`. Each test opens the server's journal first, then its
// engines, which find it at open.
import { copyFileSync, existsSync, mkdtempSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { hostname, tmpdir } from "node:os";
import { join } from "node:path";
import type { Envelope, ExportPage } from "@finisher/core-types";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { Engine, type FinNodeError, FontManager, Journal, testHooks } from "../index.js";
import { type CatalogFile, catalogJson, catalogPin, FontServer, waitFor } from "./fontserver.ts";

let tmp: string;
let serverData: string;
let projectDir: string;
let journals: Journal[] = [];
let engines: Engine[] = [];

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "fin-node-sources-"));
  serverData = join(tmp, "server-data");
  projectDir = join(tmp, "My Project");
});

afterEach(async () => {
  for (const e of engines) await e.close().catch(() => undefined);
  engines = [];
  for (const j of journals) await j.close();
  journals = [];
  rmSync(tmp, { recursive: true, force: true });
});

/** A canonical ULID (stored raw; anything else is keyed). */
const U = (n: number): string => `01JC${String(n).padStart(22, "0")}`;
const KEYED = /^k1:[0-9a-f]{32}$/;

function open(privacy: unknown = null, data = serverData, testOptions?: { statsEveryMs?: number }): Journal {
  const j = Journal.open(data, privacy === null ? null : JSON.stringify(privacy), hostname(), testOptions);
  journals.push(j);
  return j;
}

async function create(data = serverData): Promise<Engine> {
  const e = await Engine.create(data, projectDir, "Demo", "30");
  engines.push(e);
  return e;
}

function tx(txId: string, baseRev: number, ops: unknown[], branch = "main", author = "test"): string {
  return JSON.stringify({ txId, author, branch, baseRev, schemaVersion: 2, ops });
}

const addAsset = {
  op: "asset.add",
  id: "$a",
  path: "/media/a.mov",
  duration: "600",
  video: { width: 1920, height: 1080, rate: "30" },
  audio: null,
  fingerprint: "fp",
};

const insert = (at: number, duration = 5): object => ({
  op: "clip.insert",
  id: "$c",
  track: "t1",
  at: String(at),
  asset: "a1",
  sourceIn: "0",
  duration: String(duration),
});

const req = (o: object) => JSON.stringify({ author: "test", ...o });

async function rejection(p: Promise<unknown>): Promise<FinNodeError> {
  try {
    await p;
  } catch (e) {
    return e as FinNodeError;
  }
  throw new Error("expected a rejection");
}

async function entries(j: Journal, filter: object = {}): Promise<Envelope[]> {
  const out: Envelope[] = [];
  let cursor: string | undefined;
  for (;;) {
    const page = JSON.parse(await j.export(JSON.stringify(filter), cursor, 1000)) as ExportPage;
    out.push(...page.lines);
    if (page.next === undefined) return out;
    cursor = page.next;
  }
}

// biome-ignore lint/suspicious/noExplicitAny: payloads are checked field by field
type Commit = Envelope & { payload: any };

async function commits(j: Journal): Promise<Commit[]> {
  return (await entries(j)).filter((e) => e.kind === "log.commit") as Commit[];
}

const byTx = (all: Commit[], txId: string) => all.filter((e) => e.ctx?.txId === txId);

/** `list[i]`, which must exist. */
function at<T>(list: readonly T[], i: number): T {
  const x = list.at(i);
  if (x === undefined) throw new Error(`no entry ${i} of ${list.length}`);
  return x;
}

type Rows<N extends number, R extends Commit[] = []> = R["length"] extends N ? R : Rows<N, [...R, Commit]>;

/** The first `n` entries of `list`, which must hold at least `n`. */
function rows<N extends number>(list: Commit[], n: N): Rows<N> {
  expect(list.length).toBeGreaterThanOrEqual(n);
  return list.slice(0, n) as Rows<N>;
}

/** A project with the asset (rev 1) and one clip (rev 2) on main, by ULID txIds 1 and 2. */
async function project(): Promise<Engine> {
  const e = await create();
  await e.commit(tx(U(1), 0, [addAsset]));
  await e.commit(tx(U(2), 1, [insert(0)]));
  return e;
}

describe("log.commit from edits", () => {
  test("commit_outcomes_accepted_rebased_deduped_conflict_refused", async () => {
    const j = open();
    const e = await project();
    await e.commit(tx(U(3), 1, [insert(20, 1)])); // stale by one rev: rebased
    await e.commit(tx(U(1), 0, [addAsset])); // a retry: deduped
    await e.commit(tx(U(4), 3, [{ op: "clip.delete", clip: "c1" }]));
    expect((await rejection(e.commit(tx(U(5), 3, [{ op: "clip.delete", clip: "c1" }])))).code).toBe("CONFLICT");
    expect((await rejection(e.commit(tx(U(6), 99, [addAsset])))).code).toBe("BASE_REV_AHEAD");
    const all = await commits(j);
    expect(all.map((c) => c.payload.record)).toEqual(Array(all.length).fill("edit"));
    expect(all.map((c) => [c.ctx?.txId, c.payload.outcome])).toEqual([
      [U(1), "accepted"],
      [U(2), "accepted"],
      [U(3), "rebased"],
      [U(1), "deduped"],
      [U(4), "accepted"],
      [U(5), "conflict"],
      [U(6), "refused"],
    ]);
    const [first, , rebased, deduped, , conflict, refused] = rows(all, 7);
    expect(first.payload).toMatchObject({ ops: { "asset.add": 1 }, baseRev: 0 });
    expect(first.payload.rebasedOver).toBeUndefined();
    expect(first.payload.code).toBeUndefined();
    expect(rebased.payload).toMatchObject({ baseRev: 1, rebasedOver: 1, ops: { "clip.insert": 1 } });
    expect(deduped.payload.rebasedOver).toBeUndefined();
    expect(conflict.payload.code).toBe("CONFLICT");
    expect(Object.values(conflict.payload.conflicts as Record<string, number>).reduce((a, b) => a + b, 0)).toBeGreaterThan(0);
    expect(refused.payload).toMatchObject({ code: "BASE_REV_AHEAD", baseRev: 99 });
    for (const c of all) {
      expect(c.source).toBe("finNode");
      expect(c.ctx?.project).toBe(e.projectId);
      expect(c.ctx?.branch).toBe("main");
      expect(typeof c.payload.micros).toBe("number");
      expect(c.payload.intent).toBeUndefined();
      expect(c.payload.links).toBeUndefined();
    }
  });

  test("actor_from_meta_and_unknown_without", async () => {
    const j = open();
    const e = await create();
    const meta = JSON.stringify({ actor: { kind: "agent", session: "my-agent-session", call: U(90) } });
    await e.commit(tx(U(1), 0, [addAsset]), undefined, meta);
    await e.commit(tx(U(2), 1, [insert(0)]));
    await e.checkpoint(req({ txId: U(3), branch: "main", name: "v1" }), meta);
    const [withMeta, without, cp] = rows(await commits(j), 3);
    expect(withMeta.ctx?.actor).toEqual({ kind: "agent", session: expect.stringMatching(KEYED), call: U(90) });
    expect(without.ctx?.actor).toEqual({ kind: "unknown" });
    expect(cp.ctx?.actor).toEqual(withMeta.ctx?.actor);
    // A meta that does not decode is an unknown actor, never a failed commit.
    await e.commit(tx(U(4), 3, [insert(10)]), undefined, "{ nope");
    expect((await commits(j)).at(-1)?.ctx?.actor).toEqual({ kind: "unknown" });
  });

  test("ctx_author_is_hashed", async () => {
    const j = open();
    const e = await create();
    await e.commit(tx(U(1), 0, [addAsset], "main", "Someone Real"));
    const [c] = rows(await commits(j), 1);
    expect(c.ctx?.author).toMatch(KEYED);
    expect(JSON.stringify(await entries(j))).not.toContain("Someone Real");
  });

  test("non_ulid_txid_is_hashed", async () => {
    const j = open();
    const e = await create();
    await e.commit(tx("idem-key-1", 0, [addAsset]));
    await e.commit(tx(U(2), 1, [insert(0)]));
    const [keyed, raw] = rows(await commits(j), 2);
    expect(keyed.ctx?.txId).toMatch(KEYED);
    expect(raw.ctx?.txId).toBe(U(2));
    expect(JSON.stringify(await entries(j))).not.toContain("idem-key-1");
  });
});

describe("log.commit from history calls", () => {
  test("history_dry_runs_are_dry_run", async () => {
    const j = open();
    const e = await project();
    await e.branchCreate(req({ txId: U(9), name: "side", from: "main" }));
    await e.undo(req({ txId: U(10), branch: "main", dryRun: true }));
    await e.branchCreate(req({ txId: U(11), name: "feature", from: "main", dryRun: true }));
    await e.checkpoint(req({ txId: U(12), branch: "main", name: "v1", dryRun: true }));
    await e.branchSet(req({ txId: U(13), branch: "side", name: "trunk", dryRun: true }));
    const dry = (await commits(j)).slice(3);
    expect(dry.map((c) => [c.payload.record, c.payload.outcome])).toEqual([
      ["undo", "dryRun"],
      ["branch", "dryRun"],
      ["checkpoint", "dryRun"],
      ["branchSet", "dryRun"],
    ]);
    for (const c of dry) expect(c.payload.intent).toBeUndefined();
  });

  test("pick_and_revert_no_op_is_no_op", async () => {
    const j = open();
    const e = await project();
    await e.branchCreate(req({ txId: U(10), name: "feature", from: "main" }));
    await e.commit(tx(U(11), 2, [insert(10)], "br1"));
    await e.commit(tx(U(16), 3, [insert(20)], "br1"));
    await e.cherryPick(req({ txId: U(12), origin: U(11), into: "main" }));
    await e.cherryPick(req({ txId: U(13), origin: U(11), into: "main" }));
    await e.merge(req({ txId: U(20), from: "feature", into: "main" }));
    await e.revert(req({ txId: U(14), mergeId: U(20), branch: "main" }));
    await e.revert(req({ txId: U(15), mergeId: U(20), branch: "main" }));
    const all = await commits(j);
    expect(at(byTx(all, U(12)), 0).payload.outcome).toBe("accepted");
    expect(at(byTx(all, U(13)), 0).payload).toMatchObject({ record: "pick", outcome: "noOp", code: "ALREADY_PRESENT" });
    expect(at(byTx(all, U(15)), 0).payload).toMatchObject({ record: "revert", outcome: "noOp", code: "ALREADY_REVERTED" });
  });

  test("empty_merge_is_no_op", async () => {
    const j = open();
    const e = await project();
    await e.branchCreate(req({ txId: U(10), name: "feature", from: "main" }));
    await e.merge(req({ txId: U(11), from: "feature", into: "main" }));
    const merge = (await commits(j)).at(-1)!;
    expect(merge.payload).toMatchObject({ record: "merge", outcome: "noOp" });
    expect(merge.payload.code).toBeUndefined();
    expect(merge.ctx?.txId).toBe(j.idRefForTest(`${U(11)}-1`));
  });

  test("deduped_retry_on_every_write_path", async () => {
    const j = open();
    const e = await project();
    const twice = async (f: () => Promise<string>) => {
      const first = await f();
      expect(await f()).toBe(first);
    };
    await twice(() => e.commit(tx("c-retry", 2, [insert(30)])));
    await twice(() => e.branchCreate(req({ txId: "b1", name: "feature", from: "main" })));
    await twice(() => e.branchCreate(req({ txId: "b2", name: "other", from: "main" })));
    await e.commit(tx("t-b1", 3, [insert(10)], "br1"));
    await twice(() => e.branchSet(req({ txId: "bs1", branch: "br1", name: "feature-2" })));
    await twice(() => e.checkpoint(req({ txId: "cp1", branch: "main", name: "v1" })));
    await twice(() => e.merge(req({ txId: "m1", from: "br1", into: "main" })));
    await twice(() => e.undo(req({ txId: "u1", branch: "main" })));
    await twice(() => e.redo(req({ txId: "r1", branch: "main" })));
    await twice(() => e.revert(req({ txId: "v1", mergeId: "m1", branch: "main" })));
    await twice(() => e.cherryPick(req({ txId: "p1", origin: "t-b1", into: "br2" })));
    const all = (await commits(j)).slice(2).filter((c) => c.payload.record !== "edit" || c.payload.ops?.["clip.insert"] !== undefined);
    const pairs = new Map<string, string[]>();
    for (const c of all) pairs.set(c.payload.record, [...(pairs.get(c.payload.record) ?? []), c.payload.outcome]);
    expect(Object.fromEntries(pairs)).toEqual({
      edit: ["accepted", "deduped", "accepted"],
      branch: ["accepted", "deduped", "accepted", "deduped"],
      branchSet: ["accepted", "deduped"],
      checkpoint: ["accepted", "deduped"],
      merge: ["accepted", "deduped"],
      undo: ["accepted", "deduped"],
      redo: ["accepted", "deduped"],
      revert: ["accepted", "deduped"],
      pick: ["accepted", "deduped"],
    });
    // A deduped merge names the records it answers with, as the first did.
    const merges = all.filter((c) => c.payload.record === "merge");
    expect(at(merges, 1).ctx?.txId).toBe(at(merges, 0).ctx?.txId);
  });

  test("undo_links_the_undone_tx", async () => {
    const j = open();
    const e = await project();
    await e.undo(req({ txId: U(3), branch: "main" }));
    const undo = (await commits(j)).at(-1)!;
    expect(undo.payload).toMatchObject({ record: "undo", outcome: "accepted", links: [U(2)], ops: { "item.remove": 1 } });
    expect(undo.ctx?.txId).toBe(U(3));
  });

  test("redo_and_revert_link", async () => {
    const j = open();
    const e = await project();
    await e.undo(req({ txId: U(3), branch: "main" }));
    await e.redo(req({ txId: U(4), branch: "main" }));
    await e.branchCreate(req({ txId: U(5), name: "feature", from: "main" }));
    await e.commit(tx(U(6), 4, [insert(10)], "br1"));
    await e.merge(req({ txId: U(7), from: "feature", into: "main" }));
    await e.revert(req({ txId: U(8), mergeId: U(7), branch: "main" }));
    const all = await commits(j);
    expect(at(byTx(all, U(4)), 0).payload).toMatchObject({ record: "redo", links: [U(3)] });
    expect(at(byTx(all, U(8)), 0).payload).toMatchObject({ record: "revert", outcome: "accepted", links: [U(7)] });
  });

  test("merge_links_copied_txs", async () => {
    const j = open();
    const e = await project();
    await e.branchCreate(req({ txId: U(5), name: "feature", from: "main" }));
    await e.commit(tx(U(6), 2, [insert(10)], "br1"));
    await e.commit(tx(U(7), 3, [insert(20)], "br1"));
    await e.merge(req({ txId: U(8), from: "feature", into: "main" }));
    const merge = (await commits(j)).at(-1)!;
    expect(merge.payload).toMatchObject({ record: "merge", outcome: "accepted", links: [U(6), U(7)], ops: { "item.insert": 2 } });
    // The group's records are `<mergeId>-<n>`: not ULIDs, so keyed.
    expect(merge.ctx?.txId).toMatch(KEYED);
    expect(merge.ctx?.txId).toBe(j.idRefForTest(`${U(8)}-1`));
    expect(merge.ctx?.branch).toBe("main");
  });

  test("pick_links_source", async () => {
    const j = open();
    const e = await project();
    await e.branchCreate(req({ txId: U(5), name: "feature", from: "main" }));
    await e.commit(tx(U(6), 2, [insert(10)], "br1"));
    await e.cherryPick(req({ txId: U(7), origin: U(6), into: "main" }));
    const pick = (await commits(j)).at(-1)!;
    expect(pick.payload).toMatchObject({ record: "pick", outcome: "accepted", links: [U(6)] });
  });

  test("merge_and_recover_name_their_first_record_on_every_path", async () => {
    const j = open();
    const e = await project();
    const first = (id: string) => j.idRefForTest(`${id}-1`);
    expect(j.idRefForTest(U(1))).toBe(U(1));
    await e.branchCreate(req({ txId: U(5), name: "feature", from: "main" }));
    await e.commit(tx(U(6), 2, [{ op: "clip.delete", clip: "c1" }], "br1"));
    await e.commit(tx(U(7), 3, [{ op: "clip.delete", clip: "c1" }]));
    // Refused (CONFLICT), then planned, then nothing merged (skipped), then a merge that copies.
    expect((await rejection(e.merge(req({ txId: "m-a", from: "feature", into: "main" })))).code).toBe("CONFLICT");
    await e.merge(req({ txId: "m-a", from: "feature", into: "main", skipConflicting: true, dryRun: true }));
    await e.merge(req({ txId: "m-a", from: "feature", into: "main", skipConflicting: true }));
    await e.commit(tx(U(8), 3, [insert(30)], "br1"));
    await e.merge(req({ txId: "m-b", from: "feature", into: "main", skipConflicting: true }));
    await e.merge(req({ txId: "m-b", from: "feature", into: "main", skipConflicting: true }));
    await e.recover(req({ txId: "r-a", create: "all" }));
    await rejection(e.recover(req({ txId: "r-b", create: "nope" })));
    const merges = (await commits(j)).filter((c) => c.payload.record === "merge");
    expect(merges.map((c) => c.payload.outcome)).toEqual(["conflict", "dryRun", "noOp", "accepted", "deduped"]);
    expect(merges.slice(0, 3).map((c) => c.ctx?.txId)).toEqual(Array(3).fill(first("m-a")));
    expect(merges.slice(3).map((c) => c.ctx?.txId)).toEqual(Array(2).fill(first("m-b")));
    const recovers = (await commits(j)).filter((c) => c.payload.record === "recover");
    expect(recovers.map((c) => [c.payload.outcome, c.ctx?.txId])).toEqual([
      ["noOp", first("r-a")],
      ["refused", first("r-b")],
    ]);
  });

  test("checkpoint_and_branch_records", async () => {
    const j = open();
    const e = await project();
    await e.checkpoint(req({ txId: U(3), branch: "main", name: "v1" }));
    await e.branchCreate(req({ txId: U(4), name: "feature", from: "main" }));
    await e.branchSet(req({ txId: U(5), branch: "feature", archived: true }));
    expect((await rejection(e.checkpoint(req({ txId: U(6), branch: "main", name: "V1" })))).code).toBe("NAME_TAKEN");
    const [cp, branch, set, taken] = rows((await commits(j)).slice(2), 4);
    expect(cp.payload).toMatchObject({ record: "checkpoint", outcome: "accepted" });
    expect(cp.payload.ops).toBeUndefined();
    expect(branch.payload).toMatchObject({ record: "branch", outcome: "accepted" });
    expect(branch.ctx?.branch).toBe("main");
    expect(set.payload).toMatchObject({ record: "branchSet", outcome: "accepted" });
    expect(taken.payload).toMatchObject({ record: "checkpoint", outcome: "refused", code: "NAME_TAKEN" });
    // `branch.list` and `resolveBranch` are not writes.
    await e.branches(true);
    await e.resolveBranch("main");
    expect(await commits(j)).toHaveLength(6);
  });

  test("recover_create_is_logged", async () => {
    const j = open();
    const e = await project();
    await e.recover(req({ txId: U(3), create: "all" }));
    await e.recover(req({ txId: U(4), create: "all", dryRun: true }));
    expect((await rejection(e.recover(req({ txId: U(5), create: "nope" })))).code).toBeTruthy();
    const [made, dry, refused] = rows((await commits(j)).slice(2), 3);
    // Nothing was set aside on this server, so nothing is made.
    expect(made.payload).toMatchObject({ record: "recover", outcome: "noOp" });
    expect(made.ctx?.txId).toBe(j.idRefForTest(`${U(3)}-1`));
    expect(dry.payload).toMatchObject({ record: "recover", outcome: "dryRun" });
    expect(refused.payload).toMatchObject({ record: "recover", outcome: "refused" });
  });

  test("recover_listing_is_not_logged", async () => {
    const j = open();
    const e = await project();
    await e.recover(req({}));
    await e.recover(req({ dryRun: true }));
    expect(await commits(j)).toHaveLength(2);
  });
});

describe("intents (training)", () => {
  const training = { training: "on" };

  test("intent_is_content_and_committed", async () => {
    const j = open(training);
    const e = await project();
    await e.commit(tx(U(3), 1, [insert(20, 1)], "main", "Someone Real"));
    const c = (await commits(j)).at(-1)!;
    expect(c.consent.purposes).toEqual(["diagnostics", "training"]);
    expect(c.payload.outcome).toBe("rebased");
    const [intent] = c.payload.intent;
    // As committed: rebased onto rev 2, the alias normalised, the author keyed.
    expect(intent).toMatchObject({ txId: U(3), baseRev: 2, branch: "main", author: expect.stringMatching(KEYED) });
    expect(intent.ops.map((o: { op: string }) => o.op)).toEqual(["item.insert"]);
    expect(c.payload.baseRev).toBe(1);
    // The asset path is a keyed path token, never the path.
    const first = at(await commits(j), 0);
    expect(first.payload.intent[0].ops[0].path).toMatch(/^<other>\/k1:[0-9a-f]{32}\.mov$/);
    expect(JSON.stringify(await entries(j))).not.toContain("/media/a.mov");
    // A history call's intent is its records.
    await e.undo(req({ txId: U(4), branch: "main" }));
    const undo = (await commits(j)).at(-1)!;
    expect(undo.payload.intent.map((t: { txId: string }) => t.txId)).toEqual([U(4)]);
    // A deduped retry and a dry run carry none (the first attempt's entry has it).
    await e.commit(tx(U(3), 1, [insert(20, 1)], "main", "Someone Real"));
    await e.undo(req({ txId: U(4), branch: "main" }));
    await e.redo(req({ txId: U(5), branch: "main", dryRun: true }));
    const [edit, again, dry] = rows((await commits(j)).slice(-3), 3);
    expect([edit.payload.outcome, again.payload.outcome, dry.payload.outcome]).toEqual(["deduped", "deduped", "dryRun"]);
    for (const c of [edit, again, dry]) {
      expect(c.payload.intent).toBeUndefined();
      expect(c.payload.intentOmitted).toBeUndefined();
    }
  });

  test("failed_commit_intent_is_the_request", async () => {
    const j = open(training);
    const e = await project();
    await e.commit(tx(U(3), 2, [{ op: "clip.delete", clip: "c1" }]));
    await rejection(e.commit(tx(U(4), 2, [{ op: "clip.delete", clip: "c1" }])));
    const c = (await commits(j)).at(-1)!;
    expect(c.payload.outcome).toBe("conflict");
    expect(c.payload.intent).toEqual([expect.objectContaining({ txId: U(4), baseRev: 2, ops: [{ op: "item.delete", item: "c1", mode: "ripple" }] })]);
  });

  test("oversized_intent_is_omitted", async () => {
    const j = open(training);
    const e = await create();
    const ops = Array.from({ length: 300 }, (_, i) => ({ ...addAsset, id: `$a${i}`, fingerprint: `fingerprint-${i}`.padEnd(40, "x") }));
    await e.commit(tx(U(1), 0, ops));
    const [c] = rows(await commits(j), 1);
    expect(c.payload.outcome).toBe("accepted");
    expect(c.payload.intent).toBeUndefined();
    expect(c.payload.intentOmitted).toBe("tooLarge");
    expect(c.payload.ops).toEqual({ "asset.add": 300 });
  });

  test("diagnostics alone keeps no intent", async () => {
    const j = open();
    await project();
    for (const c of await commits(j)) {
      expect(c.payload.intent).toBeUndefined();
      expect(c.payload.intentOmitted).toBeUndefined();
    }
  });
});

describe("other sources", () => {
  test("internal_error_is_error_internal", async () => {
    const j = open();
    const e = await create();
    // An expected refusal is not internal.
    expect((await rejection(e.commit(tx(U(1), 9, [addAsset])))).code).toBe("BASE_REV_AHEAD");
    expect((await entries(j)).filter((x) => x.kind === "error.internal")).toEqual([]);
    expect((await rejection(e.panicForTest())).code).toBe("INTERNAL");
    const internal = (await entries(j)).filter((x) => x.kind === "error.internal");
    expect(internal).toHaveLength(1);
    // The call's name and the error's code and detail keys, never their values.
    expect(at(internal, 0).payload).toEqual({ call: "other", code: "INTERNAL", details: ["other"] });
    expect(at(internal, 0).ctx?.project).toBe(e.projectId);
  });

  test("read_only_transition_is_storage_state", async () => {
    const j = open();
    const e = await project();
    const states = async () =>
      (await entries(j, { kinds: ["storage.state"] })).map((x) => (x.payload as { state: string; reason: string }));
    // The records reach the folder (every 2 s), then a sync conflict copy of a segment appears.
    const segs = join(projectDir, ".finisher", "segments");
    const first = await waitFor(() => {
      const dirs = existsSync(segs) ? readdirSync(segs).map((d) => join(segs, d)).filter((d) => statSync(d).isDirectory()) : [];
      return dirs.flatMap((d) => readdirSync(d).filter((f) => f.endsWith(".fseg")).map((f) => join(d, f)))[0];
    }, 10_000, "an exported segment");
    const copy = first.replace(/\.fseg$/, " (conflicted copy 2026-11-07).fseg");
    copyFileSync(first, copy);
    const ro = await waitFor(async () => (await states()).find((s) => s.state === "readOnly"), 15_000, "readOnly");
    expect(ro).toEqual({ state: "readOnly", reason: "CONFLICT_COPY" });
    expect(JSON.parse(await e.status()).readOnly.reason).toBe("CONFLICT_COPY");
    rmSync(copy);
    const back = await waitFor(async () => (await states()).find((s) => s.state === "writable"), 15_000, "writable");
    expect(back).toEqual({ state: "writable", reason: "CONFLICT_COPY" });
    expect(await states()).toHaveLength(2);
  }, 40_000);

  test("eval_stats_keys", async () => {
    const j = open(null, serverData, { statsEveryMs: 200 });
    const e = await project();
    await e.evalFrame("main", null, "1");
    await e.evalAudio("main", null, "0", "1");
    await e.fontsPlan("main", "[]");
    await rejection(e.evalFrame("main", null, "not a time"));
    const keys = await waitFor(async () => {
      const stats = (await entries(j)).filter((x) => x.kind === "eval.stats");
      const found = new Map(stats.map((s) => [(s.payload as { key: string }).key, s.payload as { histogram: { count: number; errors?: object } }]));
      return found.size >= 3 ? found : undefined;
    }, 5_000, "eval.stats");
    expect([...keys.keys()].sort()).toEqual(["eval.audio", "eval.frame", "fonts.plan"]);
    const frame = [...(await entries(j))].filter((x) => x.kind === "eval.stats" && (x.payload as { key: string }).key === "eval.frame");
    const counted = frame.reduce((n, x) => n + (x.payload as { histogram: { count: number } }).histogram.count, 0);
    expect(counted).toBe(2);
    expect(frame.some((x) => JSON.stringify(x.payload).includes("INVALID_TIME"))).toBe(true);
  });

  test("commit_with_journal_off_touches_no_events_dir", async () => {
    open({ diagnostics: "off" });
    const e = await project();
    await e.undo(req({ txId: U(3), branch: "main" }));
    await e.checkpoint(req({ txId: U(4), branch: "main", name: "v1" }));
    await e.evalFrame("main", null, "0");
    await e.recover(req({ txId: U(5), create: "all" }));
    await rejection(e.commit(tx(U(6), 99, [addAsset])));
    await rejection(e.panicForTest());
    expect(existsSync(join(serverData, "events"))).toBe(false);
  });

  test("follower_honours_never_from_synced_file", async () => {
    const followerData = join(tmp, "server-data-2");
    const j = open(null, followerData);
    const leader = await project();
    const follower = await Engine.open(followerData, projectDir);
    engines.push(follower);
    const id = follower.projectId;
    await waitFor(async () => ((await follower.head("main")) === 2 ? true : undefined), 10_000, "the follower to catch up");
    await follower.undo(req({ txId: U(3), branch: "main", dryRun: true }));
    expect((await commits(j)).map((c) => [c.ctx?.project, c.payload.outcome])).toEqual([[id, "dryRun"]]);
    expect(JSON.parse(j.projectPolicies())[id]).toBe("default");
    // The leader writes the flag; the follower re-reads the synced file at its next poll.
    await leader.setCollect("never");
    await waitFor(() => (JSON.parse(j.projectPolicies())[id] === "never" ? true : undefined), 10_000, "never on the follower");
    expect(await commits(j)).toEqual([]);
    await follower.undo(req({ txId: U(4), branch: "main", dryRun: true }));
    expect((await entries(j)).filter((x) => x.ctx?.project === id)).toEqual([]);
  }, 20_000);

  test("journal_stall_does_not_slow_commits", async () => {
    const j = open();
    const e = await project();
    let rev = await e.head("main");
    const time = async (n: number): Promise<number> => {
      const ms: number[] = [];
      for (let i = 0; i < n; i++) {
        const t = performance.now();
        await e.commit(tx(`stall-${rev}`, rev, [{ op: "clip.trim", clip: "c1", edge: "end", delta: "1/30" }]));
        ms.push(performance.now() - t);
        rev++;
      }
      ms.sort((a, b) => a - b);
      return at(ms, Math.floor(n / 2));
    };
    await time(20);
    const running = await time(100);
    expect(j.pauseWriterForTest()).toBe(true);
    try {
      const stalled = await time(100);
      expect(stalled).toBeLessThan(running * 3 + 2);
    } finally {
      j.resumeWriterForTest();
    }
    expect((await commits(j)).length).toBeGreaterThanOrEqual(220);
  }, 30_000);
});

describe("fonts.fetch", () => {
  const hooks = testHooks!;
  let server: FontServer;
  const ENV = ["FIN_FONT_CATALOG", "FIN_FONTS_TEST_BACKOFF_MS", "FIN_FONTS_TEST_NO_PROXY"];

  beforeEach(async () => {
    server = await FontServer.start();
    process.env.FIN_FONTS_TEST_BACKOFF_MS = "0";
    process.env.FIN_FONTS_TEST_NO_PROXY = "1";
  });

  afterEach(async () => {
    for (const e of engines) await e.close().catch(() => undefined);
    engines = [];
    await server.close();
    for (const k of ENV) delete process.env[k];
  });

  test("font_fetch_events", async () => {
    const bytes = hooks.testFont("sans");
    const good: CatalogFile = { id: "sans", bytes, url: server.serve("sans.ttf", bytes) };
    const gone: CatalogFile = { id: "gone", bytes: hooks.testFont("latinBold"), url: server.serve("gone.ttf", bytes, { status: 404 }) };
    const catalog = join(tmp, "catalog.json");
    writeFileSync(catalog, catalogJson([good, gone]));
    process.env.FIN_FONT_CATALOG = catalog;
    const j = open();
    const e = await create();
    const pin = catalogPin(good);
    const { pins } = JSON.parse(pin.fonts) as { pins: Record<string, unknown> };
    const ops = Object.entries(pins).map(([key, p]) => ({ op: "font.pin", key, pin: p }));
    await e.commit(tx(U(1), 0, ops));
    const fetches = async () => (await entries(j)).filter((x) => x.kind === "fonts.fetch").map((x) => x.payload as Record<string, unknown>);
    const ok = await waitFor(async () => (await fetches()).find((f) => f.outcome === "ok"), 10_000, "a fetch");
    expect(ok).toMatchObject({ entry: expect.stringMatching(KEYED), bundled: false, bytes: bytes.length, outcome: "ok" });
    expect(typeof ok.micros).toBe("number");
    const fonts = FontManager.open(serverData);
    await fonts.fetch(JSON.stringify({ entries: ["sans"] }));
    await rejection(fonts.fetch(JSON.stringify({ entries: ["gone"] })));
    const all = await fetches();
    expect(all.find((f) => f.outcome === "cached")).toMatchObject({ entry: ok.entry, bytes: 0 });
    expect(all.find((f) => f.outcome === "failed")).toMatchObject({ code: "FONT_FETCH_FAILED", bytes: 0 });
    // The family name itself is never stored.
    expect(JSON.stringify(await entries(j))).not.toContain("Test Sans");
  }, 20_000);
});
