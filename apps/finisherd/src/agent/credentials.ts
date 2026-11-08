// An agent session's branch and credential (sub-project 7A Task 7, Decisions 15 and 18; spec
// §5.1, §5.2, §6.1):
// - **Open** (`openSession`, steps 2–5 of Decision 15; step 1, resolving the role, model and
//   provider, and the first run are the runtime's): unless `plan`, the root branch
//   `agent.<role>-<n>` is created from `base` (default `main`) at its head, as the starter;
//   the session is recorded; its `agent` credential is minted **with the session's id**, the
//   scopes of its mode, `projects: [projectId]` and `branches: [root]` (its subtree is implied:
//   `reach.ts`); the credential file is written to `<server-data>/agents/<id>/credential.json`
//   (0600 from creation, then renamed into place); the session becomes `idle`.
// - **End** revokes the credential through 5A's one revoke path; **resume** refreshes its
//   expiry; both reach this module as `session_outbox` items (`handle`), at least once, so
//   handling one twice changes nothing.
// - **One live session per root branch:** `BRANCH_IN_USE` (exit 4).
import { renameSync, rmSync } from "node:fs";
import { join } from "node:path";
import type { BranchCreated, BranchView } from "@finisher/core-types";
import type { BudgetSpec, CredentialFile, FinError, Mode, Principal, Scope, Session } from "@finisher/contracts";
import { HUMAN_ONLY } from "@finisher/contracts";
import { createPrivateFile } from "@finisher/fin-node";
import { Effect } from "effect";
import type { Credentials } from "../auth/credentials.ts";
import { type RevokeDeps, revokeCredential } from "../auth/revoke.ts";
import type { CallMeta } from "../diagnostics.ts";
import { finError, toFinError } from "../errors.ts";
import type { HistoryPayload } from "../history.ts";
import type { Ids } from "./clock.ts";
import { sessionAuthor } from "./reach.ts";
import type { AgentStore, NewSession, OutboxItem } from "./store.ts";

/**
 * The scopes of each mode (spec §6.1), as far as 7A has them: `jobs.submit` waits for 5C and
 * `review.write` for 5E, `auto`'s render and critic jobs for 5C and sub-project 10, and a
 * parent's `agent.run` for its children for 7B. No mode holds `project.create`, a human-only
 * scope, `branch.protected`, `review.decide`, `jobs.control`, `project.admin` or `fonts.fetch`;
 * `project.open` is refused to sessions by the chain (`reach.ts`). `diagnostics.record` lets
 * the session's `fin` record its own diagnostics, as every minted credential's default does.
 */
export const MODE_SCOPES: { readonly [M in Mode]: ReadonlyArray<Scope> } = (() => {
  const plan: ReadonlyArray<Scope> = ["server.read", "project.read", "library.read", "jobs.read", "diagnostics.read", "diagnostics.record"];
  const draft: ReadonlyArray<Scope> = [...plan, "project.write"];
  return { plan, draft, auto: draft };
})();

/** Scopes no mode may hold, beside every human-only one (spec §6.1). */
export const NEVER_SESSION_SCOPES: ReadonlySet<Scope> = new Set<Scope>([
  ...HUMAN_ONLY,
  "project.create",
  "project.admin",
  "branch.protected",
  "review.decide",
  "jobs.control",
  "fonts.fetch",
]);

/**
 * A session's credential outlasts it (Decision 15.3): it ends with the session, so it has no
 * absolute limit, and an idle month ends it (a resume restarts the month).
 */
export const SESSION_IDLE_S = 30 * 86_400;

const ROLE = /^[a-z][a-z0-9-]{0,31}$/;
const CREDENTIAL_FILE = "credential.json";

/** `<server-data>/agents/<sessionId>/credential.json`. */
export function credentialPath(agentsRoot: string, sessionId: string): string {
  return join(agentsRoot, sessionId, CREDENTIAL_FILE);
}

export function branchInUse(branch: string, sessionId: string): FinError {
  return finError(
    "BRANCH_IN_USE",
    `branch ${branch} is the root of session ${sessionId}, which is still live`,
    { branch, sessionId },
    "End that session first, or start this one from another branch.",
  );
}

/** Writes `file` to `path` private from creation, then renames it into place (temp-then-rename). */
function writeCredentialFile(path: string, file: CredentialFile, suffix: string): void {
  const tmp = `${path}.${suffix}.tmp`;
  const f = createPrivateFile(tmp);
  try {
    f.commit(`${JSON.stringify(file)}\n`);
    renameSync(tmp, path);
  } catch (e) {
    f.discard();
    rmSync(tmp, { force: true });
    throw e;
  }
}

export interface SessionCredentialDeps {
  readonly store: AgentStore;
  readonly credentials: Credentials;
  /** 5A's revoke path (tickets, connections, the audit, the store's end). */
  readonly revoke: RevokeDeps;
}

export class SessionCredentials {
  readonly #d: SessionCredentialDeps;

  constructor(deps: SessionCredentialDeps) {
    this.#d = deps;
  }

