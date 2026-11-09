// 5B Task 1: the `devices` control chain and `DeviceStore` (Decisions 2, 3): migrated once,
// rows loaded at start, at most 64 live devices, `lastSeenAt` written once a minute, ended rows
// deleted after 180 days, names checked, the device principal the owner's.
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type DeviceJwk, MAX_LIVE_DEVICES, SCOPES } from "@finisher/contracts";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { DEVICE_RETENTION_MS, DeviceStore, SEEN_WRITE_MS } from "../src/auth/devices.ts";
import { Audit } from "../src/control/audit.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import { exitCodeOf } from "../src/console/exit-codes.ts";
import { FinError } from "@finisher/contracts";

let tmp: string;
let serverData: string;
let store: ControlStore | undefined;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-devices-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(async () => {
  await store?.close();
  store = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

function clock(start = 1_800_000_000_000) {
  let now = start;
  return { now: () => now, advance: (ms: number) => (now += ms) };
}

const open = (c = clock()) => {
  store = openControlStore(serverData, { serverId: "sid-1", checkpoint: false, clock: c.now });
  return { store, devices: new DeviceStore(store, new Audit(store, c.now), c.now) };
};
const reopen = async (c: ReturnType<typeof clock>) => {
  await store?.close();
  return open(c);
};

const jwk = (n: number): DeviceJwk => ({ kty: "EC", crv: "P-256", x: `x${n}`, y: `y${n}` });
const device = (n: number, name = `Device ${n}`) => ({ name, platform: "linux" as const, jwk: jwk(n), jkt: `jkt-${n}`, approvedBy: "console:console" });

const codeOf = (f: () => unknown): string | undefined => {
  try {
    f();
  } catch (e) {
    return e instanceof FinError ? e.code : String(e);
  }
  return undefined;
};

describe("DeviceStore", () => {
  test("devices_chain_migrates_once", async () => {
    const c = clock();
    const { devices } = open(c);
    devices.add(device(1));
    const rows = () => store!.db.prepare("SELECT chain, n FROM migrations WHERE chain = 'devices'").all();
    expect(rows()).toEqual([{ chain: "devices", n: 1 }]);
    const meta = store!.db.prepare("SELECT value FROM meta WHERE key = 'formatVersion.devices'").get() as { value: string };
    expect(meta.value).toBe("1");
    const again = await reopen(c);
    expect(rows()).toEqual([{ chain: "devices", n: 1 }]);
    // Rows load at start.
    expect(again.devices.list().map((d) => d.name)).toEqual(["Device 1"]);
    expect(again.devices.byJkt("jkt-1")?.name).toBe("Device 1");
    // The pairing tables exist; the safety code and the address may stay NULL.
    for (const t of ["pair_offers", "pair_pending"]) {
      expect(store!.db.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?").get(t), t).toBeDefined();
    }
    const cols = store!.db.prepare("PRAGMA table_info(pair_pending)").all() as Array<{ name: string; notnull: number }>;
    expect(cols.find((x) => x.name === "safetyCode")?.notnull).toBe(0);
    expect(cols.find((x) => x.name === "remote")?.notnull).toBe(0);
  });

  test("device_limit_is_exit_4", () => {
    const { devices } = open();
    for (let i = 0; i < MAX_LIVE_DEVICES; i++) devices.add(device(i));
    expect(codeOf(() => devices.add(device(999)))).toBe("DEVICE_LIMIT");
    expect(exitCodeOf("DEVICE_LIMIT")).toBe(4);
    // Revoking one makes room.
    expect(devices.markEnded(devices.list()[0]!.id, "owner")).toBe(true);
    devices.persist();
    expect(devices.add(device(999)).name).toBe("Device 999");
    expect(devices.liveCount()).toBe(MAX_LIVE_DEVICES);
  });

  test("last_seen_written_once_a_minute", async () => {
    const c = clock();
    const { devices } = open(c);
    const d = devices.add(device(1));
    const written = () => store!.db.prepare("SELECT lastSeenAt, lastRoute FROM devices WHERE id = ?").get(d.id);
    expect(devices.seen(d.id, "lan")).toBe(true);
    const first = c.now();
    c.advance(SEEN_WRITE_MS - 1);
    expect(devices.seen(d.id, "tailscale")).toBe(false);
    // In memory at once, written later.
    expect(devices.list()[0]).toMatchObject({ lastSeenAt: c.now(), lastRoute: "tailscale" });
    expect(written()).toEqual({ lastSeenAt: first, lastRoute: "lan" });
    c.advance(1);
    expect(devices.seen(d.id, "tailscale")).toBe(true);
    expect(written()).toEqual({ lastSeenAt: c.now(), lastRoute: "tailscale" });
    // A route is a kind, never an address.
    expect(Object.values(written() as object).some((v) => typeof v === "string" && /\d+\.\d+/.test(v))).toBe(false);
    // An ended device is not seen.
    devices.markEnded(d.id, "owner");
    expect(devices.seen(d.id, "lan")).toBe(false);
  });

  test("ended_rows_deleted_after_180_days", async () => {
    const c = clock();
    const { devices } = open(c);
    const a = devices.add(device(1));
    const b = devices.add(device(2));
    devices.markEnded(a.id, "owner");
    devices.persist();
    expect(devices.byJkt("jkt-1")).toBeUndefined();
    expect(devices.list().map((d) => d.id)).toEqual([b.id]);
    expect(devices.list(true).map((d) => d.id)).toEqual([a.id, b.id]);
    c.advance(DEVICE_RETENTION_MS - 1);
    expect((await reopen(c)).devices.list(true).map((d) => d.id)).toEqual([a.id, b.id]);
    c.advance(2);
    const later = await reopen(c);
    expect(later.devices.list(true).map((d) => d.id)).toEqual([b.id]);
    expect(later.devices.get(b.id)?.revokedAt).toBeUndefined();
  });

  test("ending_is_in_memory_first_then_persisted", async () => {
    const c = clock();
    const { devices } = open(c);
    const a = devices.add(device(1));
    expect(devices.markEnded(a.id, "rotation")).toBe(true);
    expect(devices.markEnded(a.id, "owner")).toBe(false);
    expect(devices.unsaved()).toBe(1);
    expect(devices.byJkt("jkt-1")).toBeUndefined();
    devices.persist();
    expect(devices.unsaved()).toBe(0);
    expect(store!.db.prepare("SELECT revokedReason FROM devices WHERE id = ?").get(a.id)).toEqual({ revokedReason: "rotation" });
    // A revoked device's key never pairs again.
    expect(codeOf(() => devices.add(device(1)))).toBe("INVALID_REQUEST");
  });

  test("name_rules", () => {
    const { devices } = open();
    expect(codeOf(() => devices.add(device(1, "")))).toBe("INVALID_NAME");
    expect(codeOf(() => devices.add(device(1, "x".repeat(65))))).toBe("INVALID_NAME");
    expect(codeOf(() => devices.add(device(1, "Café")))).toBe("INVALID_NAME");
    expect(codeOf(() => devices.add(device(1, "bad\u0007name")))).toBe("INVALID_NAME");
    const d = devices.add(device(1, "Café tablet"));
    expect(codeOf(() => devices.rename(d.id, "two\nlines"))).toBe("INVALID_NAME");
    expect(devices.rename(d.id, "Edit bay").name).toBe("Edit bay");
    expect(codeOf(() => devices.rename("01nope00000000000000000000", "x"))).toBe("NOT_FOUND");
    expect(codeOf(() => devices.add(device(1, "Same key")))).toBe("INVALID_REQUEST");
  });

  test("rename_audited_with_id_only (owner decision 4)", () => {
    const c = clock();
    const { devices } = open(c);
    const d = devices.add(device(1));
    devices.rename(d.id, "Edit bay", { kind: "console", id: "console", scopes: [], projects: "*", branches: "*", human: true });
    const row = store!.db.prepare("SELECT details FROM audit WHERE kind = 'device.rename'").get() as { details: string };
    expect(JSON.parse(row.details)).toEqual({ deviceId: d.id });
  });

  test("the device principal is the owner (Decision 3)", () => {
    const { devices } = open();
    const d = devices.add(device(1));
    expect(devices.principalOf(d)).toEqual({ kind: "device", id: d.id, scopes: [...SCOPES], projects: "*", branches: "*", human: true });
    // The owner's view never carries the key.
    expect(Object.keys(devices.list()[0]!)).not.toContain("jwk");
  });
});
