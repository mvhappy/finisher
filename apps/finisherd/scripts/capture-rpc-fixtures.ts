// Captures real NDJSON frames from a running finisherd for the Rust `fin` client (Task 12).
// Usage: pnpm --filter finisherd capture-rpc-fixtures
// Writes crates/fin/tests/fixtures/rpc/<scenario>.{request,response}.ndjson: every frame the
// client wrote, and every frame the server wrote, one per line in wire order. Frames are
// byte-for-byte the server's output except for two normalizations that keep the fixtures
// stable across runs: the temp directory becomes `/fixture`, the project id
// `01fixtureproject0000000000` and the database's incarnation `01fixtureincarnation000000`
// (same 26-char shape). Since protocol 2 every write and subscription carries a generation.
// The console's confirmed scenarios (5A Decision 30) record only the confirmed call, after a
// `console.challenge` that is left out; its code becomes `0FIXTURECHALLENGE0`, which
// `rpc_contract.rs` replaces with a code of its own connection.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  addAsset,
  connectProved,
  generationOf,
  insert,
  NdjsonClient,
  ok,
  type RunningServer,
  startServer,
  tx,
} from "../test/support/ndjson-client.ts";

const OUT = fileURLToPath(new URL("../../../crates/fin/tests/fixtures/rpc/", import.meta.url));
const FIXTURE_PROJECT_ID = "01fixtureproject0000000000";
const FIXTURE_INCARNATION = "01fixtureincarnation000000";
const FIXTURE_CHALLENGE = "0FIXTURECHALLENGE0";

const tmp = mkdtempSync(join(tmpdir(), "finisherd-fixtures-"));
let server: RunningServer | undefined;
let projectId = "";
let incarnation = "";
let challenge = "";

function normalize(line: string): string {
  let out = line.split(JSON.stringify(tmp).slice(1, -1)).join("/fixture");
  if (projectId) out = out.split(projectId).join(FIXTURE_PROJECT_ID);
  if (incarnation) out = out.split(incarnation).join(FIXTURE_INCARNATION);
  if (challenge) out = out.split(`"challenge":"${challenge}"`).join(`"challenge":"${FIXTURE_CHALLENGE}"`);
  return out;
}

function write(name: string, client: NdjsonClient): void {
  const pick = (dir: ">>" | "<<") =>
    client.transcript
      .filter((t) => t.dir === dir)
      .map((t) => `${normalize(t.line)}\n`)
      .join("");
  writeFileSync(join(OUT, `${name}.request.ndjson`), pick(">>"));
  writeFileSync(join(OUT, `${name}.response.ndjson`), pick("<<"));
  process.stderr.write(`wrote ${name}.{request,response}.ndjson\n`);
}

async function scenario(name: string, run: (c: NdjsonClient) => Promise<void>, credential: "local" | "console" = "local"): Promise<void> {
  const c = await connectProved(server!, credential);
  await run(c);
  await c.close();
  write(name, c);
}

/**
 * A console scenario of one mutating human-only call: the challenge is asked for first and left
 * out of the transcript; `send(code)` makes the recorded call.
 */
async function confirmedScenario(name: string, send: (c: NdjsonClient, code: string) => Promise<void>): Promise<void> {
  const c = await connectProved(server!, "console");
  challenge = ok<{ code: string }>(await c.call("console.challenge", {})).code;
  c.transcript.length = 0;
  c.restartIds();
  await send(c, challenge);
  await c.close();
  write(name, c);
  challenge = "";
}

