// Agent and MCP credentials (spec §4.1, §4.5; plan 5A Task 9, Decisions 32–34).
//
// - **Stored without a key:** a row of `control.db` `credentials` (chain `auth`); the key is
//   `credentialKey(credential.master, envId, kind, id)`, recomputed for every handshake, so a copy of
//   `control.db` holds no secret.
// - **Live** means not revoked and not expired: idle past `idleExpiryS` since it was last used
//   (or minted), or past `absoluteExpiryAt`. Only a live credential gets a hello proof
//   (`lookup`), and every request re-checks (`isLive`, through the connection registry), so an
//   expired or revoked one runs nothing more; the expiry sweep (`expired`, every 60 s) ends its
//   connections and streams through the one revoke path (`revoke.ts`).
// - **Use** (`touch`) is kept in memory on every request and written at most once a minute.
// - Rows load once at start (a few hundred at most: sub-project 7 mints one per session) and
//   rows ended over 180 days ago are deleted then.
// Keys never leave this module but in a mint's answer, which is the credential file itself.
import type { Credential, CredentialFile, CredentialKind, Principal, Scope } from "@finisher/contracts";
import { CREDENTIAL_EXPIRY, CREDENTIAL_LABEL_MAX, CREDENTIAL_MAX_EXPIRY_S, CREDENTIAL_MIN_EXPIRY_S, HUMAN_ONLY, MINT_DEFAULT_SCOPES } from "@finisher/contracts";
import type { StatementSync } from "node:sqlite";
import { Context } from "effect";
import { humanOnly, scopeDenied } from "../chain/fixes.ts";
import type { Audit } from "../control/audit.ts";
import type { ControlStore } from "../control/db.ts";
import { registerChain } from "../control/migrations.ts";
import { auth0001 } from "../control/migrations/auth/0001.ts";
import { ulid } from "../control/ulid.ts";
import { finError } from "../errors.ts";
import { b64u, credentialKey } from "./crypto.ts";

registerChain("auth", [auth0001], { formatVersion: 1, minReaderVersion: 1 });

/** `lastUsedAt` is written at most this often (Decision 32). */
export const TOUCH_WRITE_MS = 60_000;
/** Ended rows are deleted this long after they ended. */
export const ENDED_RETENTION_MS = 180 * 86_400_000;
/** Live credentials at most (a mint past it is refused). */
export const MAX_LIVE_CREDENTIALS = 4096;

export type EndReason = "owner" | "self" | "expired";

export interface CredentialRow {
  readonly id: string;
  readonly kind: CredentialKind;
  readonly label: string;
  readonly scopes: ReadonlyArray<Scope>;
  readonly projects: "*" | ReadonlyArray<string>;
  readonly branches: "*" | ReadonlyArray<string>;
  readonly createdBy: string;
  readonly createdAt: number;
  lastUsedAt?: number;
  readonly idleExpiryS: number;
  readonly absoluteExpiryAt?: number;
  revokedAt?: number;
  revokedReason?: EndReason;
}

export interface MintRequest {
  readonly kind: CredentialKind;
  readonly label: string;
  readonly scopes?: ReadonlyArray<Scope> | undefined;
  readonly projects?: ReadonlyArray<string> | undefined;
  readonly branches?: ReadonlyArray<string> | undefined;
  readonly idleS?: number | undefined;
  readonly absoluteS?: number | null | undefined;
}

export interface CredentialRef {
  readonly kind: "agent" | "mcp";
  readonly id: string;
}

const PROJECT_ID = /^[0-9A-Za-z_-]{1,64}$/;
/** A credential id: a ULID's 26 characters. */
const CREDENTIAL_ID = /^[0-9A-Za-z]{26}$/;
/** A branch id or name as `fin` accepts them, or `own`: no space or control character. */
const BRANCH_RULE = /^[^\s\p{Cc}]{1,128}$/u;
const CONTROL = /\p{Cc}/u;

const invalid = (message: string, details: Record<string, unknown> = {}) =>
  finError("INVALID_REQUEST", message, details, "Change the request as the message says and try again.");

