// The WebSocket listener's work before authentication (review of 5A Task 6, finding 8): slow
// headers are cut off, open connections are capped, and `settings.json`'s origins are read
// again only when the file changes. Review of Task 7, M2: sockets that have not upgraded are
// capped apart from WebSockets and closed if they send nothing; WebSockets are capped per
// owner, and some are kept for `device` and `console`.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { connect } from "node:net";
import type { Duplex } from "node:stream";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect, Scope } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { WebSocket } from "ws";
import { cachedOrigins, settingsPath } from "../src/config.ts";
import { CONSOLE, LOCAL, type Principal } from "../src/principal.ts";
import {
  makeTicketedWebSocketServer,
  MAX_WS_LOCAL,
  MAX_WS_PER_OWNER,
  RESERVED_WS,
  type UpgradeResult,
  WebSocketSlots,
} from "../src/upgrade.ts";

const agent = (id: string): Principal => ({ kind: "agent", id, scopes: [], projects: "*", branches: "*", human: false });
const device: Principal = { kind: "device", id: "01devicedevicedevicedevice", scopes: [], projects: "*", branches: "*", human: true };

/** A listener in its own scope; `check` decides every upgrade. */
async function listener(
  check: (req: { headers: Record<string, unknown> }) => UpgradeResult,
  more: { maxUnauthenticated?: number; headersTimeoutMs?: number; atCapGraceMs?: number; perAddress?: number; remoteOf?: (s: Duplex) => string; slots?: WebSocketSlots } = {},
) {
  const scope = Effect.runSync(Scope.make());
  const { port } = await Effect.runPromise(
    Scope.provide(makeTicketedWebSocketServer({ check: check as never, maxPayload: 1024, ...more }), scope),
  );
  return { port, close: () => Effect.runPromise(Scope.close(scope, Effect.void as never)) };
}

/** Opens a WebSocket as `who` (the test's check reads it from a header): resolves with the socket or the refusal status. */
function open(port: number, who: string): Promise<{ ws?: WebSocket; status?: number }> {
  return new Promise((resolve) => {
    const ws = new WebSocket(`ws://127.0.0.1:${port}/rpc`, ["finisher.v2"], { headers: { "x-test-who": who } });
    ws.once("open", () => resolve({ ws }));
    ws.once("unexpected-response", (_req, res) => resolve({ status: res.statusCode ?? 0 }));
    ws.once("error", () => resolve({ status: -1 }));
  });
}

/** Ok for the principal and owner the header names (`<who>` or `<who>|<owner>`). */
const byHeader = (principals: Record<string, Principal>) => (req: { headers: Record<string, unknown> }): UpgradeResult => {
  const [who, owner] = String(req.headers["x-test-who"]).split("|");
  const principal = principals[who!]!;
  return { ok: true, principal, owner: owner ?? `${principal.kind}:${principal.id}`, protocol: "finisher.v2" };
};

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
            maxUnauthenticated: 2,
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

  test("a socket that connects and sends nothing is closed by the header timeout", { timeout: 10_000 }, async () => {
    const l = await listener(() => ({ ok: false, status: 401, reason: "badTicket" }), { headersTimeoutMs: 300 });
    try {
      const idle = await closedAfter(l.port, "");
      expect(idle.data).toBe("");
      expect(idle.ms).toBeGreaterThanOrEqual(250);
      expect(idle.ms).toBeLessThan(2_000);
    } finally {
      await l.close();
    }
  });

  test("upgraded WebSockets do not count against the unauthenticated cap", { timeout: 10_000 }, async () => {
    const l = await listener(byHeader({ a: agent("01aaaaaaaaaaaaaaaaaaaaaaaa"), b: agent("01bbbbbbbbbbbbbbbbbbbbbbbb") }), { maxUnauthenticated: 1 });
    try {
      const first = await open(l.port, "a");
      expect(first.ws).toBeDefined();
      // The first no longer holds the one unauthenticated slot.
      const second = await open(l.port, "b");
      expect(second.ws).toBeDefined();
      for (const r of [first, second]) r.ws?.terminate();
    } finally {
      await l.close();
    }
  });
});

