// A hook's `fin` credential (7F plan Task 5; Decision 11, Owner question 6; spec §9.2: "a
// `plan`-scoped agent credential of its own").
//
// - **What it is:** an `agent` credential minted by the server (`Credentials.mintWithId`, maker
//   `local:hooks`: `mint` is people's only) with `plan`'s scopes (`MODE_SCOPES.plan`),
//   `projects: [the event's project]`, `branches: ["main"]` (no write scope, so branch limits
//   never apply), label `hook <layer>:<id>` (cut at the label limit), the minimum idle expiry
//   (`CREDENTIAL_MIN_EXPIRY_S`) and no absolute one. It is not a session's: `fin` finds no
//   session for it (`agent.self` answers `SESSION_NOT_FOUND`) and the chain treats it as a
//   sessionless `agent` credential.
// - **One per (hook, project)**, minted on a hook's first run in a project (after the run's
//   re-check), its file `<server-data>/hooks/creds/<hash>.json` (0600 from creation, renamed into
//   place), `<hash>` of the layer, the hook's id and the project. Each later run renews it
//   (`touch`); one that expired meanwhile (a minute without use: the expiry sweep ends it) is
//   minted again.
// - **Ended through 5A's one revoke path** (`revokeCredential`, audited `credential.revoke`), its
//   file removed: when the hook's configuration changes (its key: the layer, source, spec as
//   written, `run`, program, `file` and bound hashes) or disappears (a resolution of its project,
//   or of the server layers, no longer holds it with `fin`), at server stop (`close`), and at
//   start for any left over (`sweep`: every live credential whose label starts `hook `, and
//   every file in `hooks/creds/`).
// - **The recursion guard** (`isHook`): the ids of the hook credentials held now; the engine
//   runs no hook for an event whose principal is one.
import { createHash } from "node:crypto";
import { mkdirSync, readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import { CREDENTIAL_LABEL_MAX, CREDENTIAL_MIN_EXPIRY_S, type HookLayer, type Principal } from "@finisher/contracts";
import type { Credentials } from "../auth/credentials.ts";
import { type RevokeDeps, revokeCredential } from "../auth/revoke.ts";
import { MODE_SCOPES, writeCredentialFile } from "../agent/credentials.ts";
import { ulid } from "../control/ulid.ts";
import { LOCAL } from "../principal.ts";
import type { ConfiguredHook } from "./config.ts";

/** Every hook credential's label starts with it (`sweep` finds leftovers by it). */
export const HOOK_LABEL_PREFIX = "hook ";
/** The maker recorded on a hook credential (`createdBy`, the mint's audit row). */
const MINTER: Principal = { ...LOCAL, id: "hooks" };
/** Layers whose hooks are the same in every project (the others are one project's). */
const SERVER_LAYERS: ReadonlySet<HookLayer> = new Set(["managed", "user", "plugin"]);

/** What the engine asks of the credentials. */
export interface HookCredentialPort {
  /** The credential file of `h` in `projectId`: minted on first use, else renewed. */
  forRun(h: ConfiguredHook, projectId: string): string;
  /** Whether `p` is a hook credential held now (the recursion guard). */
  isHook(p: Pick<Principal, "kind" | "id">): boolean;
  /** Ends the credentials whose hook a resolution (of `projectId`, or of the server layers alone) no longer holds as it was. */
  retain(projectId: string | undefined, hooks: readonly ConfiguredHook[]): void;
}

export interface HookCredentialDeps {
  readonly credentials: Credentials;
  /** 5A's revoke path (`revoke` marks the credential ended by its owner). */
  readonly revoke: RevokeDeps;
  /** Server data, links resolved (the launcher's policy reads the file there). */
  readonly serverData: string;
  readonly log?: (message: string) => void;
}

interface Held {
  readonly id: string;
  /** The hook's configuration key when it was minted. */
  readonly key: string;
  readonly layer: HookLayer;
  readonly hook: string;
  readonly projectId: string;
  readonly path: string;
}

const sha = (s: string) => createHash("sha256").update(s).digest("hex");

/** A hook's configuration as its credential is bound to it: any change ends the credential. */
export function hookKey(h: ConfiguredHook): string {
  return sha(JSON.stringify([h.layer, h.source, h.spec ?? null, h.run ?? null, h.program ?? null, h.file ?? null, h.bound ?? null]));
}

const declaresFin = (h: ConfiguredHook) => h.spec?.fin === true && h.run !== undefined && h.unreadable !== true;

export class HookCredentials implements HookCredentialPort {
  readonly #d: HookCredentialDeps;
  readonly #dir: string;
  /** By `<layer>\0<id>\0<project>`. */
  readonly #held = new Map<string, Held>();
  readonly #ids = new Set<string>();

  constructor(d: HookCredentialDeps) {
    this.#d = d;
    this.#dir = join(d.serverData, "hooks", "creds");
  }

  /** At start, before any hook runs: ends every live hook credential left over and removes every file. */
  sweep(): number {
    let n = 0;
    for (const c of this.#d.credentials.list()) {
      if (c.kind !== "agent" || !c.label.startsWith(HOOK_LABEL_PREFIX) || this.#ids.has(c.id)) continue;
      if (this.#revoke(c.id)) n++;
    }
    let names: string[] = [];
    try {
      names = readdirSync(this.#dir);
    } catch {
      // None yet.
    }
    for (const name of names) rmSync(join(this.#dir, name), { force: true, recursive: true });
    return n;
  }

  forRun(h: ConfiguredHook, projectId: string): string {
    const slot = `${h.layer}\0${h.id}\0${projectId}`;
    const key = hookKey(h);
    const held = this.#held.get(slot);
    if (held !== undefined) {
      if (held.key === key && this.#d.credentials.isLive({ kind: "agent", id: held.id })) {
        this.#d.credentials.touch(held.id);
        return held.path;
      }
      this.#end(slot, held);
    }
    mkdirSync(this.#dir, { recursive: true, mode: 0o700 });
    const label = `${HOOK_LABEL_PREFIX}${h.layer}:${h.id}`.slice(0, CREDENTIAL_LABEL_MAX);
    const { credential, file } = this.#d.credentials.mintWithId(MINTER, ulid(), {
      kind: "agent",
      label,
      scopes: MODE_SCOPES.plan,
      projects: [projectId],
      branches: ["main"],
      idleS: CREDENTIAL_MIN_EXPIRY_S,
      absoluteS: null,
    });
    const path = join(this.#dir, `${sha(slot).slice(0, 32)}.json`);
    try {
      writeCredentialFile(path, file);
    } catch (e) {
      // A credential no file holds is no use: ended at once.
      this.#revoke(credential.id);
      throw e;
    }
    this.#held.set(slot, { id: credential.id, key, layer: h.layer, hook: h.id, projectId, path });
    this.#ids.add(credential.id);
    return path;
  }

  isHook(p: Pick<Principal, "kind" | "id">): boolean {
    return p.kind === "agent" && this.#ids.has(p.id);
  }

  retain(projectId: string | undefined, hooks: readonly ConfiguredHook[]): void {
    if (this.#held.size === 0) return;
    const present = new Map<string, string>();
    for (const h of hooks) if (declaresFin(h)) present.set(`${h.layer}\0${h.id}`, hookKey(h));
    for (const [slot, held] of this.#held) {
      // A project's layers are seen only in its own resolution; the server's in every one.
      const seen = SERVER_LAYERS.has(held.layer) || held.projectId === projectId;
      if (seen && present.get(`${held.layer}\0${held.hook}`) !== held.key) this.#end(slot, held);
    }
  }

  /** At server stop: ends every credential held. */
  close(): void {
    for (const [slot, held] of [...this.#held]) this.#end(slot, held);
  }

  #end(slot: string, held: Held): void {
    this.#held.delete(slot);
    this.#ids.delete(held.id);
    rmSync(held.path, { force: true });
    this.#revoke(held.id);
  }

  /** 5A's revoke path; a failure is logged (the expiry sweep ends it within a minute anyway). */
  #revoke(id: string): boolean {
    try {
      return revokeCredential(this.#d.revoke, { kind: "agent", id }, { rpc: "hooks", reason: "owner" }).revoked;
    } catch (e) {
      this.#d.log?.(`hooks: a hook credential was not revoked: ${e instanceof Error ? e.message : String(e)}`);
      return false;
    }
  }
}