function seconds(field: string, v: number): number {
  if (!Number.isSafeInteger(v) || v < CREDENTIAL_MIN_EXPIRY_S || v > CREDENTIAL_MAX_EXPIRY_S) {
    throw invalid(`${field} must be a whole number of seconds from ${CREDENTIAL_MIN_EXPIRY_S} to ${CREDENTIAL_MAX_EXPIRY_S}`, { field });
  }
  return v;
}

function list<T extends string>(field: string, v: ReadonlyArray<T> | undefined, ok: RegExp): "*" | ReadonlyArray<T> {
  if (v === undefined) return "*";
  if (v.length === 0) throw invalid(`${field} names nothing: leave it out to allow every one`, { field });
  for (const x of v) if (!ok.test(x)) throw invalid(`${field} has an entry that is not an id`, { field });
  return [...new Set(v)];
}

const parseList = (text: string): "*" | ReadonlyArray<string> => JSON.parse(text) as "*" | ReadonlyArray<string>;

export class Credentials {
  readonly #store: ControlStore;
  #master: Buffer;
  readonly #saveMaster: ((key: Buffer) => void) | undefined;
  readonly #audit: Audit;
  readonly #clock: () => number;
  readonly #rows = new Map<string, CredentialRow>();
  /** The `lastUsedAt` last written, per id. */
  readonly #written = new Map<string, number>();
  readonly #insert: StatementSync;
  readonly #touch: StatementSync;
  readonly #end: StatementSync;
  /** Rows ended in memory and not yet written (`persist`). */
  readonly #unsaved = new Set<string>();
  #retry: ReturnType<typeof setTimeout> | undefined;
  #backoffMs = 500;

