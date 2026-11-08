// The WebSocket listener's work before authentication (review of 5A Task 6, finding 8): slow
// headers are cut off, open connections are capped, and `settings.json`'s origins are read
// again only when the file changes.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect, Scope } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { cachedOrigins, settingsPath } from "../src/config.ts";
import { makeTicketedWebSocketServer } from "../src/upgrade.ts";

let tmp: string;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-upgrade-"));
});

afterEach(() => rmSync(tmp, { recursive: true, force: true }));

/** Resolves when the server closes `sock`, with the time it took (ms). */
function closedAfter(port: number, send: string): Promise<{ ms: number; data: string }> {
  return new Promise((resolve, reject) => {
    const t0 = Date.now();
    let data = "";
    const sock = connect({ host: "127.0.0.1", port }, () => sock.write(send));
    sock.setEncoding("utf8");
    sock.on("data", (d: string) => (data += d));
    sock.on("error", () => undefined);
    sock.on("close", () => resolve({ ms: Date.now() - t0, data }));
    setTimeout(() => {
      sock.destroy();
      reject(new Error("the server never closed the connection"));
    }, 8_000);
  });
}

describe("before the upgrade", () => {
  test("slow headers are cut off; open connections are capped", { timeout: 20_000 }, async () => {
    const scope = Effect.runSync(Scope.make());
    try {
      const { port } = await Effect.runPromise(
        Scope.provide(
          makeTicketedWebSocketServer({
            check: () => ({ ok: false, status: 401, reason: "badTicket" }),
            maxPayload: 1024,
            headersTimeoutMs: 500,
            maxConnections: 2,
          }),
          scope,
        ),
      );
      // Half a request, never finished: closed after the header timeout (checked each second).
      const slow = await closedAfter(port, "GET /rpc HTTP/1.1\r\nHost: 127.0.0.1\r\n");
      expect(slow.ms).toBeLessThan(4_000);
      // Two idle connections fill the listener; a third is closed at once.
      const idle = [0, 1].map(
        () =>
          new Promise<ReturnType<typeof connect>>((r) => {
            const s = connect({ host: "127.0.0.1", port }, () => r(s));
            s.on("error", () => undefined);
          }),
      );
      const held = await Promise.all(idle);
      await new Promise((r) => setTimeout(r, 100));
      const third = await closedAfter(port, "GET /rpc HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
      expect(third.data).toBe("");
      expect(third.ms).toBeLessThan(400);
      for (const s of held) s.destroy();
    } finally {
      await Effect.runPromise(Scope.close(scope, Effect.void as never));
    }
  });

  test("origins are read again only when settings.json changes", () => {
    const sd = join(tmp, "sd");
    mkdirSync(sd);
    let reads = 0;
    const origins = cachedOrigins(sd, () => (reads++, [`https://app${reads}.example`]));
    expect(origins()).toEqual([]);
    expect(reads).toBe(0);
    writeFileSync(settingsPath(sd), JSON.stringify({ server: { origins: ["https://a.example"] } }));
    expect(origins()).toEqual(["https://app1.example"]);
    for (let i = 0; i < 100; i++) origins();
    expect(reads).toBe(1);
    // A change (a new file, as the settings writer renames one in) is read once.
    rmSync(settingsPath(sd));
    writeFileSync(settingsPath(sd), JSON.stringify({ server: { origins: ["https://b.example", "https://c.example"] } }));
    expect(origins()).toEqual(["https://app2.example"]);
    expect(reads).toBe(2);
  });
});
