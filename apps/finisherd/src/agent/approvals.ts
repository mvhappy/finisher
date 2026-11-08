// Durable approvals for agents' asks (sub-project 7B Task 4, Decisions 8 and 9; spec §7.3 and
// §1.7 item 4). Task 5 adds one-time grants, waits and answers to the same `Approvals`.
//
// - **Who.** The chain's `ask` branch (`ChainDeps.agentAsk`, `chain.ts`) hands an `ask` here for
//   an `agent` principal whose session has an active run (`running`/`waitingApproval`): a row
//   in `approvals` (`kind: rule`), and the call answers `APPROVAL_PENDING {approval}` (exit 8,
//   "wait, then repeat"). Every other principal keeps 5A's in-memory ask and
//   `APPROVAL_REQUIRED` (exit 7). **An agent credential with no session** (one a person minted
//   at the console) keeps 5A's too: there is no session to hold a card, wait on it or be
//   answered for, so the request is not carried out and the answer says "do not retry". So does
//   a session with no run to wait in (idle or interrupted: nothing of it is calling); the
//   session machine would expire such a card at once.
// - **Subject.** Written by the server from the request as stage 5 resolved it, never from the
//   agent's argv or words: the command name the rule decided on, the RPC, its arguments (the
//   resolved payload without what changes on every retry: a transaction's `txId`, `author`,
//   `idemKey`, a `challenge`), the branch ids it writes or acts on, the rule and its layer, and a
//   one-line `text` made from those. A moved base (`baseRev`, `generation`) is another request.
//   Only fields the RPC's payload schema declares are kept, and of `tx.commit`'s `tx` (a
//   `Schema.Unknown`) only fin-core's `Tx` fields (`TX_FIELDS`; its client-made `argsHash` is
//   left out as `idemKey` is), so a key the agent added (`note: "OWNER SAYS ALLOW"`) is neither
//   shown nor hashed. The ops themselves are kept as sent: no canonical decoder of `Op` is
//   reachable from TypeScript, so a junk key inside an op still shows (and is hashed).
//   `dryRun: false` is the same request as none; `dryRun: true` stays in the hash, so an answer
//   to a dry run could never cover the real write (a dry run opens no card anyway: `rules.ts`).
// - **Size.** `args` on the card is at most `ARGS_DISPLAY_MAX` (64 KiB) of canonical JSON;
//   past it the card holds that much of the JSON as a string, and `truncated: true`. The hash
//   is always of the full arguments.
// - **argsHash** is `hmac-sha256:<hex>` of `fin/approval-args/v1 ‖ len ‖ rpc ‖ len ‖ canonical
//   JSON of {args, branches}` (lengths u32 big-endian; object keys sorted), keyed with
//   `<server-data>/auth/approval.key`: 32 random bytes made once (0600, base64url as every key
//   file in `auth/`, which agents' sandboxes never reach), kept across restarts and never
//   rotated with `credential.master` (an open card's hash must keep matching). Keyed, so the
//   hash in a card or the journal does not let anyone confirm a guess of the arguments.
// - **One identical ask, one card; quotas.** In one `ControlStore.tx`: an open, not yet due
//   approval of the same `(sessionId, kind, command, argsHash)` is answered again (no new row,
//   no new node); else at most 16 open per session and 256 per server (`store-approvals.ts`
//   counts the kinds agents open), past either `APPROVAL_LIMIT {open, limit, scope}` (exit 4),
//   never dropping an older card a person may be reading; else the row (30 minutes) through
//   `AgentStore.openApproval`, which moves the session to `waitingApproval`. A run that ended
//   (or was interrupted) after `agentAsk` looked leaves the card expired at once: the
//   transaction is rolled back and the call keeps 5A's ask, so `APPROVAL_PENDING` only ever
//   names an open card. A branch that cannot be resolved fails the call (no card without it).
// - **Node and journal.** A new card becomes an `approval` node in the active run and an
//   `approvalOpened` watch item (`Runtime.approvalOpened`); the journal's `agent.approval`
//   (meta `kind: rule`) is written when it is answered or expires, as 7A's budget card is.
// - **Grants (Task 5, Decision 10).** An `allow` answer is a one-time grant of exactly
//   `(sessionId, command, argsHash)`, the hash computed as the card's was. When a rule asks
//   again, `commandRules` (`grantPort`) consumes the oldest such grant in `control.db` before
//   the handler runs, recording the call's idempotency key (the payload's `idemKey`, else the
//   write's `txId`, else its `FIN_CALL`, else a fresh id), and the call goes ahead; a second
//   identical request, other arguments or another session find none and ask. The write commits in the project's store,
//   the grant in `control.db`: consuming first means a crash between them loses the grant (the
//   agent asks again) and never doubles a write (owner decision 2026-11-08). A handler that
//   fails without committing gives it back (`restoreGrant`, through the chain's `undo`). A grant
//   expires with its approval.
// - **Waits (Decision 11).** `wait(id, timeoutMs)` answers `allow`, `deny`, `expired` (also an
//   open card past its `expiresAt`) or, at the deadline (by the injected clock, and in real
//   time whatever that clock does), `pending`; an answer wakes it at once (`notify`), else it
//   looks again every `WAIT_POLL_MS`. A wait ends when its fiber is interrupted (the RPC of a
//   client that went away) or the caller's signal aborts; a session has at most
//   `MAX_WAITS_PER_SESSION` waits at a time, past it `APPROVAL_LIMIT {waits, limit, scope}`.
// - **Answers (Decision 12).** `answer` records a person's decision on a `rule` or `provider`
//   card through the runtime (`approve`: the journal, the watchers, a provider request's
//   `respond`), after a `remember` wrote its rule (`remember.ts`); a refused answer takes the
//   rule back. A `budget` card is the runtime's own (`Runtime.approve` with its `add`).
import { createHmac, randomBytes } from "node:crypto";
import { join } from "node:path";
import { type Approval, type ApprovalDecision, type FinError, FinRpcs, type FinRpcTag, type ListedRule, type Principal, type RememberScope, type RuleLayer, type Session } from "@finisher/contracts";
import { Effect } from "effect";
import { authDir, readKeyFile, writeKeyFile } from "../auth/keys.ts";
import type { AgentAsk } from "../chain/chain.ts";
import type { Ask, PolicyInput } from "../chain/policy.ts";
import { ulid } from "../control/ulid.ts";
import { actorFor } from "../diagnostics.ts";
import { approvalPending } from "../chain/fixes.ts";
import { resolveWrites } from "../chain/resolve.ts";
import { ensurePrivateDir } from "../config.ts";
import type { ControlStore } from "../control/db.ts";
import { finError, toFinError } from "../errors.ts";
import { KEY_BYTES } from "../auth/crypto.ts";
import type { Clock } from "./clock.ts";
import type { AgentStore } from "./store.ts";
import { approvalNotFound } from "./session-access.ts";
import { Grants, OpenApprovals } from "./store-approvals.ts";