  /**
   * Mints `s`'s credential (id = `s.id`) for `by` and writes its file; answers its path.
   * `root`: the session's root branch, `null` in `plan` (whose credential holds no write scope).
   */
  mintForSession(s: Session, root: string | null, by: Principal): Effect.Effect<{ credentialId: string; path: string }, FinError> {
    return Effect.try({
      try: () => {
        const scopes = MODE_SCOPES[s.mode];
        const { file } = this.#d.credentials.mintWithId(by, s.id, {
          kind: "agent",
          label: `agent session (${s.role})`,
          scopes,
          projects: [s.projectId],
          // A plan session writes nothing (no `project.write`); its base is what it reads.
          branches: [root ?? s.base.branch],
          idleS: SESSION_IDLE_S,
          absoluteS: null,
        });
        const path = credentialPath(this.#d.store.root, s.id);
        try {
          writeCredentialFile(path, file, s.id);
        } catch (e) {
          // A credential no file holds is no use to anyone: ended at once.
          this.#revoke(s.id, "agent.session.start");
          throw e;
        }
        return { credentialId: s.id, path };
      },
      catch: toFinError,
    });
  }

  /** Ends `sessionId`'s credential through the one revoke path; nothing when it has ended. */
  revokeForSession(sessionId: string, rpc = "agent.session.end"): Effect.Effect<void> {
    return Effect.sync(() => this.#revoke(sessionId, rpc));
  }

  #revoke(sessionId: string, rpc: string): void {
    if (this.#d.credentials.get(sessionId) === undefined) return;
    revokeCredential(this.#d.revoke, { kind: "agent", id: sessionId }, { rpc, reason: "owner" });
  }

  /**
   * Restarts the expiry of `s`'s credential (a resume, Decision 18); a session whose credential
   * was never minted (a crash between its record and the mint) gets it now. An ended one is
   * `CREDENTIAL_REVOKED` or `CREDENTIAL_EXPIRED`: the runtime refuses the resume.
   */
  refreshForSession(s: Session): Effect.Effect<void, FinError> {
    if (this.#d.credentials.get(s.id) === undefined) {
      const by: Principal = { kind: s.createdBy.principal.kind, id: s.createdBy.principal.id, scopes: [], projects: "*", branches: "*", human: false };
      return Effect.asVoid(this.mintForSession(s, s.rootBranch ?? null, by));
    }
    return Effect.try({ try: () => void this.#d.credentials.refresh(s.id), catch: toFinError });
  }

  /**
   * Does a credential effect the store queued (`revokeCredential`, `refreshCredential`): true
   * when it was one, false for any other (the runtime's). Idempotent: a revoked credential is
   * not revoked twice, and a refresh of an ended one is reported and dropped (the session
   * cannot run as it, so its next request is refused).
   */
  handle(item: OutboxItem): boolean {
    const e = item.effect;
    if (e.kind === "revokeCredential") {
      this.#revoke(item.sessionId, "agent.session.end");
      return true;
    }
    if (e.kind === "refreshCredential") {
      const s = this.#d.store.get(item.sessionId);
      if (s !== undefined) {
        const exit = Effect.runSyncExit(this.refreshForSession(s));
        if (exit._tag === "Failure") process.stderr.write(`finisherd: warning: cannot refresh session ${s.id}'s credential\n`);
      }
      return true;
    }
    return false;
  }

  /** Every credential effect still owed (at start, and after each `apply`): done, then marked done. */
  drain(sessionId?: string): number {
    let n = 0;
    for (const item of this.#d.store.outbox(sessionId)) {
      if (!this.handle(item)) continue;
      this.#d.store.outboxDone(item.id);
      n++;
    }
    return n;
  }

  /** `BRANCH_IN_USE` when a session (but `except`) that has not ended has `root` as its root branch. */
  claimRoot(projectId: string, root: string, except?: string): Effect.Effect<void, FinError> {
    return Effect.suspend(() => {
      for (let cursor: string | undefined, page = 0; page < 1000; page++) {
        const { sessions, next } = this.#d.store.list({ projectId, limit: 200, ...(cursor === undefined ? {} : { cursor }) });
        const holder = sessions.find((s) => s.rootBranch === root && s.status !== "ended" && s.id !== except);
        if (holder !== undefined) return Effect.fail(branchInUse(root, holder.id));
        if (next === undefined) break;
        cursor = next;
      }
      return Effect.void;
    });
  }
}

/** What opening a session needs of the project (the registry, as the starter). */
export interface SessionBranchOps {
  /** Every branch of the project, archived ones too. */
  branches(projectId: string): Effect.Effect<ReadonlyArray<BranchView>, FinError>;
  /** A branch reference (name, id or `name#id`) as its id. */
  resolveBranch(projectId: string, ref: string): Effect.Effect<string, FinError>;
  /** `branch.create` as the starter (its actor and guard): the new branch. */
  createBranch(
    projectId: string,
    req: { readonly txId: string; readonly author: string; readonly name: string; readonly from: string; readonly at: { readonly seq: number } },
  ): Effect.Effect<BranchView, FinError>;
}

/** What `registryBranchOps` needs of the project registry (`ProjectRegistry`). */
export interface BranchRegistry {
  branches(projectId: string, all: boolean): Effect.Effect<{ readonly branches: ReadonlyArray<BranchView> }, FinError>;
  resolveBranch(projectId: string, ref: string): Effect.Effect<string, FinError>;
  history(projectId: string, op: "branchCreate", payload: HistoryPayload, meta: CallMeta): Effect.Effect<unknown, FinError>;
}

/**
 * `SessionBranchOps` over the project registry, as the starter: `meta` is the start call's actor
 * and the chain's guard (`{...actorFor(principal, headers), guard}`), so the root branch is
 * created exactly as the starter's own `branch.create` would be.
 */
export function registryBranchOps(registry: BranchRegistry, meta: CallMeta): SessionBranchOps {
  return {
    branches: (projectId) => Effect.map(registry.branches(projectId, true), (r) => r.branches),
    resolveBranch: (projectId, ref) => registry.resolveBranch(projectId, ref),
    createBranch: (projectId, req) =>
      Effect.map(registry.history(projectId, "branchCreate", { projectId, ...req }, meta), (r) => (r as BranchCreated).branch),
  };
}

/** A session to open: step 1 of Decision 15 is done (role, mode, harness, model resolved). */
export interface OpenSession extends Omit<NewSession, "id" | "rootBranch" | "base"> {
  /** The branch to start from (a reference; default `main`). */
  readonly base?: string;
  /**
   * An existing branch to work on instead of a new one (7B's child branches): it must not be
   * another live session's root (`BRANCH_IN_USE`). `base` is then that branch.
   */
  readonly root?: string;
  readonly budget: BudgetSpec;
}

/**
 * The next free `agent.<role>-<n>` (display only; ids stay `br<n>`). The spec's
 * `agent/<role>-<n>` cannot be a branch name: fin-store allows letters, digits, `.`, `_`, `-`.
 */
export function nextBranchName(branches: ReadonlyArray<{ readonly name: string }>, role: string): string {
  const re = new RegExp(`^agent\\.${role}-(\\d+)$`);
  let n = 0;
  for (const b of branches) {
    const m = re.exec(b.name);
    if (m !== null) n = Math.max(n, Number(m[1]));
  }
  return `agent.${role}-${n + 1}`;
}

/**
 * Decision 15, steps 2–5: the root branch (unless `plan`), the session record, its credential
 * and file, then `idle`. A failure after the record ends the session (its branch stays, as on
 * any end) and revokes what was minted.
 */
export function openSession(
  deps: { readonly creds: SessionCredentials; readonly store: AgentStore; readonly ops: SessionBranchOps; readonly ids: Ids },
  req: OpenSession,
  by: Principal,
): Effect.Effect<{ session: Session; credentialPath: string }, FinError> {
  return Effect.gen(function* () {
    if (!ROLE.test(req.role)) return yield* Effect.fail(finError("INVALID_REQUEST", "a role is a lower-case name", { field: "role" }));
    const id = deps.ids.ulid();
    const baseRef = req.root ?? req.base ?? "main";
    const baseId = yield* deps.ops.resolveBranch(req.projectId, baseRef);
    const branches = yield* deps.ops.branches(req.projectId);
    const base = branches.find((b) => b.id === baseId);
    if (base === undefined) return yield* Effect.fail(finError("BRANCH_NOT_FOUND", `no branch ${baseRef}`, { branch: baseRef }));
    let root: string | undefined;
    if (req.mode !== "plan" && req.root !== undefined) {
      root = base.id;
      yield* deps.creds.claimRoot(req.projectId, root);
    } else if (req.mode !== "plan") {
      const made = yield* deps.ops.createBranch(req.projectId, {
        txId: deps.ids.ulid(),
        author: sessionAuthor({ role: req.role, id }),
        name: nextBranchName(branches, req.role),
        from: base.id,
        at: { seq: base.head },
      });
      root = made.id;
    }
    const { base: _base, root: _root, ...rest } = req;
    const session = yield* Effect.tryPromise({
      try: () => deps.store.createSession({ ...rest, id, base: { branch: base.id, seq: base.head }, ...(root === undefined ? {} : { rootBranch: root }) }),
      catch: toFinError,
    });
    // Two starts on one existing root may both pass the first check: the second look, with
    // both recorded, ends whichever finds the other.
    const claimed = root === undefined || req.root === undefined ? Effect.void : deps.creds.claimRoot(req.projectId, root, id);
    const minted = yield* Effect.catch(Effect.andThen(claimed, deps.creds.mintForSession(session, root ?? null, by)), (e) =>
      Effect.andThen(
        Effect.sync(() => {
          deps.store.apply(id, { kind: "ended", reason: e.code });
          deps.creds.drain(id);
        }),
        Effect.fail(e),
      ),
    );
    const { session: ready } = deps.store.apply(id, { kind: "ready" });
    deps.creds.drain(id);
    return { session: ready, credentialPath: minted.path };
  });
}
