// Captures real NDJSON frames from a running finisherd for the Rust `fin` client (Task 12).
// Usage: pnpm --filter finisherd capture-rpc-fixtures
// Writes crates/fin/tests/fixtures/rpc/<scenario>.{request,response}.ndjson: every frame the
// client wrote, and every frame the server wrote, one per line in wire order. Frames are
// byte-for-byte the server's output except for two normalizations that keep the fixtures
// stable across runs: the temp directory becomes `/fixture` and the project id becomes
// `01fixtureproject0000000000` (same 26-char shape).
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { addAsset, insert, NdjsonClient, ok, type RunningServer, startServer, tx } from "../test/support/ndjson-client.ts";

const OUT = fileURLToPath(new URL("../../../crates/fin/tests/fixtures/rpc/", import.meta.url));
const FIXTURE_PROJECT_ID = "01fixtureproject0000000000";

const tmp = mkdtempSync(join(tmpdir(), "finisherd-fixtures-"));
let server: RunningServer | undefined;
let projectId = "";

function normalize(line: string): string {
  let out = line.split(JSON.stringify(tmp).slice(1, -1)).join("/fixture");
  if (projectId) out = out.split(projectId).join(FIXTURE_PROJECT_ID);
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

async function scenario(name: string, run: (c: NdjsonClient) => Promise<void>): Promise<void> {
  const c = await NdjsonClient.connect(server!.info.socket);
  await run(c);
  await c.close();
  write(name, c);
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

  await scenario("tx-commit", async (c) => {
    ok(await c.call("tx.commit", { projectId, tx: tx("tx-1", 0, [addAsset()]) }));
  });

  const setup = await NdjsonClient.connect(server.info.socket);
  ok(await setup.call("tx.commit", { projectId, tx: tx("tx-2", 1, [insert(0, 5)]) }));
  ok(await setup.call("tx.commit", { projectId, tx: tx("tx-3", 2, [{ op: "clip.delete", clip: "c1" }]) }));
  await setup.close();

  await scenario("conflict", async (c) => {
    const exit = await c.call("tx.commit", { projectId, tx: tx("tx-late", 2, [{ op: "clip.delete", clip: "c1" }]) });
    if (exit.ok || exit.error.code !== "CONFLICT") throw new Error(`expected CONFLICT, got ${JSON.stringify(exit)}`);
  });

  // Replays revs 2..3 from the log, then one live commit (rev 4), then the client interrupts.
  await scenario("ops-subscribe", async (c) => {
    const seen: number[] = [];
    let live!: () => void;
    const gotLive = new Promise<void>((r) => (live = r));
    const sub = c.stream("ops.subscribe", { projectId, branch: "main", fromRev: 1 }, (v) => {
      seen.push((v as { rev: number }).rev);
      if (seen.includes(4)) live();
    });
    while (seen.length < 2) await new Promise((r) => setTimeout(r, 10));
    const writer = await NdjsonClient.connect(server!.info.socket);
    ok(await writer.call("tx.commit", { projectId, tx: tx("tx-4", 3, [addAsset("$b")]) }));
    await writer.close();
    await gotLive;
    // Let the trailing Ack for the live chunk reach the server before interrupting.
    await new Promise((r) => setTimeout(r, 50));
    c.interrupt(sub.id);
    await sub.done;
  });

  await scenario("subscribe-error", async (c) => {
    const sub = c.stream("ops.subscribe", { projectId: "nope", branch: "main", fromRev: 0 }, () => undefined);
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

  // Already open here: relocate changes nothing, the answer is the usual ref.
  await scenario("project-open-relocate", async (c) => {
    ok(await c.call("project.open", { dir, relocate: true }));
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

  // Last: it changes the database's incarnation.
  await unary("project-rebuild", "project.rebuild", {});
} finally {
  const code = await server?.stop();
  rmSync(tmp, { recursive: true, force: true });
  if (code !== undefined && code !== 0) process.exitCode = 1;
}