/** How long a person has to answer a rule or provider card (spec §7.3). */
export const RULE_APPROVAL_TTL_MS = 30 * 60_000;
/** Open cards one session may have, and the server (5A's numbers, refused past them). */
export const MAX_OPEN_PER_SESSION = 16;
export const MAX_OPEN_PER_SERVER = 256;
/** `<server-data>/auth/approval.key`. */
export const APPROVAL_KEY_FILE = "approval.key";
const HASH_LABEL = "fin/approval-args/v1";
const TEXT_MAX = 1000;
/** `agent.approval.wait`'s cap (spec §7.3). */
const APPROVAL_WAIT_MAX = 90_000;
/** The most canonical JSON of `args` a card holds (bytes); the hash covers all of it. */
export const ARGS_DISPLAY_MAX = 64 * 1024;
/** Fields that change on every retry of the same request: not its arguments. */
const VOLATILE = new Set(["txId", "author", "idemKey", "challenge"]);
/** fin-core's `Tx` (`crates/fin-core/src/tx.rs`) less `txId`, `author` and its client-made `argsHash`. */
const TX_FIELDS = ["branch", "baseRev", "schemaVersion", "ops"] as const;

/** What a card shows: written by the server from the resolved request (module docs). */
export interface Subject {
  readonly command: string;
  readonly rpc: string;
  readonly args: unknown;
  /** The branch ids the call writes or acts on, resolved. */
  readonly branches: readonly string[];
  readonly rule: string;
  readonly layer: RuleLayer;
  readonly text: string;
  /** `args` is the first `ARGS_DISPLAY_MAX` bytes of their canonical JSON (module docs). */
  readonly truncated?: true;
}