  /** `saveMaster` writes a new `credential.master` durably (`rotateMaster`; none in unit tests). */
  constructor(store: ControlStore, master: Buffer, audit: Audit, clock: () => number = Date.now, saveMaster?: (key: Buffer) => void) {
    this.#store = store;
    this.#master = master;
    this.#saveMaster = saveMaster;
    this.#audit = audit;
    this.#clock = clock;
    const db = store.db;
    const cutoff = clock() - ENDED_RETENTION_MS;
    db.prepare("DELETE FROM credentials WHERE revokedAt IS NOT NULL AND revokedAt < ?").run(cutoff);
    for (const r of db.prepare("SELECT * FROM credentials").all() as Array<Record<string, unknown>>) {
      const row: CredentialRow = {
        id: String(r.id),
        kind: r.kind as CredentialKind,
        label: String(r.label),
        scopes: JSON.parse(String(r.scopes)) as Scope[],
        projects: parseList(String(r.projects)),
        branches: parseList(String(r.branches)),
        createdBy: String(r.createdBy),
        createdAt: Number(r.createdAt),
        idleExpiryS: Number(r.idleExpiryS),
        ...(r.lastUsedAt === null ? {} : { lastUsedAt: Number(r.lastUsedAt) }),
        ...(r.absoluteExpiryAt === null ? {} : { absoluteExpiryAt: Number(r.absoluteExpiryAt) }),
        ...(r.revokedAt === null ? {} : { revokedAt: Number(r.revokedAt), revokedReason: r.revokedReason as EndReason }),
      };
      this.#rows.set(row.id, row);
      if (row.lastUsedAt !== undefined) this.#written.set(row.id, row.lastUsedAt);
    }
    this.#insert = db.prepare(
      "INSERT INTO credentials (id, kind, label, scopes, projects, branches, createdBy, createdAt, lastUsedAt, idleExpiryS, absoluteExpiryAt) " +
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?)",
    );
    this.#touch = db.prepare("UPDATE credentials SET lastUsedAt = ? WHERE id = ?");
    this.#end = db.prepare("UPDATE credentials SET revokedAt = ?, revokedReason = ? WHERE id = ? AND revokedAt IS NULL");
  }

  /** `live`, `revoked` or `expired` at `now`. */
  #status(row: CredentialRow, now: number): Credential["status"] {
    if (row.revokedAt !== undefined) return row.revokedReason === "expired" ? "expired" : "revoked";
    if (row.absoluteExpiryAt !== undefined && now >= row.absoluteExpiryAt) return "expired";
    if (now >= (row.lastUsedAt ?? row.createdAt) + row.idleExpiryS * 1000) return "expired";
    return "live";
  }

  #live(kind: string, id: string): CredentialRow | undefined {
    const row = this.#rows.get(id);
    return row !== undefined && row.kind === kind && this.#status(row, this.#clock()) === "live" ? row : undefined;
  }

  /** The owner's view of a row: never its key. */
  view(row: CredentialRow): Credential {
    return {
      id: row.id,
      kind: row.kind,
      label: row.label,
      scopes: [...row.scopes],
      projects: row.projects === "*" ? "*" : [...row.projects],
      branches: row.branches === "*" ? "*" : [...row.branches],
      createdBy: row.createdBy,
      createdAt: row.createdAt,
      idleExpiryS: row.idleExpiryS,
      status: this.#status(row, this.#clock()),
      ...(row.lastUsedAt === undefined ? {} : { lastUsedAt: row.lastUsedAt }),
      ...(row.absoluteExpiryAt === undefined ? {} : { absoluteExpiryAt: row.absoluteExpiryAt }),
      ...(row.revokedAt === undefined ? {} : { revokedAt: row.revokedAt }),
    };
  }

  /**
   * Mints a credential for `by` (a person: the console). Scopes default to the mint defaults
   * and never include a human-only one (`SCOPE_DENIED` naming them).
   */
  mint(by: Principal, req: MintRequest): { credential: Credential; file: CredentialFile } {
    if (!by.human) throw humanOnly("auth.credential.mint", "auth.admin", by);
    return this.#mint(by, req, ulid(this.#clock()), "auth.credential.mint");
  }

  /**
   * Mints the `agent` credential of an agent session for the server itself (sub-project 7A
   * Decision 15.3, spec §1.7 item 1): its id is the caller's (`id`, the session id), and `by`
   * (the session's starter, who may be `local`) is recorded as its maker. Not an RPC: only the
   * agent runtime calls it, with the scopes of the session's mode. A human-only scope is still
   * `SCOPE_DENIED`, and an id already used is `INVALID_REQUEST`.
   */
  mintWithId(by: Principal, id: string, req: MintRequest): { credential: Credential; file: CredentialFile } {
    if (!CREDENTIAL_ID.test(id)) throw invalid("a credential id is 26 letters and digits", { field: "id" });
    if (this.#rows.has(id)) throw invalid("a credential with this id exists", { field: "id" });
    return this.#mint(by, req, id, "agent.session.start");
  }

  #mint(by: Principal, req: MintRequest, id: string, tag: string): { credential: Credential; file: CredentialFile } {
    const label = req.label.trim();
    if (label.length === 0 || label.length > CREDENTIAL_LABEL_MAX || CONTROL.test(label)) {
      throw invalid(`the label must be 1 to ${CREDENTIAL_LABEL_MAX} characters, without control characters`, { field: "label" });
    }
    const scopes = [...new Set(req.scopes ?? MINT_DEFAULT_SCOPES)];
    const human = scopes.filter((s) => HUMAN_ONLY.has(s));
    if (human.length > 0) {
      const e = scopeDenied(tag, human, by);
      throw finError(
        "SCOPE_DENIED",
        `a credential cannot hold ${human.join(", ")}: only a person can`,
        { tag, scopes: human },
        e.fix,
      );
    }
    const projects = list("projects", req.projects, PROJECT_ID);
    const branches = list("branches", req.branches, BRANCH_RULE);
    const defaults = CREDENTIAL_EXPIRY[req.kind];
    const idleExpiryS = seconds("idleS", req.idleS ?? defaults.idleS);
    const absoluteS = req.absoluteS === undefined ? defaults.absoluteS : req.absoluteS === null ? null : seconds("absoluteS", req.absoluteS);
    const now = this.#clock();
    let live = 0;
    for (const r of this.#rows.values()) if (this.#status(r, now) === "live") live++;
    if (live >= MAX_LIVE_CREDENTIALS) {
      throw finError("CREDENTIAL_LIMIT", `this server holds ${MAX_LIVE_CREDENTIALS} live credentials`, { max: MAX_LIVE_CREDENTIALS },
        "Revoke credentials you no longer use, then mint this one again.");
    }
    const row: CredentialRow = {
      id,
      kind: req.kind,
      label,
      scopes,
      projects,
      branches,
      createdBy: `${by.kind}:${by.id}`,
      createdAt: now,
      idleExpiryS,
      ...(absoluteS === null ? {} : { absoluteExpiryAt: now + absoluteS * 1000 }),
    };
    this.#insert.run(row.id, row.kind, row.label, JSON.stringify(scopes), JSON.stringify(projects), JSON.stringify(branches),
      row.createdBy, now, idleExpiryS, row.absoluteExpiryAt ?? null);
    this.#rows.set(row.id, row);
    this.#audit.append({
      kind: "credential.mint",
      principal: { kind: by.kind, id: by.id },
      credentialKind: row.kind,
      outcome: "ok",
      details: { credentialId: row.id, scopes: [...scopes] },
    });
    const key = credentialKey(this.#master, this.#store.envId, row.kind, row.id);
    return { credential: this.view(row), file: { v: 1, kind: row.kind, id: row.id, key: b64u.encode(key), env: this.#store.envId } };
  }

  /** The key and principal of a live credential, for the handshake; `undefined` otherwise. */
  lookup(ref: CredentialRef): { key: Buffer; principal: Principal } | undefined {
    const row = this.#live(ref.kind, ref.id);
    if (row === undefined) return undefined;
    return { key: credentialKey(this.#master, this.#store.envId, row.kind, row.id), principal: this.principal(row) };
  }

  /**
   * What the handshake answers for `ref` (review of Tasks 8–9, L4): a live credential's key and
   * principal; for one that exists but has ended, its key too and why it ended, so the server
   * proves itself and then refuses it (`CREDENTIAL_REVOKED` or `CREDENTIAL_EXPIRED`) instead
   * of looking like a squatter. An unknown id (or a row deleted 180 days after it ended) gets
   * nothing, so ids still cannot be probed.
   */
  forHello(ref: CredentialRef): { key: Buffer; principal: Principal; ended?: "revoked" | "expired" } | undefined {
    const row = this.#rows.get(ref.id);
    if (row === undefined || row.kind !== ref.kind) return undefined;
    const status = this.#status(row, this.#clock());
    return {
      key: credentialKey(this.#master, this.#store.envId, row.kind, row.id),
      principal: this.principal(row),
      ...(status === "live" ? {} : { ended: status }),
    };
  }

  keyFor(kind: CredentialKind, id: string): Buffer | undefined {
    return this.lookup({ kind, id })?.key;
  }

  principal(row: CredentialRow): Principal {
    return {
      kind: row.kind,
      id: row.id,
      scopes: [...row.scopes],
      projects: row.projects === "*" ? "*" : [...row.projects],
      branches: row.branches === "*" ? "*" : [...row.branches],
      human: false,
    };
  }

  /** Whether `p` (an `agent`/`mcp` principal) still runs: its credential is live. */
  isLive(p: { readonly kind: string; readonly id: string }): boolean {
    return this.#live(p.kind, p.id) !== undefined;
  }

  /**
   * Restarts a live credential's idle expiry now and writes it at once (sub-project 7A
   * Decision 18: a resumed session keeps its credential). An ended credential is not brought
   * back: `CREDENTIAL_REVOKED` or `CREDENTIAL_EXPIRED`; an unknown id is `CREDENTIAL_REVOKED`.
   */
  refresh(id: string): Credential {
    const row = this.#rows.get(id);
    const status = row === undefined ? "revoked" : this.#status(row, this.#clock());
    if (row === undefined || status !== "live") {
      throw status === "expired"
        ? finError("CREDENTIAL_EXPIRED", "this credential expired", { id }, "Start a new session: an ended one cannot be resumed.")
        : finError("CREDENTIAL_REVOKED", "this credential was revoked", { id }, "Start a new session: an ended one cannot be resumed.");
    }
    const now = this.#clock();
    row.lastUsedAt = now;
    this.#touch.run(now, id);
    this.#written.set(id, now);
    return this.view(row);
  }

  /** The credential `id`, live or ended, as the owner sees it. */
  get(id: string): Credential | undefined {
    const row = this.#rows.get(id);
    return row === undefined ? undefined : this.view(row);
  }

  /** Notes a use of `id` now; written at most once a minute. */
  touch(id: string): void {
    const row = this.#rows.get(id);
    if (row === undefined || row.revokedAt !== undefined) return;
    const now = this.#clock();
    row.lastUsedAt = now;
    if (now - (this.#written.get(id) ?? Number.NEGATIVE_INFINITY) < TOUCH_WRITE_MS) return;
    this.#written.set(id, now);
    try {
      this.#touch.run(now, id);
    } catch (e) {
      // A busy store must not fail the request; the next minute's use writes it.
      this.#written.delete(id);
      process.stderr.write(`finisherd: warning: cannot record a credential's use: ${String(e)}\n`);
    }
  }

  /**
   * Ends `ref` in memory (`revoke.ts` step 1, review of Tasks 8–9, L2): from now on no
   * handshake proves it and no request of it runs. Writing it to the store is `persist`'s,
   * which the revoke path calls last, so a busy store never stops a revocation. True when it
   * was not ended yet (an expired one not yet swept included).
   */
  markEnded(ref: CredentialRef, reason: EndReason): boolean {
    const row = this.#rows.get(ref.id);
    if (row === undefined || row.kind !== ref.kind || row.revokedAt !== undefined) return false;
    const now = this.#clock();
    // An owner or self revocation of one that has already expired is recorded as its expiry.
    const why: EndReason = reason !== "expired" && this.#status(row, now) === "expired" ? "expired" : reason;
    row.revokedAt = now;
    row.revokedReason = why;
    this.#unsaved.add(row.id);
    return true;
  }

  /**
   * Writes every ended row not saved yet. A failure (a busy store) is retried after 1 s,
   * doubling up to a minute, until it lands; meanwhile the row is ended in memory. Until then a
   * restart would bring the credential back (TASKS records it).
   */
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
        process.stderr.write(`finisherd: warning: cannot record a credential's end yet (retrying): ${String(e)}\n`);
        this.#backoffMs = Math.min(this.#backoffMs * 2, 60_000);
        this.#retry = setTimeout(() => this.persist(), this.#backoffMs);
        this.#retry.unref();
        return;
      }
    }
    this.#backoffMs = 500;
  }

  /** Ends `ref` and writes it at once (`markEnded`, then `persist`). */
  end(ref: CredentialRef, reason: EndReason): boolean {
    const ended = this.markEnded(ref, reason);
    if (ended) this.persist();
    return ended;
  }

  /** Stops the retry timer (the server's shutdown; the control store closes after). */
  dispose(): void {
    if (this.#retry !== undefined) clearTimeout(this.#retry);
    this.#retry = undefined;
  }

  /** Ended rows not written yet (tests). */
  unsaved(): number {
    return this.#unsaved.size;
  }

  /** Whether `id` was ended in memory and its end is not written yet (a restart would bring it back). */
  endUnsaved(id: string): boolean {
    return this.#unsaved.has(id);
  }

  /**
   * Replaces `credential.master` with `master` (review of Tasks 8–9, M3): written first (a
   * failure changes nothing), then used for every key from now on, so no credential file made
   * before proves again. Answers the credentials not ended yet, which the caller revokes
   * through the one revoke path so their connections and writes in flight end too.
   */
  rotateMaster(master: Buffer): ReadonlyArray<CredentialRef> {
    if (this.#saveMaster === undefined) throw finError("INTERNAL", "this server cannot write credential.master", {});
    this.#saveMaster(master);
    this.#master = master;
    return [...this.#rows.values()].filter((r) => r.revokedAt === undefined).map((r) => ({ kind: r.kind, id: r.id }));
  }

  /** The kind of credential `id`, when there is one. */
  kindOf(id: string): CredentialKind | undefined {
    return this.#rows.get(id)?.kind;
  }

  /** Credentials past their expiry that were not ended yet (the sweep ends them). */
  expired(): ReadonlyArray<CredentialRef> {
    const now = this.#clock();
    return [...this.#rows.values()].filter((r) => r.revokedAt === undefined && this.#status(r, now) === "expired").map((r) => ({ kind: r.kind, id: r.id }));
  }

  /** Every live credential, or every one kept (`all`), newest first. */
  list(all = false): ReadonlyArray<Credential> {
    const now = this.#clock();
    return [...this.#rows.values()]
      .filter((r) => all || this.#status(r, now) === "live")
      .sort((a, b) => b.createdAt - a.createdAt || (a.id < b.id ? 1 : -1))
      .map((r) => this.view(r));
  }
}

export class CredentialsTag extends Context.Service<CredentialsTag, Credentials>()("finisherd/Credentials") {}
