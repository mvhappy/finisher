// Pairing (5B Decisions 17–21, spec §5.2, §5.10): a one-time offer by link or QR code, redeemed
// by a device with its P-256 key, approved by the owner after comparing the device's name and
// safety code.
//
// - **Offers** (`offer`): 32 random bytes of secret; only `SHA-256(secret)` is stored. At most 8
//   open (the oldest expires first), each for at most 10 minutes. The link is
//   `finisher://pair?v=1&env=…&spki=…&r=<base64url(JSON routes)>#<offerId>.<secret>`: the
//   routes in `r` are capped (`linkRoutes`: non-loopback ones, public before Tailscale before
//   LAN, at most `MAX_LINK_ROUTES`; loopback alone when the server listens nowhere else), so
//   the QR code stays small (the console falls back to the link when it is still too wide).
// - **Redeem** (`redeem`, `POST /pair/redeem`): the secret compared in constant time; the
//   offer is consumed at the **first valid** redeem (a second is `PAIRING_USED`, which the
//   legitimate device sees); an unknown, expired or wrong one is `PAIRING_EXPIRED` (they cannot
//   be told apart). The answer is a pending id and the safety code (the first 4 bytes of
//   `HMAC-SHA-256(secret, jkt)`, big-endian, mod 10⁶, six digits). At most 16 pending, each for
//   10 minutes. A `self` offer (the console's, for the desktop shell's own renderer) redeems
//   only over the loopback listener and is approved at once.
// - **Approve / reject** (the owner): approval adds the device and marks the request in one
//   transaction. `pending()` streams the requests, snapshot first, never the address.
// - **Status** (`POST /pair/status`): by a DPoP proof of the pending request's key.
// The safety code and the address stay in memory (the `pair_pending` columns are NULL): a
// request pending at a restart loads as expired. The audit holds names and addresses for
// pairing only (owner decision 4); the journal (`auth.pair`) holds neither.
import { createHash, createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import type { Device, PairPendingItem, PendingView, Platform, Principal, Route, RouteKind } from "@finisher/contracts";
import { MAX_OPEN_OFFERS, MAX_PENDING, nameProblem, PAIR_OFFER_DEFAULT_TTL_S, PAIR_OFFER_MAX_TTL_S, PairRedeemBody, PENDING_TTL_S } from "@finisher/contracts";
import { qrSvg } from "@finisher/fin-node";
import { Context, Effect, PubSub, Schema, Stream } from "effect";
import { calculateJwkThumbprint } from "jose";
import type { Audit } from "../control/audit.ts";
import type { ControlStore } from "../control/db.ts";
import { ulid } from "../control/ulid.ts";
import type { Environment } from "../env/descriptor.ts";
import { envMoved } from "../env/descriptor.ts";
import { finError } from "../errors.ts";
import { checkedName, type DeviceStore } from "./devices.ts";

/** Routes a link carries at most (the QR code's size). */
export const MAX_LINK_ROUTES = 3;
const MIN_TTL_S = 30;

export interface JournalLike {
  readonly enabled: boolean;
  record(kind: string, kv: number, ctxJson: string | null, payloadJson: string, source: string): void;
}

export interface PairingDeps {
  readonly store: ControlStore;
  readonly devices: DeviceStore;
  readonly audit: Audit;
  readonly env: Environment;
  readonly journal?: JournalLike | undefined;
  readonly clock?: () => number;
}

interface Offer {
  readonly id: string;
  readonly secretHash: Buffer;
  readonly label: string | undefined;
  readonly self: boolean;
  readonly createdBy: string;
  readonly createdAt: number;
  readonly expiresAt: number;
  state: "open" | "used" | "expired";
}

interface Pending {
  readonly id: string;
  readonly offerId: string;
  readonly deviceName: string;
  readonly platform: Platform;
  readonly jwk: { kty: "EC"; crv: "P-256"; x: string; y: string };
  readonly jkt: string;
  readonly safetyCode: string;
  readonly redeemedAt: number;
  readonly remote: string;
  readonly route: RouteKind;
  readonly expiresAt: number;
  state: "pending" | "approved" | "rejected" | "expired";
  deviceId?: string;
}

export interface RedeemCtx {
  readonly remote: string;
  readonly listener: "loopback" | "remote";
  readonly route: RouteKind;
}

/** The safety code both sides show (Decision 18). */
export function safetyCode(secret: Buffer, jkt: string): string {
  const mac = createHmac("sha256", secret).update(jkt, "utf8").digest();
  return String(mac.readUInt32BE(0) % 1_000_000).padStart(6, "0");
}

/** The routes a link carries: non-loopback first by kind, capped; loopback when there is nothing else. */
export function linkRoutes(routes: ReadonlyArray<Route>): ReadonlyArray<Route> {
  const order: Record<string, number> = { public: 0, connect: 1, tailscale: 2, lan: 3, other: 4 };
  const remote = routes.filter((r) => r.kind !== "loopback").sort((a, b) => (order[a.kind] ?? 9) - (order[b.kind] ?? 9));
  return remote.length > 0 ? remote.slice(0, MAX_LINK_ROUTES) : routes.filter((r) => r.kind === "loopback");
}

const expired = () =>
  finError("PAIRING_EXPIRED", "this pairing link is no longer valid", {}, "Ask the server's owner for a new pairing link or QR code.");
const used = () =>
  finError(
    "PAIRING_USED",
    "this pairing link was already used",
    {},
    "Someone already used this link. If it was not you, tell the server's owner, and ask for a new link.",
  );
const rejected = () => finError("PAIRING_REJECTED", "the server's owner turned this pairing down", {}, "Ask the server's owner whether to pair this device.");
const invalid = (message: string, details: Record<string, unknown> = {}) =>
  finError("INVALID_REQUEST", message, details, "Change the request as the message says and try again.");
const notFound = () => finError("NOT_FOUND", "no pairing request has this id", {}, "List the pending pairing requests and use an id shown there.");

const sha256 = (b: Buffer) => createHash("sha256").update(b).digest();
const coord = (v: string) => v.length === 43 && Buffer.from(v, "base64url").toString("base64url") === v;

export class Pairing {
  readonly #d: PairingDeps;
  readonly #clock: () => number;
  readonly #offers = new Map<string, Offer>();
  readonly #pending = new Map<string, Pending>();
  readonly #bus = Effect.runSync(PubSub.sliding<PairPendingItem>(256));

  constructor(deps: PairingDeps) {
    this.#d = deps;
    this.#clock = deps.clock ?? Date.now;
    const db = deps.store.db;
    for (const r of db.prepare("SELECT * FROM pair_offers WHERE state = 'open'").all() as Array<Record<string, unknown>>) {
      this.#offers.set(String(r.id), {
        id: String(r.id),
        secretHash: Buffer.from(String(r.secretHash), "base64url"),
        label: r.label === null ? undefined : String(r.label),
        self: Number(r.self) === 1,
        createdBy: String(r.createdBy),
        createdAt: Number(r.createdAt),
        expiresAt: Number(r.expiresAt),
        state: "open",
      });
    }
    // A request pending at a restart lost its safety code (it is kept in memory only).
    db.prepare("UPDATE pair_pending SET state = 'expired' WHERE state = 'pending'").run();
    this.sweep();
  }

  #journal(payload: Record<string, unknown>): void {
    const j = this.#d.journal;
    if (j === undefined || !j.enabled) return;
    try {
      j.record("auth.pair", 1, null, JSON.stringify(payload), "finisherd");
    } catch {
      // The journal is diagnostics: a refusal never fails pairing.
    }
  }

  #audit(kind: "pair.offer" | "pair.redeem" | "pair.approve" | "pair.reject" | "pair.expire", by: { kind: string; id: string }, details: Record<string, unknown>): void {
    this.#d.audit.append({ kind, principal: by, outcome: "ok", details });
  }

  #view(p: Pending): PendingView {
    return { pendingId: p.id, deviceName: p.deviceName, platform: p.platform, safetyCode: p.safetyCode, redeemedAt: p.redeemedAt, expiresAt: p.expiresAt };
  }

  /** Writes an offer's new state when it is still open: false when another call ended it first. */
  #writeOffer(o: Offer, state: "used" | "expired"): boolean {
    return Number(this.#d.store.db.prepare("UPDATE pair_offers SET state = ? WHERE id = ? AND state = 'open'").run(state, o.id).changes) === 1;
  }

  /** In memory, after the write committed. */
  #offerEnded(o: Offer, state: "used" | "expired"): void {
    o.state = state;
    this.#offers.delete(o.id);
  }

  #expireOffer(o: Offer): void {
    if (this.#writeOffer(o, "expired")) {
      this.#audit("pair.expire", { kind: "system", id: "finisherd" }, {});
      this.#journal({ outcome: "expired" });
    }
    this.#offerEnded(o, "expired");
  }

  #writePending(p: Pending, state: "approved" | "rejected" | "expired"): void {
    this.#d.store.db.prepare("UPDATE pair_pending SET state = ? WHERE id = ?").run(state, p.id);
  }

  /** In memory and on the stream, after the write committed. */
  #pendingEnded(p: Pending, state: "approved" | "rejected" | "expired"): void {
    p.state = state;
    Effect.runSync(PubSub.publish(this.#bus, { kind: "ended", pendingId: p.id, state }));
  }

  /** Expires offers and pending requests past their time (every 10 s). */
  sweep(): void {
    const now = this.#clock();
    for (const o of [...this.#offers.values()]) if (o.expiresAt <= now) this.#expireOffer(o);
    for (const p of [...this.#pending.values()]) {
      if (p.state !== "pending" || p.expiresAt > now) continue;
      this.#writePending(p, "expired");
      this.#pendingEnded(p, "expired");
      this.#audit("pair.expire", { kind: "system", id: "finisherd" }, { deviceName: p.deviceName, address: p.remote.slice(0, 64) });
      this.#journal({ outcome: "expired", platform: p.platform, route: p.route });
    }
    // Ended requests stay a little in memory for `status`, then go (the row answers after).
    for (const p of [...this.#pending.values()]) if (p.state !== "pending" && p.expiresAt + PENDING_TTL_S * 1000 <= now) this.#pending.delete(p.id);
  }

  offer(by: Principal, req: { label?: string | undefined; ttlS?: number | undefined; self?: boolean | undefined }): {
    offerId: string;
    link: string;
    browserLink?: string;
    qr: string;
    expiresAt: number;
  } {
    if (req.self === true && by.kind !== "console") throw invalid("only the server's console makes a self pairing offer", { field: "self" });
    if (this.#d.env.state() === "moved") throw envMoved();
    const ttl = req.ttlS ?? PAIR_OFFER_DEFAULT_TTL_S;
    if (!Number.isInteger(ttl) || ttl < MIN_TTL_S || ttl > PAIR_OFFER_MAX_TTL_S) {
      throw invalid(`an offer lives ${MIN_TTL_S} to ${PAIR_OFFER_MAX_TTL_S} seconds`, { field: "ttlS" });
    }
    if (req.label !== undefined && nameProblem(req.label) !== undefined) throw invalid("a label is 1 to 64 characters without control characters", { field: "label" });
    this.sweep();
    // At most 8 open: the oldest (by creation) expires first, as the sweep would.
    const open = [...this.#offers.values()].sort((a, b) => a.createdAt - b.createdAt);
    while (open.length >= MAX_OPEN_OFFERS) this.#expireOffer(open.shift()!);
    const now = this.#clock();
    const secret = randomBytes(32);
    const offer: Offer = {
      id: ulid(now),
      secretHash: sha256(secret),
      label: req.label,
      self: req.self === true,
      createdBy: `${by.kind}:${by.id}`,
      createdAt: now,
      expiresAt: now + ttl * 1000,
      state: "open",
    };
    this.#d.store.db
      .prepare("INSERT INTO pair_offers (id, secretHash, label, self, createdBy, createdAt, expiresAt, state) VALUES (?, ?, ?, ?, ?, ?, ?, 'open')")
      .run(offer.id, offer.secretHash.toString("base64url"), offer.label ?? null, offer.self ? 1 : 0, offer.createdBy, now, offer.expiresAt);
    this.#offers.set(offer.id, offer);
    const env = this.#d.env.describe();
    const routes = offer.self ? env.routes.filter((r) => r.kind === "loopback") : linkRoutes(env.routes);
    const fragment = `${offer.id}.${secret.toString("base64url")}`;
    const r = Buffer.from(JSON.stringify(routes), "utf8").toString("base64url");
    const link = `finisher://pair?v=1&env=${env.envId}&spki=${env.spki}&r=${r}#${fragment}`;
    const webpki = routes.find((x) => x.pin === "webpki" && x.url.startsWith("https://"));
    this.#audit("pair.offer", { kind: by.kind, id: by.id }, offer.label === undefined ? {} : { label: offer.label });
    this.#journal({ outcome: "offered" });
    return {
      offerId: offer.id,
      link,
      ...(webpki === undefined ? {} : { browserLink: `${new URL(webpki.url).origin}/pair#${fragment}` }),
      qr: qrSvg(link),
      expiresAt: offer.expiresAt,
    };
  }

  async redeem(raw: unknown, ctx: RedeemCtx): Promise<{ pendingId: string; safetyCode: string }> {
    const decoded = Schema.decodeUnknownExit(PairRedeemBody)(raw, { onExcessProperty: "ignore" });
    if (decoded._tag !== "Success") throw invalid("the pairing request is not well formed");
    const body = decoded.value;
    const { name, platform, jwk } = body.device;
    checkedName(name, "device name");
    if (!coord(jwk.x) || !coord(jwk.y)) throw invalid("the device key is not a P-256 public key in canonical form", { field: "jwk" });
    const key = { kty: "EC" as const, crv: "P-256" as const, x: jwk.x, y: jwk.y };
    // The one await, before anything is looked up: from here on the redeem runs without a
    // break, so two redeems of one offer cannot both find it open (review of Task 8).
    const jkt = await calculateJwkThumbprint(key, "sha256");
    if (this.#d.env.state() === "moved") throw envMoved();
    this.sweep();
    const secret = Buffer.from(body.secret, "base64url");
    const offer = this.#offers.get(body.offerId);
    const good = offer !== undefined && secret.length === 32 && timingSafeEqual(sha256(secret), offer.secretHash);
    if (!good) {
      // Never tell an unknown offer from a wrong secret; a used one is said so to the right secret only.
      const row = this.#d.store.db.prepare("SELECT secretHash, state FROM pair_offers WHERE id = ?").get(body.offerId) as { secretHash: string; state: string } | undefined;
      if (row?.state === "used" && secret.length === 32 && timingSafeEqual(sha256(secret), Buffer.from(row.secretHash, "base64url"))) throw used();
      throw expired();
    }
    // A self offer is the desktop shell's: over loopback only, indistinguishable otherwise.
    if (offer.self && ctx.listener !== "loopback") throw expired();
    if (this.#d.devices.byJkt(jkt) !== undefined) throw invalid("this key is already a paired device's", { field: "jwk" });
    if ([...this.#pending.values()].filter((p) => p.state === "pending").length >= MAX_PENDING) {
      throw finError("PAIRING_RATE_LIMITED", "too many pairing requests wait for the owner", { reason: "pendingFull" }, "Wait for the server's owner to answer the waiting requests, then try again.");
    }
    const now = this.#clock();
    const pending: Pending = {
      id: ulid(now),
      offerId: offer.id,
      deviceName: name,
      platform,
      jwk: key,
      jkt,
      safetyCode: safetyCode(secret, jkt),
      redeemedAt: now,
      remote: ctx.remote,
      route: ctx.route,
      expiresAt: now + PENDING_TTL_S * 1000,
      state: "pending",
    };
    const approver = offer.self ? this.#consoleOf(offer) : undefined;
    let device: Device | undefined;
    // The consume, the request and (for a self offer) the approval commit together or not at all.
    try {
      this.#d.store.tx(() => {
        if (!this.#writeOffer(offer, "used")) throw used();
        this.#d.store.db
          .prepare(
            "INSERT INTO pair_pending (id, offerId, deviceName, platform, jwk, jkt, safetyCode, redeemedAt, remote, expiresAt, state) VALUES (?, ?, ?, ?, ?, ?, NULL, ?, NULL, ?, 'pending')",
          )
          .run(pending.id, offer.id, name, platform, JSON.stringify(key), jkt, now, pending.expiresAt);
        if (approver !== undefined) device = this.#approveRows(approver, pending);
      });
    } catch (e) {
      // Rolled back: an approval's device row went with it, whether or not `add` returned.
      if (approver !== undefined) this.#d.devices.forgetByJkt(jkt);
      throw e;
    }
    this.#offerEnded(offer, "used");
    this.#pending.set(pending.id, pending);
    this.#audit("pair.redeem", { kind: "anonymous", id: "" }, { deviceName: name, platform, address: ctx.remote.slice(0, 64) });
    this.#journal({ outcome: "redeemed", platform, route: ctx.route });
    if (approver !== undefined && device !== undefined) this.#approved(approver, pending, device);
    else Effect.runSync(PubSub.publish(this.#bus, { kind: "added", pending: this.#view(pending) }));
    return { pendingId: pending.id, safetyCode: pending.safetyCode };
  }

  #consoleOf(offer: Offer): Principal {
    return { kind: "console", id: offer.createdBy.slice(offer.createdBy.indexOf(":") + 1), scopes: [], projects: "*", branches: "*", human: true };
  }

  #live(pendingId: string): Pending {
    this.sweep();
    const p = this.#pending.get(pendingId);
    if (p === undefined) throw notFound();
    if (p.state === "expired") throw expired();
    if (p.state !== "pending") throw invalid(`this request was already ${p.state}`, { state: p.state });
    return p;
  }

  /** The approval's writes (inside the caller's transaction). */
  #approveRows(by: Principal, p: Pending): Device {
    const device = this.#d.devices.add({ name: p.deviceName, platform: p.platform, jwk: p.jwk, jkt: p.jkt, approvedBy: `${by.kind}:${by.id}` });
    this.#writePending(p, "approved");
    return device;
  }

  /** The approval in memory, the stream, the audit and the journal (after the commit). */
  #approved(by: Principal, p: Pending, device: Device): void {
    p.deviceId = device.id;
    this.#d.devices.announceAdded(device);
    this.#pendingEnded(p, "approved");
    this.#audit("pair.approve", { kind: by.kind, id: by.id }, { deviceName: p.deviceName, deviceId: device.id, address: p.remote.slice(0, 64) });
    this.#journal({ outcome: "approved", platform: p.platform, route: p.route, micros: (this.#clock() - p.redeemedAt) * 1000 });
  }

  approve(by: Principal, pendingId: string): Device {
    const p = this.#live(pendingId);
    let added: Device | undefined;
    try {
      this.#d.store.tx(() => {
        added = this.#approveRows(by, p);
      });
    } catch (e) {
      // Rolled back: the device's row went with it, so memory forgets it too (by its key: the
      // write that failed may have come after `add` and before it returned).
      this.#d.devices.forgetByJkt(p.jkt);
      throw e;
    }
    this.#approved(by, p, added!);
    return added!;
  }

  reject(by: Principal, pendingId: string): void {
    const p = this.#live(pendingId);
    this.#writePending(p, "rejected");
    this.#pendingEnded(p, "rejected");
    this.#audit("pair.reject", { kind: by.kind, id: by.id }, { deviceName: p.deviceName, address: p.remote.slice(0, 64) });
    this.#journal({ outcome: "rejected", platform: p.platform, route: p.route, micros: (this.#clock() - p.redeemedAt) * 1000 });
  }

  /**
   * Whether a DPoP proof by `jkt` may be remembered on `/pair/status`: a request's key still in
   * memory, or a device's (an approval the device learns of after a restart).
   */
  isPendingKey(jkt: string): boolean {
    for (const p of this.#pending.values()) if (p.jkt === jkt) return true;
    return this.#d.devices.byJkt(jkt) !== undefined;
  }

  /** `POST /pair/status` by the request's own key (the caller checked the proof). */
  status(pendingId: string, jkt: string): { state: "pending" | "approved" | "expired"; deviceId?: string } {
    this.sweep();
    const p = this.#pending.get(pendingId);
    if (p !== undefined) {
      // Another key's request is as good as none.
      if (p.jkt !== jkt) throw notFound();
      if (p.state === "rejected") throw rejected();
      if (p.state === "approved") return { state: "approved", deviceId: p.deviceId! };
      return { state: p.state };
    }
    // Gone from memory (a restart, or long ended): the row answers.
    const row = this.#d.store.db.prepare("SELECT jkt, state FROM pair_pending WHERE id = ?").get(pendingId) as { jkt: string; state: string } | undefined;
    if (row === undefined || row.jkt !== jkt) throw notFound();
    if (row.state === "rejected") throw rejected();
    const device = row.state === "approved" ? this.#d.devices.byJkt(jkt) : undefined;
    return device === undefined ? { state: "expired" } : { state: "approved", deviceId: device.id };
  }

  /** The pending requests, the snapshot first, then each one added or ended; never an address. */
  pending(): Stream.Stream<PairPendingItem> {
    const self = this;
    return Stream.unwrap(
      Effect.gen(function* () {
        const live = yield* PubSub.subscribe(self.#bus);
        self.sweep();
        const snapshot: PairPendingItem = { kind: "snapshot", pending: [...self.#pending.values()].filter((p) => p.state === "pending").map((p) => self.#view(p)) };
        return Stream.concat(Stream.make(snapshot), Stream.fromSubscription(live));
      }),
    );
  }

  /** Open offers now (tests). */
  openOffers(): number {
    return this.#offers.size;
  }
}

export class PairingTag extends Context.Service<PairingTag, Pairing>()("finisherd/Pairing") {}