try {
  mkdirSync(OUT, { recursive: true });
  server = await startServer(join(tmp, "server-data"));
  const dir = join(tmp, "My Project");

  await scenario("hello", async (c) => {
    ok(await c.call("server.hello", {}));
  });

  await scenario("ping", async (c) => {
    c.send({ _tag: "Ping" });
    await c.nextFrame();
  });

  await scenario("project-create", async (c) => {
    projectId = ok<{ projectId: string }>(
      await c.call("project.create", { dir, name: "Demo", rate: "30000/1001", idemKey: "create-1" }),
    ).projectId;
  });

  // The same create again (its answer lost): the project it made, not PROJECT_EXISTS.
  await scenario("project-create-retry", async (c) => {
    ok(await c.call("project.create", { dir, name: "Demo", rate: "30000/1001", idemKey: "create-1" }));
  });

  // The generation a client reads before it writes (`snapshot.get`); not part of the scenarios.
  const reader = await connectProved(server);
  const generation = await generationOf(reader, projectId);
  incarnation = generation.incarnation;
  await reader.close();

  await scenario("tx-commit", async (c) => {
    ok(await c.call("tx.commit", { projectId, tx: tx("tx-1", 0, [addAsset()]), generation }));
  });

  const setup = await connectProved(server);
  ok(await setup.call("tx.commit", { projectId, tx: tx("tx-2", 1, [insert(0, 5)]), generation }));
  ok(await setup.call("tx.commit", { projectId, tx: tx("tx-3", 2, [{ op: "clip.delete", clip: "c1" }]), generation }));
  await setup.close();

  await scenario("conflict", async (c) => {
    const exit = await c.call("tx.commit", { projectId, tx: tx("tx-late", 2, [{ op: "clip.delete", clip: "c1" }]), generation });
    if (exit.ok || exit.error.code !== "CONFLICT") throw new Error(`expected CONFLICT, got ${JSON.stringify(exit)}`);
  });

  // Replays revs 2..3 from the log, then one live commit (rev 4), then the client interrupts.
  await scenario("ops-subscribe", async (c) => {
    const seen: number[] = [];
    let live!: () => void;
    const gotLive = new Promise<void>((r) => (live = r));
    const sub = c.stream("ops.subscribe", { projectId, branch: "main", fromRev: 1, generation }, (v) => {
      seen.push((v as { rev: number }).rev);
      if (seen.includes(4)) live();
    });
    while (seen.length < 2) await new Promise((r) => setTimeout(r, 10));
    const writer = await connectProved(server!);
    ok(await writer.call("tx.commit", { projectId, tx: tx("tx-4", 3, [addAsset("$b")]), generation }));
    await writer.close();
    await gotLive;
    // Let the trailing Ack for the live chunk reach the server before interrupting.
    await new Promise((r) => setTimeout(r, 50));
    c.interrupt(sub.id);
    await sub.done;
  });

  await scenario("subscribe-error", async (c) => {
    const sub = c.stream("ops.subscribe", { projectId: "nope", branch: "main", fromRev: 0, generation }, () => undefined);
    await sub.done;
  });

  // Storage RPCs (plan 4A Task 15). Unary ones: one request, one answer.
  const unary = (name: string, tag: string, payload: Record<string, unknown>, code?: string) =>
    scenario(name, async (c) => {
      const exit = await c.call(tag, { projectId, ...payload });
      if (code === undefined) ok(exit);
      else if (exit.ok || exit.error.code !== code) throw new Error(`${name}: expected ${code}, got ${JSON.stringify(exit)}`);
    });
  await unary("snapshot-get", "snapshot.get", { branch: "main" });
  await unary("project-status", "project.status", {});
  await unary("project-takeover", "project.takeover", { confirm: false });
  await unary("project-verify", "project.verify", {});
  await unary("project-compact", "project.compact", {});
  await unary("project-recover", "project.recover", {});
  await unary("project-acknowledge-rewind", "project.acknowledgeRewind", { branch: "main", generation: 1 });
  await unary("project-acknowledge-salvage", "project.acknowledgeSalvage", { copy: "project.fdb.corrupt-0" });
  await unary("tx-get", "tx.get", { txId: "tx-1" });
  await unary("tx-get-unknown", "tx.get", { txId: "tx-never" });
  // Evaluation and fonts (plan 4C Task 22). The built-in catalog; nothing is pinned, so
  // nothing is fetched (no network).
  await unary("eval-frame", "eval.frame", { at: "1" });
  await unary("eval-audio", "eval.audio", { from: "0", to: "5" });
  await unary("fonts-plan", "fonts.plan", { branch: "main", ops: [addAsset("$p")] });
  // Media (plan 6A Task 13): a look past the end of the (empty) timeline, refused before any job.
  // A rendered look names the GPU it ran on, so it has no fixture.
  await unary("media-look-range", "media.look", { at: ["99"] }, "INVALID_RANGE");
  const server_ = async (name: string, tag: string, payload: Record<string, unknown>, code?: string) =>
    scenario(name, async (c) => {
      const exit = await c.call(tag, payload);
      if (code === undefined) ok(exit);
      else if (exit.ok || exit.error.code !== code) throw new Error(`${name}: expected ${code}, got ${JSON.stringify(exit)}`);
    });
  await server_("fonts-catalog", "fonts.catalog", {});
  await server_("fonts-status", "fonts.status", {});
  await server_("fonts-fetch-unknown", "fonts.fetch", { entries: ["no-such-entry"] }, "FONT_NOT_IN_CATALOG");
  await unary(
    "tx-commit-stale",
    "tx.commit",
    { tx: tx("tx-stale", 4, [addAsset("$s")]), generation: { incarnation: "an-older-database", n: 0 } },
    "HISTORY_REWOUND",
  );

  // A subscriber that read rev 1 at another generation: ended at once, nothing replayed.
  await scenario("ops-subscribe-stale", async (c) => {
    const generation = { incarnation: "an-older-database", n: 0 };
    const sub = c.stream("ops.subscribe", { projectId, branch: "main", fromRev: 1, generation }, () => undefined);
    const exit = await sub.done;
    if (exit.ok || exit.error.code !== "HISTORY_REWOUND") throw new Error(`expected HISTORY_REWOUND, got ${JSON.stringify(exit)}`);
  });

  // Already open here: relocate changes nothing, the answer is the usual ref. A relocation is a
  // person's decision (5A Task 5): the console asks, with its typed code (Task 7).
  await confirmedScenario("project-open-relocate", async (c, code) => {
    ok(await c.call("project.open", { dir, relocate: true, challenge: code }));
  });

  // The status now, then the client interrupts.
  await scenario("project-events", async (c) => {
    let first!: () => void;
    const got = new Promise<void>((r) => (first = r));
    const sub = c.stream("project.events", { projectId }, () => first());
    await got;
    await new Promise((r) => setTimeout(r, 50));
    c.interrupt(sub.id);
    await sub.done;
  });

  // Branches and history (plan 4B Task 18), on their own branches so the scenarios above
  // keep their revs. `fixture` is the author. Main is at rev 4 here. The setup between
  // scenarios (a clip, edits on `br1`, a second branch) is mirrored in `rpc_contract.rs`.
  const rpc = async (tag: string, payload: Record<string, unknown>) => {
    const c = await connectProved(server!);
    const branch = (payload.tx as { branch?: string } | undefined)?.branch;
    const extra = tag === "tx.commit" ? { generation: await generationOf(c, projectId, branch ?? "main") } : {};
    ok(await c.call(tag, { projectId, ...payload, ...extra }));
    await c.close();
  };
  const who = { author: "fixture" };
  const onBranch = (id: string, branch: string, baseRev: number, ops: unknown[]) => ({ tx: { ...tx(id, baseRev, ops), branch } });
  const clip = (at: number) => insert(at, 5);
  await rpc("tx.commit", { tx: tx("fx-m0", 4, [clip(0)]) }); // c2, main rev 5
  await unary("branch-create", "branch.create", { txId: "fx-b1", ...who, name: "feature", from: "main" }); // br1 forks at 5; main rev 6
  await unary("branch-list", "branch.list", { all: true });
  await rpc("tx.commit", onBranch("fx-e1", "br1", 5, [{ op: "clip.delete", clip: "c2" }])); // br1 rev 6
  await rpc("tx.commit", onBranch("fx-e2", "br1", 6, [clip(20)])); // br1 rev 7
  await rpc("tx.commit", { tx: tx("fx-m1", 6, [{ op: "clip.delete", clip: "c2" }]) }); // main rev 7
  await unary("history-merge-conflict", "history.merge", { txId: "fx-mg0", ...who, from: "feature", into: "main" }, "CONFLICT");
  await unary("history-merge", "history.merge", { txId: "fx-mg1", ...who, from: "feature", into: "main", skipConflicting: true }); // main rev 8
  await unary("history-undo", "history.undo", { txId: "fx-u1", ...who, branch: "main" }); // rev 9
  await unary("history-redo", "history.redo", { txId: "fx-r1", ...who, branch: "main" }); // rev 10
  await unary("history-checkpoint", "history.checkpoint", { txId: "fx-c1", ...who, branch: "main", name: "v1" }); // rev 11
  await rpc("branch.create", { txId: "fx-b2", ...who, name: "other", from: "main" }); // br2 forks at 11; main rev 12
  await rpc("tx.commit", onBranch("fx-p1", "br2", 11, [clip(40)])); // br2 rev 12
  await unary("history-cherry-pick", "history.cherryPick", { txId: "fx-pk1", ...who, origin: "fx-p1", into: "main" }); // main rev 13
  await unary("history-revert", "history.revert", { txId: "fx-rv1", ...who, mergeId: "fx-mg1", branch: "main" }); // main rev 14
  await unary("branch-set", "branch.set", { txId: "fx-s1", ...who, branch: "feature", name: "polish" }); // main rev 15
  await unary("project-recover-dismiss-unknown", "project.recover", { txId: "fx-rc2", ...who, dismiss: "000000000000" }, "NOT_FOUND");
  await unary("project-recover-groups", "project.recover", { txId: "fx-rc1", ...who, create: "all", dryRun: true });

  // Diagnostics (plan D, Task D4; 5A Task 7): `local` is the `fin` source, its exports are
  // redacted, and consent changes, `project default` and purges are the console's, confirmed.
  // Mirrored in `rpc_contract.rs` `replay_diagnostics`.
  const cliCommand = {
    name: "item.trim",
    flags: ["project"],
    argsHash: `blake3:${"ab".repeat(32)}`,
    micros: 1000,
    exit: 0,
    online: true,
    dryRun: false,
    txIds: ["fx-m0"],
  };
  await server_("diagnostics-record", "diagnostics.record", { kind: "cli.command", kv: 1, payload: cliCommand });
  await server_("diagnostics-record-unlisted", "diagnostics.record", { kind: "log.commit", kv: 1, payload: {} }, "EVENT_INVALID");
  await server_("diagnostics-consent-training-on", "diagnostics.consent", { purpose: "training", value: "on" }, "CONSENT_UI_ONLY");
  await server_("diagnostics-consent-human-only", "diagnostics.consent", { purpose: "diagnostics", value: "on" }, "HUMAN_ONLY");
  await confirmedScenario("diagnostics-consent", async (c, code) => {
    ok(await c.call("diagnostics.consent", { purpose: "diagnostics", value: "on", challenge: code }));
  });
  await server_("diagnostics-status", "diagnostics.status", {});
  await server_("diagnostics-export", "diagnostics.export", { filter: { kinds: ["cli.command"] }, limit: 10 });
  await confirmedScenario("diagnostics-project", async (c, code) => {
    ok(await c.call("diagnostics.project", { projectId, collect: "default", challenge: code }));
  });
  await confirmedScenario("diagnostics-purge", async (c, code) => {
    ok(await c.call("diagnostics.purge", { selector: { before: 0 }, challenge: code }));
  });

  // Last: it changes the database's incarnation.
  await unary("project-rebuild", "project.rebuild", {});
} finally {
  const code = await server?.stop();
  rmSync(tmp, { recursive: true, force: true });
  if (code !== undefined && code !== 0) process.exitCode = 1;
}