describe("WebSocket caps (review of Task 7, M2)", () => {
  test("a principal holds at most MAX_WS_PER_OWNER WebSockets; others still connect", { timeout: 20_000 }, async () => {
    const l = await listener(byHeader({ a: agent("01aaaaaaaaaaaaaaaaaaaaaaaa"), c: CONSOLE, d: device }));
    const held: WebSocket[] = [];
    try {
      for (let i = 0; i < MAX_WS_PER_OWNER; i++) held.push((await open(l.port, "a")).ws!);
      expect(held.every((w) => w !== undefined)).toBe(true);
      expect((await open(l.port, "a")).status).toBe(503);
      const others = [await open(l.port, "c"), await open(l.port, "d")];
      expect(others.map((o) => o.ws !== undefined)).toEqual([true, true]);
      for (const o of others) o.ws?.terminate();
      // A closed one frees its slot.
      held.pop()!.terminate();
      await new Promise((r) => setTimeout(r, 100));
      const again = await open(l.port, "a");
      expect(again.ws).toBeDefined();
      held.push(again.ws!);
    } finally {
      for (const w of held) w.terminate();
      await l.close();
    }
  });

  test("slots: per owner, local as a whole, and the reserved ones", () => {
    const slots = new WebSocketSlots(RESERVED_WS + 10);
    // `local` per connection, under the local-wide cap.
    const locals: Array<() => void> = [];
    for (let i = 0; i < MAX_WS_PER_OWNER; i++) locals.push(slots.acquire(LOCAL, "local:socket:1")!);
    expect(slots.acquire(LOCAL, "local:socket:1")).toBeUndefined();
    expect(slots.acquire(LOCAL, "local:socket:2")).toBeDefined();
    expect(slots.size).toBe(MAX_WS_PER_OWNER + 1);
    // Agents fill what is not reserved; then only device and console get in.
    for (let i = 0; slots.size < 10; i++) expect(slots.acquire(agent(`01${String(i).padStart(24, "0")}`), `agent:${i}`)).toBeDefined();
    expect(slots.acquire(agent("01zzzzzzzzzzzzzzzzzzzzzzzz"), "agent:z")).toBeUndefined();
    expect(slots.acquire(LOCAL, "local:socket:3")).toBeUndefined();
    expect(slots.acquire(device, `device:${device.id}`)).toBeDefined();
    expect(slots.acquire(CONSOLE, "console:")).toBeDefined();
    // A release gives the slot back once.
    const release = locals[0]!;
    release();
    release();
    expect(slots.size).toBe(11);
    const big = new WebSocketSlots(1000);
    let n = 0;
    for (let c = 0; c < 10; c++) for (let i = 0; i < MAX_WS_PER_OWNER; i++) if (big.acquire(LOCAL, `local:socket:${c}`)) n++;
    expect(n).toBe(MAX_WS_LOCAL);
  });
});