export interface Asked {
  readonly id: string;
  readonly approval: Approval;
  /** True when an identical open card answered (no new row). */
  readonly reused: boolean;
}

/** `undefined`: the session's run ended before the card was written; nothing was kept. */
export type AskResult = Asked | undefined;

/** Waits one session may have at a time (past it `APPROVAL_LIMIT`). */
export const MAX_WAITS_PER_SESSION = 4;

/** How often a wait looks at its card again when no answer woke it (ms of real time). */
export const WAIT_POLL_MS = 250;

export type WaitOutcome = "allow" | "deny" | "expired" | "pending";

/** An answer: the approval, and what a `remember` added (`remember.ts`). */
export interface Answered {
  readonly approval: Approval;
  readonly rule?: ListedRule;
  readonly notice?: string;
}

/** The approvals port (plan 7B's frozen interface; `answer` also says what it remembered). */
export interface Approvals {
  ask(s: Session, kind: "rule" | "provider", subject: Subject, argsHash: string): Effect.Effect<AskResult, FinError>;
  consumeGrant(sessionId: string, command: string, argsHash: string, idem: string): Effect.Effect<boolean>;
  restoreGrant(sessionId: string, idem: string): Effect.Effect<void>;
  wait(id: string, timeoutMs: number, signal?: AbortSignal): Effect.Effect<WaitOutcome, FinError>;
  answer(id: string, decision: ApprovalDecision, by: Principal, remember?: RememberScope): Effect.Effect<Answered, FinError>;
}

/** What a `remember` wrote, and how to take it back when the answer is refused. */
export interface Remembered {
  readonly rule: ListedRule;
  readonly notice?: string;
  readonly undo: () => void;
}

const plain = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);

/** The fields `tag`'s payload schema declares (every `FinRpcs` payload is a struct). */
function fieldsOf(tag: string): ReadonlySet<string> | undefined {
  const fields = (FinRpcs.requests.get(tag) as { payloadSchema?: { fields?: Record<string, unknown> } } | undefined)?.payloadSchema?.fields;
  return fields === undefined ? undefined : new Set(Object.keys(fields));
}

/** Whether a call is a dry run: its RPC takes `dryRun` (a write that keeps nothing) and it is set. */
export function isDryRun(tag: string, payload: unknown): boolean {
  return plain(payload) && payload.dryRun === true && fieldsOf(tag)?.has("dryRun") === true;
}

/**
 * A request's arguments (module docs): its RPC's fields without the retry-varying ones, and of
 * a transaction only the `Tx` fields; what the hash covers, in full.
 */
export function requestArgs(tag: string, payload: unknown): unknown {
  if (!plain(payload)) return payload;
  const known = fieldsOf(tag);
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(payload)) {
    if (VOLATILE.has(k) || (known !== undefined && !known.has(k)) || (k === "dryRun" && v !== true)) continue;
    out[k] = tag === "tx.commit" && k === "tx" && plain(v) ? Object.fromEntries(TX_FIELDS.filter((f) => v[f] !== undefined).map((f) => [f, v[f]])) : v;
  }
  return out;
}

