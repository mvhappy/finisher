// 5A Task 6 against a running finisherd: the WebSocket takes a one-time ticket in
// `Sec-WebSocket-Protocol`, checks Host and Origin first, refuses other protocol versions with
// 426 and carries the ticket's principal; failures are audited without the ticket.
import { request } from "node:http";
import { DatabaseSync } from "node:sqlite";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { connectProved, ok, type RunningServer, startServer } from "./support/ndjson-client.ts";
import { connectWs, mintTicket, openWs } from "./support/ws-client.ts";

const WIN = process.platform === "win32";
let tmp: string;
let server: RunningServer | undefined;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-ws-"));
});

afterEach(async () => {
  if (server) {
    const code = await server.stop();
    if (!WIN) expect(code).toBe(0);
  }
  server = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

async function start(): Promise<RunningServer> {
  server = await startServer(join(tmp, "server-data"));
  return server;
}

/** The raw HTTP answer to an upgrade request: status and body. */
function upgrade(port: number, headers: Record<string, string>, path = "/rpc"): Promise<{ status: number; body: string; protocol?: string }> {
  return new Promise((resolve, reject) => {
    const req = request({
      host: "127.0.0.1",
      port,
      path,
      headers: { Connection: "Upgrade", Upgrade: "websocket", "Sec-WebSocket-Version": "13", "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==", ...headers },
    });
    req.on("response", (res) => {
      let body = "";
      res.setEncoding("utf8").on("data", (d: string) => (body += d));
      res.on("end", () => resolve({ status: res.statusCode ?? 0, body }));
    });
    req.on("upgrade", (res, socket) => {
      socket.destroy();
      const protocol = res.headers["sec-websocket-protocol"];
      resolve({ status: res.statusCode ?? 0, body: "", ...(typeof protocol === "string" ? { protocol } : {}) });
    });
    req.on("error", reject);
    req.end();
  });
}

describe("the ticketed WebSocket", () => {
  test("local_ticket_is_local_principal (auth.whoami over the WebSocket); the server echoes finisher.v2 only", async () => {
    const s = await start();
    const ws = await connectWs(s);
    try {
      expect(ws.protocol).toBe("finisher.v2");
      const who = await ws.call("auth.whoami", {});
      expect(who).toMatchObject({ _tag: "Exit", exit: { _tag: "Success", value: { principal: { kind: "local", human: false } } } });
      const hello = await ws.call("server.hello", {});
      expect(hello).toMatchObject({ exit: { _tag: "Success", value: { protocolVersion: 2 } } });
    } finally {
      ws.close();
    }
    const con = await connectWs(s, "console");
    try {
      expect(await con.call("auth.whoami", {})).toMatchObject({ exit: { value: { principal: { kind: "console", human: true } } } });
    } finally {
      con.close();
    }
  });

  test("refusals: no ticket 401, a used ticket 401, a query ticket 401, wrong version 426, bad Host or Origin 403", async () => {
    const s = await start();
    const port = s.info.wsPort;
    const host = { Host: `127.0.0.1:${port}` };
    expect((await upgrade(port, { ...host, "Sec-WebSocket-Protocol": "finisher.v2" })).status).toBe(401);
    const ticket = await mintTicket(s);
    expect((await upgrade(port, { ...host, "Sec-WebSocket-Protocol": "finisher.v2" }, `/rpc?ticket=${ticket}`)).status).toBe(401);
    // Host and Origin refusals leave the ticket live.
    expect((await upgrade(port, { Host: `evil.example:${port}`, "Sec-WebSocket-Protocol": `finisher.v2, finisher.ticket.${ticket}` })).status).toBe(403);
    expect(
      (await upgrade(port, { ...host, Origin: "https://evil.example", "Sec-WebSocket-Protocol": `finisher.v2, finisher.ticket.${ticket}` })).status,
    ).toBe(403);
    const v1 = await upgrade(port, { ...host, "Sec-WebSocket-Protocol": `finisher.v1, finisher.ticket.${ticket}` });
    expect(v1.status).toBe(426);
    expect(JSON.parse(v1.body)).toEqual({ protocol: { min: 2, max: 2 } });
    const good = await upgrade(port, { ...host, "Sec-WebSocket-Protocol": `finisher.v2, finisher.ticket.${ticket}` });
    expect(good).toMatchObject({ status: 101, protocol: "finisher.v2" });
    expect((await upgrade(port, { ...host, "Sec-WebSocket-Protocol": `finisher.v2, finisher.ticket.${ticket}` })).status).toBe(401);
    expect((await upgrade(port, { ...host, "Sec-WebSocket-Protocol": "finisher.v2" }, "/")).status).toBe(404);
    // The old bearer header means nothing.
    expect((await upgrade(port, { ...host, Authorization: `Bearer ${ticket}`, "Sec-WebSocket-Protocol": "finisher.v2" })).status).toBe(401);
    await expect(openWs(port, ["finisher.v2"])).rejects.toMatchObject({ status: 401 });

    // Audited as auth.fail (aggregated per minute), never with the ticket.
    const db = new DatabaseSync(join(s.serverData, "control.db"), { readOnly: true });
    try {
      const rows = db.prepare("SELECT kind, credentialKind, count, details FROM audit WHERE kind IN ('auth.fail', 'auth.ticket')").all();
      const fails = rows.filter((r) => r.kind === "auth.fail");
      expect(fails.map((r) => r.credentialKind)).toEqual(["ticket"]);
      expect(Number(fails[0]!.count)).toBeGreaterThanOrEqual(5);
      expect(rows.some((r) => r.kind === "auth.ticket")).toBe(true);
      expect(JSON.stringify(rows)).not.toContain(ticket);
    } finally {
      db.close();
    }
  });

  test("a configured origin is allowed (settings.json server.origins)", async () => {
    const s = await start();
    writeFileSync(join(s.serverData, "settings.json"), JSON.stringify({ server: { origins: ["https://app.example"] } }));
    const ticket = await mintTicket(s);
    const port = s.info.wsPort;
    const r = await upgrade(port, { Host: `localhost:${port}`, Origin: "https://app.example", "Sec-WebSocket-Protocol": `finisher.v2, finisher.ticket.${ticket}` });
    expect(r.status).toBe(101);
  });

  test("an anonymous connection cannot mint a ticket; auth.revokeSelf from local is refused", async () => {
    const s = await start();
    const { NdjsonClient } = await import("./support/ndjson-client.ts");
    const anon = await NdjsonClient.connect(s.info.socket);
    const refused = await anon.call("auth.ticket", {});
    expect(!refused.ok && refused.error.code).toBe("UNAUTHENTICATED");
    await anon.close();
    const c = await connectProved(s);
    const self = await c.call("auth.revokeSelf", { id: "01otherotherotherotherothe" });
    expect(!self.ok && self.error.code).toBe("INVALID_REQUEST");
    const minted = ok<{ ticket: string; expiresAt: number }>(await c.call("auth.ticket", { ttlS: 600 }));
    // The server's `Date.now()` is another process's: on Windows V8 extrapolates it from the
    // performance counter, and two processes can differ by about one 15.6 ms timer tick.
    expect(minted.expiresAt - Date.now()).toBeLessThanOrEqual(60_000 + (process.platform === "win32" ? 50 : 0));
    await c.close();
  });
});