describe("settings origins", () => {
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

describe("review of Tasks 8–9: the unauthenticated slots at the cap", () => {
  test("at the cap a new socket closes the ones that waited past the short grace, so a WebSocket still gets in", { timeout: 20_000 }, async () => {
    const l = await listener(byHeader({ c: CONSOLE }), { maxUnauthenticated: 2, headersTimeoutMs: 10_000, atCapGraceMs: 200 });
    try {
      const held = await Promise.all(
        [0, 1].map(
          () =>
            new Promise<ReturnType<typeof connect>>((r) => {
              const s = connect({ host: "127.0.0.1", port: l.port }, () => r(s));
              s.on("error", () => undefined);
            }),
        ),
      );
      const closed = held.map((s) => new Promise<void>((r) => s.once("close", () => r())));
      // Within the grace a newcomer is still refused at once...
      const early = await closedAfter(l.port, "GET /rpc HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
      expect(early.data).toBe("");
      // ...past it the idle ones go and the newcomer upgrades.
      await new Promise((r) => setTimeout(r, 300));
      const r = await open(l.port, "c");
      expect(r.status).toBeUndefined();
      r.ws!.close();
      await Promise.all(closed);
    } finally {
      await l.close();
    }
  });
});

// 5B Task 6 (Decision 12): pre-upgrade pools per listener, a per-address cap on the remote
// listener, one pool of WebSocket slots for every listener.
/** A raw socket from `localPort` that stays open until the server closes it: resolves `isOpen`. */
function hold(port: number, localPort: number): Promise<{ isOpen: () => boolean; close: () => void }> {
  return new Promise((resolve) => {
    let open = true;
    const sock = connect({ host: "127.0.0.1", port, localPort, localAddress: "127.0.0.1" }, () => resolve({ isOpen: () => open, close: () => sock.destroy() }));
    sock.on("close", () => (open = false));
    sock.on("error", () => (open = false));
  });
}
const settle = () => new Promise((r) => setTimeout(r, 150));

describe("per listener (5B)", () => {
  test("per_address_cap: a fifth socket from one address is closed; another address gets in", { timeout: 20_000 }, async () => {
    const base = 40_000 + Math.floor(Math.random() * 10_000);
    const remoteOf = (s: { remotePort?: number }) => ((s.remotePort ?? 0) >= base + 100 ? "remote-b" : "remote-a");
    const l = await listener(byHeader({ device }), { perAddress: 4, remoteOf: remoteOf as never });
    try {
      const a = [];
      for (let i = 0; i < 5; i++) a.push(await hold(l.port, base + i));
      await settle();
      expect(a.map((x) => x.isOpen())).toEqual([true, true, true, true, false]);
      const b = await hold(l.port, base + 100);
      await settle();
      expect(b.isOpen()).toBe(true);
      // A socket that ends frees its address's room.
      a[0]!.close();
      await settle();
      const again = await hold(l.port, base + 5);
      await settle();
      expect(again.isOpen()).toBe(true);
      for (const x of [...a, b, again]) x.close();
    } finally {
      await l.close();
    }
  });

  test("remote_pool_does_not_starve_loopback", { timeout: 20_000 }, async () => {
    const remote = await listener(byHeader({ device }), { maxUnauthenticated: 4 });
    const loopback = await listener(byHeader({ console: CONSOLE }), { maxUnauthenticated: 4 });
    try {
      const held = [];
      for (let i = 0; i < 6; i++) held.push(await hold(remote.port, 0));
      await settle();
      // The remote pool is full, the loopback one is not: the console still connects there.
      const got = await open(loopback.port, "console");
      expect(got.ws).toBeDefined();
      got.ws!.close();
      for (const h of held) h.close();
    } finally {
      await remote.close();
      await loopback.close();
    }
  });

  test("the reserve split: agents take the unreserved slots, devices the rest", () => {
    const slots = new WebSocketSlots(RESERVED_WS + 4);
    const releases = [];
    for (let i = 0; i < 4; i++) releases.push(slots.acquire(agent(`a${i}`), `agent:a${i}`));
    expect(releases.every((r) => r !== undefined)).toBe(true);
    expect(slots.acquire(agent("a9"), "agent:a9")).toBeUndefined();
    // The reserve stays for the device and the console (8 each per owner).
    let devices = 0;
    for (let i = 0; i < 2; i++) for (let j = 0; j < MAX_WS_PER_OWNER; j++) if (slots.acquire({ ...device, id: `d${i}` }, `device:d${i}`)) devices++;
    expect(devices).toBe(RESERVED_WS);
    expect(slots.acquire(CONSOLE, "console:console")).toBeUndefined();
  });

  test("ws_slots_shared_with_device_reserve", { timeout: 20_000 }, async () => {
    const slots = new WebSocketSlots(4);
    const principals: Record<string, Principal> = { device };
    for (let i = 0; i < 4; i++) principals[`a${i}`] = agent(`01agent${String(i).padStart(19, "0")}`);
    const one = await listener(byHeader(principals), { slots });
    const two = await listener(byHeader(principals), { slots });
    const sockets: WebSocket[] = [];
    try {
      // 4 in all, minus the 16-slot reserve floor: agents get none of a pool of 4 …
      expect((await open(one.port, "a0")).status).toBe(503);
      // … the device takes them, across both listeners.
      for (const port of [one.port, two.port, one.port, two.port]) {
        const got = await open(port, "device");
        expect(got.ws).toBeDefined();
        sockets.push(got.ws!);
      }
      expect(slots.size).toBe(4);
      expect((await open(two.port, "device")).status).toBe(503);
    } finally {
      for (const w of sockets) w.close();
      await one.close();
      await two.close();
    }
  });
});