/** JSON with every object's keys sorted (arrays keep their order). */
export function canonicalJson(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map((x) => canonicalJson(x ?? null)).join(",")}]`;
  if (plain(v)) {
    const keys = Object.keys(v).filter((k) => v[k] !== undefined).sort();
    return `{${keys.map((k) => `${JSON.stringify(k)}:${canonicalJson(v[k])}`).join(",")}}`;
  }
  return JSON.stringify(v ?? null);
}

/** The card of a request (module docs): the server's words only. */
export function subjectOf(r: { command: string; tag: FinRpcTag | string; payload: unknown; rule: string; layer: RuleLayer; branches?: readonly string[] }): Subject {
  const args = requestArgs(r.tag, r.payload);
  const json = canonicalJson(args);
  const branches = [...(r.branches ?? [])];
  const full = `${r.command} ${json.slice(0, TEXT_MAX)}${branches.length === 0 ? "" : ` on ${branches.join(", ")}`}`;
  const text = full.length > TEXT_MAX ? `${full.slice(0, TEXT_MAX - 1)}…` : full;
  const base = { command: r.command, rpc: r.tag, branches, rule: r.rule, layer: r.layer, text };
  if (Buffer.byteLength(json, "utf8") <= ARGS_DISPLAY_MAX) return { ...base, args };
  // A cut through a multi-byte character decodes as U+FFFD: dropped.
  const shown = Buffer.from(json, "utf8").subarray(0, ARGS_DISPLAY_MAX).toString("utf8").replace(/\uFFFD+$/, "");
  return { ...base, args: `${shown}…`, truncated: true };
}

function framed(h: ReturnType<typeof createHmac>, s: string): void {
  const b = Buffer.from(s, "utf8");
  const len = Buffer.alloc(4);
  len.writeUInt32BE(b.length);
  h.update(len).update(b);
}

/** The keyed hash of a request's full arguments (`requestArgs`, never a capped card's; module docs). */
export function argsHash(key: Buffer, s: Pick<Subject, "command" | "rpc" | "args" | "branches">): string {
  const h = createHmac("sha256", key).update(HASH_LABEL, "ascii");
  framed(h, s.command);
  framed(h, s.rpc);
  framed(h, canonicalJson({ args: s.args, branches: s.branches }));
  return `hmac-sha256:${h.digest("hex")}`;
}

/** The server's approval key, made once in `<server-data>/auth/` (module docs). */
export function approvalKey(serverData: string): Buffer {
  const dir = authDir(serverData);
  ensurePrivateDir(dir);
  const path = join(dir, APPROVAL_KEY_FILE);
  try {
    return readKeyFile(path);
  } catch (e) {
    if ((e as { code?: unknown }).code !== "ENOENT") throw e;
    const key = randomBytes(KEY_BYTES);
    writeKeyFile(path, key);
    return key;
  }
}

/** `APPROVAL_LIMIT {open, limit, scope}` (exit 4): refused, the open cards stay as they are. */
export function approvalLimit(open: number, limit: number, scope: "session" | "server"): FinError {
  return finError(
    "APPROVAL_LIMIT",
    scope === "session" ? `this session has ${open} approvals waiting for a person, its most` : `this server has ${open} approvals waiting for a person, its most`,
    { open, limit, scope },
    "Too many requests are waiting for a person's answer. Wait for some to be answered or to expire before asking for more.",
  );
}

/** `APPROVAL_LIMIT {waits, limit, scope}` (exit 4): a session already waits on `MAX_WAITS_PER_SESSION` cards. */
function waitLimit(waits: number): FinError {
  return finError(
    "APPROVAL_LIMIT",
    `this session already has ${waits} approval waits running, its most`,
    { waits, limit: MAX_WAITS_PER_SESSION, scope: "session" },
    "Wait on one approval at a time: let a running wait end before starting another.",
  );
}

export interface ApprovalsDeps {
  readonly control: ControlStore;
  readonly store: AgentStore;
  readonly clock: Clock;
  /** A new card's node and watch item (`Runtime.approvalOpened`); best effort, the row stands. */
  readonly opened?: (s: Session, a: Approval, node: Record<string, unknown>) => void | Promise<void>;
  /** Records a person's answer (`Runtime.approve`, which also settles a provider request). */
  readonly approve?: (id: string, decision: ApprovalDecision, by: Principal) => Effect.Effect<{ approval: Approval }, FinError>;
  /** Writes the rule a `remember` asks for (`remember.ts`). */
  readonly remember?: (a: Approval, decision: ApprovalDecision, scope: RememberScope, by: Principal) => Remembered;
}

/** Thrown inside the ask's transaction to roll it back: no run is left to wait on the card. */
const RUN_GONE = Symbol("runGone");

export class AgentApprovals implements Approvals {
  readonly #d: ApprovalsDeps;
  readonly #open: OpenApprovals;
  readonly #grants: Grants;
  readonly #waiters = new Map<string, Set<() => void>>();
  /** Running waits by session (`MAX_WAITS_PER_SESSION`). */
  readonly #waits = new Map<string, number>();

  constructor(d: ApprovalsDeps) {
    this.#d = d;
    this.#open = new OpenApprovals(d.control.db);
    this.#grants = new Grants(d.control.db);
  }

  consumeGrant(sessionId: string, command: string, hash: string, idem: string): Effect.Effect<boolean> {
    return Effect.map(this.takeGrant(sessionId, command, hash, idem), (id) => id !== undefined);
  }

  /** `consumeGrant`, answering the approval whose grant it consumed. */
  takeGrant(sessionId: string, command: string, hash: string, idem: string): Effect.Effect<string | undefined> {
    return Effect.sync(() => this.#grants.consume(sessionId, command, hash, idem, this.#d.clock.now()));
  }

  restoreGrant(sessionId: string, idem: string, approvalId?: string): Effect.Effect<void> {
    return Effect.sync(() => void this.#grants.restore(sessionId, idem, approvalId));
  }

  /** Wakes the waits on `id` (an answer was recorded). */
  notify(id: string): void {
    for (const wake of this.#waiters.get(id) ?? []) wake();
  }

  #outcome(id: string, now: number): WaitOutcome {
    const a = this.#d.store.approval(id);
    if (a === undefined) throw approvalNotFound(id);
    if (a.status === "answered") return a.decision ?? "deny";
    if (a.status === "expired" || a.expiresAt <= now) return "expired";
    return "pending";
  }

  /** How many waits are running (tests). */
  get waiting(): number {
    return [...this.#waits.values()].reduce((n, x) => n + x, 0);
  }

  wait(id: string, timeoutMs: number, signal?: AbortSignal): Effect.Effect<WaitOutcome, FinError> {
    return Effect.tryPromise({
      // `interrupted` aborts when the fiber is interrupted (a client that went away).
      try: async (interrupted) => {
        const a = this.#d.store.approval(id);
        if (a === undefined) throw approvalNotFound(id);
        const waits = this.#waits.get(a.sessionId) ?? 0;
        if (waits >= MAX_WAITS_PER_SESSION) throw waitLimit(waits);
        this.#waits.set(a.sessionId, waits + 1);
        const stop = signal === undefined ? interrupted : AbortSignal.any([interrupted, signal]);
        try {
          return await this.#waitFor(id, timeoutMs, stop);
        } finally {
          const left = (this.#waits.get(a.sessionId) ?? 1) - 1;
          if (left <= 0) this.#waits.delete(a.sessionId);
          else this.#waits.set(a.sessionId, left);
        }
      },
      catch: toFinError,
    });
  }

  async #waitFor(id: string, timeoutMs: number, signal: AbortSignal): Promise<WaitOutcome> {
    const clock = this.#d.clock;
    const ms = Math.max(0, Math.min(timeoutMs, APPROVAL_WAIT_MAX));
    // Due by the injected clock, and in real time whatever that clock does: a wait never blocks past 90 s.
    const deadline = clock.now() + ms;
    const realDeadline = Date.now() + ms;
    for (;;) {
      const now = clock.now();
      const o = this.#outcome(id, now);
      if (o !== "pending" || now >= deadline || Date.now() >= realDeadline || signal.aborted) return o;
      await this.#sleep(id, Math.max(1, Math.min(WAIT_POLL_MS, deadline - now, realDeadline - Date.now())), signal);
    }
  }

  #sleep(id: string, ms: number, signal: AbortSignal): Promise<void> {
    return new Promise<void>((resolve) => {
      let set = this.#waiters.get(id);
      if (set === undefined) this.#waiters.set(id, (set = new Set()));
      const done = () => {
        clearTimeout(timer);
        signal.removeEventListener("abort", done);
        set!.delete(done);
        if (set!.size === 0 && this.#waiters.get(id) === set) this.#waiters.delete(id);
        resolve();
      };
      const timer = setTimeout(done, ms);
      signal.addEventListener("abort", done, { once: true });
      set.add(done);
    });
  }

  answer(id: string, decision: ApprovalDecision, by: Principal, remember?: RememberScope): Effect.Effect<Answered, FinError> {
    const d = this.#d;
    const self = this;
    return Effect.gen(function* () {
      const a = d.store.approval(id);
      if (a === undefined) return yield* Effect.fail(approvalNotFound(id));
      if (remember !== undefined && a.kind !== "rule" && a.kind !== "provider") {
        return yield* Effect.fail(finError("INVALID_REQUEST", "only a rule or provider approval can be remembered as a rule", { field: "remember", reason: "notARuleApproval", kind: a.kind }, "answer without remember"));
      }
      if (d.approve === undefined) return yield* Effect.die(new Error("approvals have no runtime to answer through"));
      // An answer that cannot be recorded (answered, past its deadline) writes no rule.
      const written = remember !== undefined && a.status === "open" && a.expiresAt > d.clock.now() && d.remember !== undefined
        ? yield* Effect.try({ try: () => d.remember!(a, decision, remember, by), catch: toFinError })
        : undefined;
      const r = yield* Effect.tapError(d.approve(id, decision, by), () => Effect.sync(() => written?.undo()));
      self.notify(id);
      return { approval: r.approval, ...(written === undefined ? {} : { rule: written.rule, ...(written.notice === undefined ? {} : { notice: written.notice }) }) };
    });
  }

  ask(s: Session, kind: "rule" | "provider", subject: Subject, hash: string): Effect.Effect<AskResult, FinError> {
    return Effect.flatMap(
      Effect.try({ try: () => this.#askIn(s, kind, subject, hash), catch: toFinError }),
      (r): Effect.Effect<AskResult> => {
        if (r === undefined) return Effect.succeed(undefined);
        if (r.reused || this.#d.opened === undefined) return Effect.succeed(r);
        const node = { approvalId: r.id, kind, status: "open", command: subject.command, rule: subject.rule, layer: subject.layer, text: subject.text, expiresAt: r.approval.expiresAt };
        const opened = this.#d.opened;
        return Effect.as(Effect.promise(async () => {
          try {
            await opened(r.session, r.approval, node);
          } catch {
            // The card is in `control.db`; its node is for display only.
          }
        }), r);
      },
    );
  }

  #askIn(s: Session, kind: "rule" | "provider", subject: Subject, hash: string): (Asked & { session: Session }) | undefined {
    try {
      return this.#d.control.tx(() => this.#askTx(s, kind, subject, hash));
    } catch (e) {
      if (e === RUN_GONE) return undefined;
      throw e;
    }
  }

  #askTx(s: Session, kind: "rule" | "provider", subject: Subject, hash: string): Asked & { session: Session } {
    const now = this.#d.clock.now();
    const same = this.#open.matching(s.id, kind, subject.command, hash, now);
    if (same !== undefined) return { id: same, approval: this.#d.store.approval(same)!, reused: true, session: s };
    const mine = this.#open.ofSession(s.id, now);
    if (mine >= MAX_OPEN_PER_SESSION) throw approvalLimit(mine, MAX_OPEN_PER_SESSION, "session");
    const all = this.#open.all(now);
    if (all >= MAX_OPEN_PER_SERVER) throw approvalLimit(all, MAX_OPEN_PER_SERVER, "server");
    const { approval, session } = this.#d.store.openApproval({ sessionId: s.id, kind, subject, argsHash: hash, expiresAt: now + RULE_APPROVAL_TTL_MS });
    // The run ended (or was interrupted) since `agentAsk` looked: the card was expired at once. Roll back.
    if (approval.status !== "open" || session.status !== "waitingApproval") throw RUN_GONE;
    return { id: approval.id, approval, reused: false, session };
  }
}

/** What the port reads of sessions (`AgentStore` is one). */
export interface AskSessions {
  get(sessionId: string): Session | undefined;
  activeRun(sessionId: string): string | null;
}

/**
 * The chain's `agentAsk` port (Decision 8): a rule `ask` of an agent session with an active run
 * becomes a durable card and `APPROVAL_PENDING`; anything else (`undefined`) keeps 5A's ask.
 */
export function agentAsk(approvals: Pick<Approvals, "ask">, sessions: AskSessions, key: Buffer): AgentAsk {
  return (input) =>
    Effect.gen(function* () {
      const { principal, ask } = input;
      if (principal.kind !== "agent" || ask.command === undefined || ask.rule === undefined || ask.layer === undefined) return undefined;
      const s = sessions.get(principal.id);
      if (s === undefined || sessions.activeRun(s.id) === null) return undefined;
      const { subject, hash } = yield* cardOf(input, { ...ask, command: ask.command, rule: ask.rule, layer: ask.layer }, key);
      const asked = yield* approvals.ask(s, "rule", subject, hash);
      return asked === undefined ? undefined : approvalPending(input.tag, asked.approval, ask.command, asked.reused);
    });
}

/** A request's card and its hash, as the ask writes them and a grant is looked up by. */
function cardOf(input: PolicyInput, ask: Required<Pick<Ask, "command" | "rule" | "layer">>, key: Buffer): Effect.Effect<{ subject: Subject; hash: string }, FinError> {
  return Effect.map(
    // A branch that cannot be resolved fails the call: a card must name what it writes.
    resolveWrites(input.rule, input.payload, input.projects),
    (branches) => {
      const subject = subjectOf({ command: ask.command, tag: input.tag, payload: input.payload, rule: ask.rule, layer: ask.layer, branches });
      return { subject, hash: argsHash(key, { command: ask.command, rpc: input.tag, args: requestArgs(input.tag, input.payload), branches: subject.branches }) };
    },
  );
}

/**
 * The idempotency key a grant is consumed for (module docs, Decision 10): the payload's
 * `idemKey` (`jobs.submit`, `--idem-key`), else the write's `txId` (`--idem-key`, or `FIN_CALL`
 * under `FIN_CALL_IDEM=1`), else the call's `FIN_CALL`, else a fresh id.
 */
export function idemOf(input: Pick<PolicyInput, "payload" | "principal" | "headers">): string {
  const p = plain(input.payload) ? input.payload : {};
  const tx = plain(p.tx) ? p.tx : {};
  const key = typeof p.idemKey === "string" && p.idemKey !== "" ? p.idemKey : undefined;
  const txId = typeof p.txId === "string" ? p.txId : typeof tx.txId === "string" ? tx.txId : undefined;
  return key ?? txId ?? actorFor(input.principal, input.headers).actor.call ?? `grant:${ulid()}`;
}

/** What `commandRules` asks of grants: one consumed for this call, and how to give it back. */
export type GrantPort = (input: PolicyInput, ask: Required<Pick<Ask, "command" | "rule" | "layer">>) => Effect.Effect<Effect.Effect<void> | undefined, FinError>;

/**
 * The one-time grants for `commandRules` (Decision 10): an agent session's `ask` consumes a
 * matching `allow` answer, if there is one, before the handler; the answer is the effect that
 * restores it (`undefined`: no grant, the call asks). `afterConsume` is a test's crash point.
 */
export function grantPort(approvals: Pick<AgentApprovals, "takeGrant" | "restoreGrant">, sessions: Pick<AskSessions, "get">, key: Buffer, afterConsume?: (sessionId: string, idem: string) => Promise<void>): GrantPort {
  return (input, ask) =>
    Effect.gen(function* () {
      if (input.principal.kind !== "agent") return undefined;
      const s = sessions.get(input.principal.id);
      if (s === undefined) return undefined;
      const { hash } = yield* cardOf(input, ask, key);
      const idem = idemOf(input);
      const id = yield* approvals.takeGrant(s.id, ask.command, hash, idem);
      if (id === undefined) return undefined;
      if (afterConsume !== undefined) yield* Effect.promise(() => afterConsume(s.id, idem));
      return approvals.restoreGrant(s.id, idem, id);
    });
}
