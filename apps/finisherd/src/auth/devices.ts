// Paired devices (spec §5.1, §5.6; plan 5B Decisions 2, 3, 24, 25).
//
// - **A device is the owner** on another machine or in the app: its principal holds every
//   scope, every project and branch, and is human (`principalOf`). It is named by the RFC 7638
//   thumbprint (`jkt`) of its P-256 key; `byJkt` finds live ones only.
// - Rows of `control.db` `devices` (chain `devices`) load once at start; rows ended over 180
//   days ago are deleted then, as 5A's credentials are. At most `MAX_LIVE_DEVICES` live
//   (`DEVICE_LIMIT`, exit 4: the owner revokes some).
// - `seen` keeps the last use and route kind in memory and writes them at most once a minute;
//   the route is a kind, never an address (owner decision 4).
// - Ending one (`markEnded`) happens in memory first, then `persist` writes it (retried), so a
//   busy store never holds a revocation back (5A's L2; the revoke path is `revoke.ts`).
import type { Device, DeviceEndReason, DeviceJwk, Platform, Principal, RouteKind } from "@finisher/contracts";
import { DEVICE_END_REASONS, MAX_LIVE_DEVICES, nameProblem, PLATFORMS, ROUTE_KINDS, SCOPES } from "@finisher/contracts";
import type { StatementSync } from "node:sqlite";
import type { Audit } from "../control/audit.ts";
import type { ControlStore } from "../control/db.ts";
import { registerChain } from "../control/migrations.ts";
import { devices0001 } from "../control/migrations/devices/0001.ts";
import { ulid } from "../control/ulid.ts";
import { finError } from "../errors.ts";

registerChain("devices", [devices0001], { formatVersion: 1, minReaderVersion: 1 });

/** `lastSeenAt` is written at most this often (Decision 2). */
export const SEEN_WRITE_MS = 60_000;
/** Ended rows are deleted this long after they ended. */
export const DEVICE_RETENTION_MS = 180 * 86_400_000;

export interface DeviceRow {
  readonly id: string;
  name: string;
  readonly platform: Platform;
  readonly jwk: DeviceJwk;
  readonly jkt: string;
  readonly createdAt: number;
  readonly approvedBy: string;
  lastSeenAt?: number;
  lastRoute?: RouteKind;
  revokedAt?: number;
  revokedReason?: DeviceEndReason;
}

/** A change `auth.devices.watch` reports. */
export type DeviceEvent =
  | { readonly kind: "added"; readonly device: Device }
  | { readonly kind: "renamed"; readonly device: Device }
  | { readonly kind: "revoked"; readonly id: string }
  | { readonly kind: "seen"; readonly id: string; readonly at: number; readonly route: RouteKind };

export interface NewDevice {
  readonly name: string;
  readonly platform: Platform;
  readonly jwk: DeviceJwk;
  readonly jkt: string;
  /** `<kind>:<id>` of the approving principal. */
  readonly approvedBy: string;
}

const invalidName = (field: string, problem: string) =>
  finError(
    "INVALID_NAME",
    `the ${field} is not a usable name (${problem})`,
    { field, reason: problem },
    "Use a name of 1 to 64 characters without control characters.",
  );

/** `name` checked as a device's or environment's name (`INVALID_NAME`), unchanged. */
export function checkedName(name: string, field = "name"): string {
  const problem = nameProblem(name);
  if (problem !== undefined) throw invalidName(field, problem);
  return name;
}

const deviceLimit = () =>
  finError(
    "DEVICE_LIMIT",
    `this server already has ${MAX_LIVE_DEVICES} paired devices`,
    { limit: MAX_LIVE_DEVICES },
    "Revoke a device you no longer use, then pair this one again.",
  );

const deviceNotFound = (id: string) =>
  finError("NOT_FOUND", "no live device has this id", { id }, "List the paired devices and use the id shown there.");

