// Agent and MCP credentials (spec §4.1, §4.5; plan 5A Task 9, Decisions 32–34).
//
// - **Stored without a key:** a row of `control.db` `credentials` (chain `auth`); the key is
//   `credentialKey(credential.master, kind, id)`, recomputed for every handshake, so a copy of
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
  readonly #master: Buffer;
  readonly #audit: Audit;
  readonly #clock: () => number;
  readonly #rows = new Map<string, CredentialRow>();
  /** The `lastUsedAt` last written, per id. */
  readonly #written = new Map<string, number>();
  readonly #insert: StatementSync;
  readonly #touch: StatementSync;
  readonly #end: StatementSync;

  constructor(store: ControlStore, master: Buffer, audit: Audit, clock: () => number = Date.now) {
    this.#store = store;
    this.#master = master;
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
    const label = req.label.trim();
    if (label.length === 0 || label.length > CREDENTIAL_LABEL_MAX || CONTROL.test(label)) {
      throw invalid(`the label must be 1 to ${CREDENTIAL_LABEL_MAX} characters, without control characters`, { field: "label" });
    }
    const scopes = [...new Set(req.scopes ?? MINT_DEFAULT_SCOPES)];
    const human = scopes.filter((s) => HUMAN_ONLY.has(s));
    if (human.length > 0) {
      const e = scopeDenied("auth.credential.mint", human, by);
      throw finError(
        "SCOPE_DENIED",
        `a credential cannot hold ${human.join(", ")}: only a person can`,
        { tag: "auth.credential.mint", scopes: human },
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
      id: ulid(now),
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
    const key = credentialKey(this.#master, row.kind, row.id);
    return { credential: this.view(row), file: { v: 1, kind: row.kind, id: row.id, key: b64u.encode(key), env: this.#store.envId } };
  }

  /** The key and principal of a live credential, for the handshake; `undefined` otherwise. */
  lookup(ref: CredentialRef): { key: Buffer; principal: Principal } | undefined {
    const row = this.#live(ref.kind, ref.id);
    if (row === undefined) return undefined;
    return { key: credentialKey(this.#master, row.kind, row.id), principal: this.principal(row) };
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
   * Ends `ref` in the store (`revoke.ts` calls it first, then drops its tickets and
   * connections); true when it was not ended yet (an expired one not yet swept included).
   */
  end(ref: CredentialRef, reason: EndReason): boolean {
    const row = this.#rows.get(ref.id);
    if (row === undefined || row.kind !== ref.kind || row.revokedAt !== undefined) return false;
    const now = this.#clock();
    // An owner or self revocation of one that has already expired is recorded as its expiry.
    const why: EndReason = reason !== "expired" && this.#status(row, now) === "expired" ? "expired" : reason;
    this.#end.run(now, why, row.id);
    row.revokedAt = now;
    row.revokedReason = why;
    return true;
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
