// The audit log (plan 5A Task 1, Decision 4).
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { Audit, type AuditInput } from "../src/control/audit.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";

let dir: string;
let store: ControlStore;
let now: number;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "audit-"));
  store = openControlStore(dir, { serverId: "sid", checkpoint: false });
  now = Date.UTC(2026, 10, 7, 12, 0, 0);
});

afterEach(async () => {
  await store.close();
  rmSync(dir, { recursive: true, force: true });
});

const denied = (code = "SCOPE_DENIED"): AuditInput => ({
  kind: "auth.denied",
  principal: { kind: "agent", id: "01agent" },
  credentialKind: "agent",
  outcome: "denied",
  code,
  details: { rpc: "tx.commit", scope: "project.write" },
});

const fail: AuditInput = { kind: "auth.fail", principal: { kind: "anonymous", id: "-" }, credentialKind: "local", outcome: "failed", code: "UNAUTHENTICATED" };

function count(): number {
  return (store.db.prepare("SELECT COUNT(*) AS n FROM audit").get() as { n: number }).n;
}

describe("audit", () => {
  test("audit_append_list_cursor_purge", async () => {
    const audit = new Audit(store, () => now++);
    for (let i = 0; i < 25; i++) audit.append(denied(`C${i}`));
    audit.append({ kind: "credential.mint", principal: { kind: "console", id: "console" }, outcome: "ok" });
    const first = audit.list({ limit: 10 });
    expect(first.entries).toHaveLength(10);
    expect(first.entries[0]).toMatchObject({ kind: "credential.mint", principalKind: "console", outcome: "ok", count: 1, details: {} });
    expect(first.entries[1]).toMatchObject({
      kind: "auth.denied",
      principalKind: "agent",
      principalId: "01agent",
      credentialKind: "agent",
      code: "C24",
      details: { rpc: "tx.commit", scope: "project.write" },
    });
    expect(first.entries[1]).not.toHaveProperty("remote");
    const second = audit.list({ limit: 10, cursor: first.next! });
    const third = audit.list({ limit: 10, cursor: second.next! });
    expect(third.entries).toHaveLength(6);
    expect(third.next).toBeUndefined();
    const ids = [...first.entries, ...second.entries, ...third.entries].map((e) => e.id);
    expect(new Set(ids).size).toBe(26);
    expect(audit.list({ kind: "credential.mint" }).entries).toHaveLength(1);
    const start = Date.UTC(2026, 10, 7, 12, 0, 0);
    expect(audit.list({ since: start + 20 }).entries).toHaveLength(6);
    expect(audit.list({ limit: 5000 }).entries).toHaveLength(26);
    expect(await Effect.runPromise(audit.purge({ before: start + 10 }))).toBe(10);
    expect(count()).toBe(16);
    expect(await Effect.runPromise(audit.purge({ all: true }))).toBe(16);
    expect(count()).toBe(0);
  });

  test("auth_fail_aggregates_per_credential_kind_per_minute", () => {
    const audit = new Audit(store, () => now);
    // 100 failures (100 connections) within one minute: one row.
    for (let i = 0; i < 100; i++) {
      now += 100;
      audit.append(fail);
    }
    audit.append({ ...fail, credentialKind: "console" });
    now += 60_000; // the next minute: a new row
    audit.append(fail);
    const rows = audit.list({ kind: "auth.fail" }).entries;
    expect(rows.map((r) => [r.credentialKind, r.count])).toEqual([
      ["local", 1],
      ["console", 1],
      ["local", 100],
    ]);
  });

  test("retention_by_age_and_rows", async () => {
    const day = 86_400_000;
    const audit = new Audit(store, () => now, { maxRows: 50 });
    const at = (t: number) => new Audit(store, () => t);
    for (let i = 0; i < 5; i++) at(now - 200 * day + i).append(denied());
    for (let i = 0; i < 100; i++) at(now - day + i).append(denied());
    await Effect.runPromise(audit.sweep());
    const left = audit.list({ limit: 1000 }).entries;
    expect(left).toHaveLength(50);
    expect(Math.min(...left.map((e) => e.at))).toBe(now - day + 50); // the newest 50 kept
    // The default limits: 180 days.
    const def = new Audit(store, () => now);
    at(now - 181 * day).append(denied());
    await Effect.runPromise(def.sweep());
    expect(count()).toBe(50);
  });

  test("deletes_are_batched", async () => {
    store.tx(() => {
      const ins = store.db.prepare("INSERT INTO audit (at, kind, principalKind, principalId, outcome, details) VALUES (?, 'auth.denied', 'local', 'local', 'denied', '{}')");
      for (let i = 0; i < 10_000; i++) ins.run(i);
    });
    const audit = new Audit(store, () => now);
    let turns = 0;
    let running = true;
    const spin = () => {
      if (!running) return;
      turns++;
      setImmediate(spin);
    };
    setImmediate(spin);
    expect(await Effect.runPromise(audit.purge({ all: true }))).toBe(10_000);
    running = false;
    // ≥ 20 statements of ≤ 500 rows, the event loop turning between them.
    expect(turns).toBeGreaterThanOrEqual(19);
  });

  test("audit_failure_never_throws", async () => {
    const other = openControlStore(mkdtempSync(join(tmpdir(), "audit-closed-")), { serverId: "x", checkpoint: false });
    const audit = new Audit(other, () => now);
    audit.append(denied());
    await other.close();
    expect(() => audit.append(denied())).not.toThrow();
    expect(() => audit.append(fail)).not.toThrow();
    expect(audit.failures()).toBe(2);
  });
});