const ROUTES: ReadonlySet<string> = new Set(ROUTE_KINDS);
const PLATFORM_SET: ReadonlySet<string> = new Set(PLATFORMS);
const END_REASONS: ReadonlySet<string> = new Set(DEVICE_END_REASONS);

export class DeviceStore {
  readonly #audit: Audit;
  readonly #db: ControlStore["db"];
  readonly #clock: () => number;
  readonly #rows = new Map<string, DeviceRow>();
  readonly #byJkt = new Map<string, DeviceRow>();
  /** The `lastSeenAt` last written, per id. */
  readonly #written = new Map<string, number>();
  readonly #insert: StatementSync;
  readonly #rename: StatementSync;
  readonly #seen: StatementSync;
  readonly #end: StatementSync;
  readonly #unsaved = new Set<string>();
  readonly #listeners = new Set<(e: DeviceEvent) => void>();
  #retry: ReturnType<typeof setTimeout> | undefined;
  #backoffMs = 500;

  constructor(store: ControlStore, audit: Audit, clock: () => number = Date.now) {
    this.#audit = audit;
    this.#clock = clock;
    const db = store.db;
    this.#db = db;
    db.prepare("DELETE FROM devices WHERE revokedAt IS NOT NULL AND revokedAt < ?").run(clock() - DEVICE_RETENTION_MS);
    for (const r of db.prepare("SELECT * FROM devices ORDER BY createdAt, rowid").all() as Array<Record<string, unknown>>) {
      let jwk: DeviceJwk;
      try {
        jwk = JSON.parse(String(r.jwk)) as DeviceJwk;
      } catch {
        // A damaged row never stops the start; its device cannot prove, so it pairs again.
        process.stderr.write("finisherd: warning: a paired device's row is damaged and was skipped\n");
        continue;
      }
      const reason = String(r.revokedReason);
      const platform = String(r.platform);
      const route = r.lastRoute === null ? undefined : String(r.lastRoute);
      const row: DeviceRow = {
        id: String(r.id),
        name: String(r.name),
        platform: (PLATFORM_SET.has(platform) ? platform : "other") as Platform,
        jwk,
        jkt: String(r.jkt),
        createdAt: Number(r.createdAt),
        approvedBy: String(r.approvedBy),
        ...(r.lastSeenAt === null ? {} : { lastSeenAt: Number(r.lastSeenAt) }),
        ...(route === undefined ? {} : { lastRoute: (ROUTES.has(route) ? route : "other") as RouteKind }),
        ...(r.revokedAt === null ? {} : { revokedAt: Number(r.revokedAt), revokedReason: (END_REASONS.has(reason) ? reason : "owner") as DeviceEndReason }),
      };
      this.#rows.set(row.id, row);
      if (row.revokedAt === undefined) this.#byJkt.set(row.jkt, row);
      if (row.lastSeenAt !== undefined) this.#written.set(row.id, row.lastSeenAt);
    }
    this.#insert = db.prepare(
      "INSERT INTO devices (id, name, platform, jwk, jkt, createdAt, approvedBy, lastSeenAt, lastRoute, revokedAt, revokedReason) " +
        "VALUES (?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, NULL)",
    );
    this.#rename = db.prepare("UPDATE devices SET name = ? WHERE id = ?");
    this.#seen = db.prepare("UPDATE devices SET lastSeenAt = ?, lastRoute = ? WHERE id = ?");
    this.#end = db.prepare("UPDATE devices SET revokedAt = ?, revokedReason = ? WHERE id = ? AND revokedAt IS NULL");
  }

  /** The owner's view of a row: never its key. */
  view(row: DeviceRow): Device {
    return {
      id: row.id,
      name: row.name,
      platform: row.platform,
      jkt: row.jkt,
      createdAt: row.createdAt,
      approvedBy: row.approvedBy,
      ...(row.lastSeenAt === undefined ? {} : { lastSeenAt: row.lastSeenAt }),
      ...(row.lastRoute === undefined ? {} : { lastRoute: row.lastRoute }),
      ...(row.revokedAt === undefined ? {} : { revokedAt: row.revokedAt }),
    };
  }

  /** Live devices, or every row kept (`all`), oldest first. */
  list(all = false): ReadonlyArray<Device> {
    return [...this.#rows.values()].filter((r) => all || r.revokedAt === undefined).map((r) => this.view(r));
  }

  /** The live device with this id. */
  get(id: string): DeviceRow | undefined {
    const row = this.#rows.get(id);
    return row?.revokedAt === undefined ? row : undefined;
  }

  /** The live device whose key has thumbprint `jkt`. */
  byJkt(jkt: string): DeviceRow | undefined {
    return this.#byJkt.get(jkt);
  }

  /** Whether `jkt` is any kept device's key, live or revoked (a revoked one's proof says so). */
  isKnownJkt(jkt: string): boolean {
    if (this.#byJkt.has(jkt)) return true;
    for (const r of this.#rows.values()) if (r.jkt === jkt) return true;
    return false;
  }

  /** `auth.devices.watch` (Task 9): every change after it happened; the returned function stops it. */
  onEvent(f: (e: DeviceEvent) => void): () => void {
    this.#listeners.add(f);
    return () => void this.#listeners.delete(f);
  }

  #emit(e: DeviceEvent): void {
    for (const f of [...this.#listeners]) {
      try {
        f(e);
      } catch {
        // A watcher's failure is its own.
      }
    }
  }

  /** Announces a device `add` put in place, once its transaction committed (pairing calls it). */
  announceAdded(d: Device): void {
    this.#emit({ kind: "added", device: d });
  }

  /** Forgets the device of `jkt` added in a transaction that rolled back (memory only). */
  forgetByJkt(jkt: string): void {
    const row = this.#byJkt.get(jkt);
    if (row !== undefined) this.forget(row.id);
  }

  /** Forgets a device whose `add` was rolled back with its transaction (memory only). */
  forget(id: string): void {
    const row = this.#rows.get(id);
    if (row === undefined) return;
    this.#rows.delete(id);
    if (this.#byJkt.get(row.jkt) === row) this.#byJkt.delete(row.jkt);
  }

  /** Live devices now. */
  liveCount(): number {
    return this.#byJkt.size;
  }

  /**
   * Adds an approved device (pairing's approval runs it inside its own transaction). Its name
   * is checked (`INVALID_NAME`), its key must not be a live device's (`INVALID_REQUEST`), and a
   * server with `MAX_LIVE_DEVICES` live ones refuses it (`DEVICE_LIMIT`).
   */
  add(d: NewDevice): Device {
    checkedName(d.name, "device name");
    if (!PLATFORM_SET.has(d.platform)) throw finError("INVALID_REQUEST", "unknown platform", { field: "platform" });
    if (this.#byJkt.has(d.jkt)) throw finError("INVALID_REQUEST", "this key is already a paired device's", { field: "jwk" });
    // `jkt` is unique over every row kept: a revoked device's key does not pair again while its
    // row is kept (180 days).
    if ([...this.#rows.values()].some((r) => r.jkt === d.jkt)) {
      throw finError("INVALID_REQUEST", "this key belonged to a revoked device: pair with a new key", { field: "jwk" });
    }
    if (this.#byJkt.size >= MAX_LIVE_DEVICES) throw deviceLimit();
    const now = this.#clock();
    const row: DeviceRow = { id: ulid(now), name: d.name, platform: d.platform, jwk: d.jwk, jkt: d.jkt, createdAt: now, approvedBy: d.approvedBy };
    this.#insert.run(row.id, row.name, row.platform, JSON.stringify(row.jwk), row.jkt, row.createdAt, row.approvedBy);
    this.#rows.set(row.id, row);
    this.#byJkt.set(row.jkt, row);
    return this.view(row);
  }

  /**
   * Renames a live device (`NOT_FOUND`, `INVALID_NAME`); audited `device.rename {deviceId}`
   * when `by` is given: never the name (owner decision 4 keeps names to pairing, revocation and
   * failures; the plan's Decision 25 named it).
   */
  rename(id: string, name: string, by?: Principal): Device {
    const row = this.get(id);
    if (row === undefined) throw deviceNotFound(id);
    checkedName(name, "device name");
    this.#rename.run(name, id);
    row.name = name;
    this.#emit({ kind: "renamed", device: this.view(row) });
    if (by !== undefined) {
      this.#audit.append({ kind: "device.rename", principal: { kind: by.kind, id: by.id }, outcome: "ok", details: { deviceId: id } });
    }
    return this.view(row);
  }

  /**
   * Notes a use of `id` over a route of kind `route`: kept in memory, written at most once a
   * minute. True when this use was written (the watch stream's `seen`, at most once a minute).
   */
  seen(id: string, route: RouteKind): boolean {
    const row = this.get(id);
    if (row === undefined) return false;
    const now = this.#clock();
    row.lastSeenAt = now;
    row.lastRoute = route;
    if (now - (this.#written.get(id) ?? Number.NEGATIVE_INFINITY) < SEEN_WRITE_MS) return false;
    this.#written.set(id, now);
    this.#emit({ kind: "seen", id, at: now, route });
    try {
      this.#seen.run(now, route, id);
    } catch (e) {
      // A busy store must not fail the request; the next minute's use writes it (the throttle
      // stands, so a busy store is not retried on every request).
      process.stderr.write(`finisherd: warning: cannot record a device's use: ${String(e)}\n`);
      return false;
    }
    return true;
  }

  /**
   * Ends `id` in memory: from now on `byJkt` does not find it, so no proof by its key gets a
   * token. Writing it is `persist`'s. True when it was live.
   */
  markEnded(id: string, reason: DeviceEndReason): boolean {
    const row = this.#rows.get(id);
    if (row === undefined || row.revokedAt !== undefined) return false;
    row.revokedAt = this.#clock();
    row.revokedReason = reason;
    this.#byJkt.delete(row.jkt);
    this.#unsaved.add(id);
    this.#emit({ kind: "revoked", id });
    return true;
  }

  /** Writes the ends `markEnded` kept, retrying (500 ms doubling to 60 s) while the store is busy. */
  persist(): void {
    if (this.#retry !== undefined) {
      clearTimeout(this.#retry);
      this.#retry = undefined;
    }
    for (const id of [...this.#unsaved]) {
      const row = this.#rows.get(id);
      if (row?.revokedAt === undefined) {
        this.#unsaved.delete(id);
        continue;
      }
      try {
        this.#end.run(row.revokedAt, row.revokedReason ?? "owner", id);
        this.#unsaved.delete(id);
      } catch (e) {
        process.stderr.write(`finisherd: warning: cannot record a device's end yet (retrying): ${String(e)}\n`);
        this.#backoffMs = Math.min(this.#backoffMs * 2, 60_000);
        this.#retry = setTimeout(() => this.persist(), this.#backoffMs);
        this.#retry.unref();
        return;
      }
    }
    this.#backoffMs = 500;
  }

  /**
   * Deletes every row (`env.reset`: the data is a copy, so its devices belong to the original).
   * Callers end the devices first; this forgets them.
   */
  deleteAll(): number {
    this.#unsaved.clear();
    const n = this.#rows.size;
    this.#db.prepare("DELETE FROM devices").run();
    this.#rows.clear();
    this.#byJkt.clear();
    this.#written.clear();
    return n;
  }

  /** Ends not yet written. */
  unsaved(): number {
    return this.#unsaved.size;
  }

  /** A last write of the ends kept, then the retry timer stops (the server's shutdown; the control store closes after). */
  dispose(): void {
    if (this.#unsaved.size > 0) this.persist();
    if (this.#retry !== undefined) clearTimeout(this.#retry);
    this.#retry = undefined;
  }

  /** The owner on this device (Decision 3): every scope, project and branch; a person. */
  principalOf(d: Pick<Device, "id">): Principal {
    return { kind: "device", id: d.id, scopes: [...SCOPES], projects: "*", branches: "*", human: true };
  }
}
