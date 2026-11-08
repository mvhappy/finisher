// Command rules in the chain (sub-project 7B Decisions 3–7; spec §7.2): `allow | ask | deny` on
// command globs, from five layers, enforced at stage 5 for `agent` principals whatever the
// transport (`fin`, `fin mcp`, a hand-written client, the built-in tool all reach the same RPC).
//
// - **Names.** A call is named by every command it could be (`commandsOf`: shared RPCs, alias
//   ops, `batch` for several ops, `render` for a render job, always `rpc:<method>` last); each
//   name is evaluated and the strictest outcome wins, so a payload shaped to look like another
//   command can only tighten.
// - **Layers** (`managed` > `local` > `project` > `user` > `role`; owner decision 2026-11-08,
//   Claude Code's model): every applicable rule of every layer is combined. **A `deny` in any
//   layer wins, then an `ask`, then `allow`; no match is `allow`** within the scopes. Layer
//   order only breaks ties (which rule a refusal names) and gates trust: a local `allow` never
//   lifts a project `deny`, a role `allow` never lifts a user `ask`, and nothing lifts a managed
//   `deny` or `ask`. `local` applies always, `project` only when trusted (`trust.ts`), `role`
//   is the session's role's rules (`roles/*.json`).
// - **`where`** conditions hold only on values the server resolved at stage 5; all present must
//   hold. Active now: `branch` (a reference resolved per project, against the branches the call
//   writes, acts on, forks from or reads), `mode` and `preset` (the session's mode and
//   provider), `bytesOver` (the files a transaction's `asset.add` ops add, by size on disk, a
//   relative path in the project folder as the store resolves it; a call adding none is 0
//   bytes; **a size that cannot be known holds**, failing closed: only `deny` and `ask` rules
//   are ever evaluated). **Inert until their source exists** (a condition with no value never
//   holds, by design until 7G and a render job kind): `minutesOver` (a render's output minutes:
//   no render job kind yet; `CommandRulesOptions.minutes` supplies it), `costOver` (no per-call
//   cost estimate yet) and `unlicensedDownload` (7G records downloads and licences).
// - **Outcomes.** `deny` is `RULE_DENIED {rule, layer}` (exit 7). `ask` names the command, rule
//   and layer for the chain's ask: a session with an active run gets a durable card and
//   `APPROVAL_PENDING` (exit 8, `approvals.ts`), anything else 5A's `APPROVAL_REQUIRED`.
//   A dry run (an RPC that takes `dryRun`, with it set: `fin --dry-run`) passes an `ask`, as
//   it keeps nothing; a `deny` still refuses it.
// - **Who.** Only `agent` principals (sp5 §4.4 gives people their own confirmation path). A
//   session's credential brings its session (project, role, mode); a hand-minted agent
//   credential has none, so the managed, user and local layers apply to it, with the local and
//   (trusted) project-file rules of every project its call names (stage 3's resolved projects:
//   `projectId`, `pinned`, a job's `project`, an opened folder's, and any `projectId`).
import { stat } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import type { FinError, Principal, Rule, RuleLayer, RuleWhere, Session } from "@finisher/contracts";
import { Effect } from "effect";
import type { Outcome, PolicyInput, PolicyRule } from "../chain/policy.ts";
import { resolveWrites } from "../chain/resolve.ts";
import { finError } from "../errors.ts";
import { isDryRun } from "./approvals.ts";
import { commandsOf } from "./command-of.ts";
import { globMatch } from "./glob.ts";
import type { Role } from "./roles.ts";
import type { RuleStore } from "./rule-store.ts";
import { managedRules, type ProjectRules, projectRules, userRules } from "./trust.ts";

export type Layer = RuleLayer;
export type RuleOutcome = { readonly effect: "allow" } | { readonly effect: "deny"; readonly rule: Rule; readonly layer: Layer } | { readonly effect: "ask"; readonly rule: Rule; readonly layer: Layer };

/** Ties between rules of one effect go to the higher layer. */
export const LAYER_ORDER: readonly Layer[] = ["managed", "local", "project", "user", "role"];
const RANK: Readonly<Record<Layer, number>> = Object.fromEntries(LAYER_ORDER.map((l, i) => [l, i])) as Record<Layer, number>;

/** What stage 5 resolved about a call, for `where` (absent: not known, so the condition does not hold). */
export interface Resolved {
  /** Every branch id the call writes, acts on, forks from or reads. */
  readonly branches: readonly string[];
  /** The ids `where.branch` references resolve to in the call's project (an unknown one is absent). */
  readonly refs: ReadonlyMap<string, string>;
  /** What the call's `asset.add` ops add (0: none); absent: a size that cannot be known. */
  readonly bytes?: number;
  readonly minutes?: number;
  readonly costMicros?: number;
  readonly unlicensedDownload?: boolean;
}

export const NOTHING_RESOLVED: Resolved = { branches: [], refs: new Map(), bytes: 0 };

function holds(w: RuleWhere | undefined, session: Session | undefined, r: Resolved): boolean {
  if (w === undefined) return true;
  if (w.mode !== undefined && session?.mode !== w.mode) return false;
  if (w.preset !== undefined && session?.provider !== w.preset) return false;
  if (w.branch !== undefined) {
    const id = r.refs.get(w.branch);
    if (id === undefined || !r.branches.includes(id)) return false;
  }
  // 7G resolves whether an `asset.add` is a download without a recorded licence; until then it never holds.
  if (w.unlicensedDownload !== undefined && r.unlicensedDownload !== true) return false;
  const over = (limit: number | undefined, value: number | undefined) => limit === undefined || (value !== undefined && value > limit);
  // Unknown bytes hold (fail closed); unknown minutes and cost do not (inert, module docs).
  const bytes = w.bytesOver === undefined || r.bytes === undefined || r.bytes > w.bytesOver;
  return bytes && over(w.minutesOver, r.minutes) && over(w.costOver, r.costMicros);
}

/** The applicable rules of one call, every layer together. */
export class RuleSet {
  readonly rules: readonly Rule[];

  constructor(rules: readonly Rule[]) {
    this.rules = [...rules].sort((a, b) => RANK[a.layer] - RANK[b.layer]);
  }

  /** One name's outcome: a `deny` anywhere, else an `ask`, else `allow`; the highest layer names it. */
  evaluate(input: { readonly command: string; readonly principal?: Principal; readonly session?: Session; readonly resolved: Resolved }): RuleOutcome {
    let ask: Rule | undefined;
    for (const rule of this.rules) {
      if (rule.effect === "allow" || !globMatch(rule.command, input.command) || !holds(rule.where, input.session, input.resolved)) continue;
      if (rule.effect === "deny") return { effect: "deny", rule, layer: rule.layer };
      ask ??= rule;
    }
    return ask === undefined ? { effect: "allow" } : { effect: "ask", rule: ask, layer: ask.layer };
  }

  /** The strictest outcome over every name a call could be, with the name it was decided on. */
  decide(commands: readonly string[], session: Session | undefined, resolved: Resolved): RuleOutcome & { readonly command?: string } {
    let ask: (RuleOutcome & { readonly command: string }) | undefined;
    for (const command of commands) {
      const o = this.evaluate({ command, ...(session === undefined ? {} : { session }), resolved });
      if (o.effect === "deny") return { ...o, command };
      if (o.effect === "ask") ask ??= { ...o, command };
    }
    return ask ?? { effect: "allow" };
  }
}

/** Where a call stands for the rule layers. */
export interface RuleContext {
  readonly session?: Session;
  /** The call's projects: the session's and every one the call names. */
  readonly projectIds?: readonly string[];
}

/** Every applicable rule of a call, and whether each project's file applies. */
export interface Layered {
  readonly rules: readonly Rule[];
  readonly projects: ReadonlyMap<string, ProjectRules>;
}

export interface RuleSource {
  layers(ctx: RuleContext): Layered;
}

export interface LayeredRulesDeps {
  readonly serverData: string;
  readonly store: RuleStore;
  readonly roles: ReadonlyMap<string, Role>;
  /** The folder of a project (open, else indexed), for its `finisher.rules.json`. */
  readonly dirOf: (projectId: string) => string | undefined;
}

/** The five layers as the server holds them (module docs; `trust.ts` for the files). */
export class LayeredRules implements RuleSource {
  readonly #d: LayeredRulesDeps;

  constructor(d: LayeredRulesDeps) {
    this.#d = d;
  }

  get serverData(): string {
    return this.#d.serverData;
  }

  project(projectId: string | undefined): ProjectRules {
    if (projectId === undefined) return { status: "none", rules: [], ignored: [] };
    return projectRules(this.#d.dirOf(projectId), this.#d.store.trustOf(projectId));
  }

  layers(ctx: RuleContext): Layered {
    const ids = [...new Set([...(ctx.session === undefined ? [] : [ctx.session.projectId]), ...(ctx.projectIds ?? [])])];
    const projects = new Map(ids.map((id) => [id, this.project(id)]));
    const role = ctx.session === undefined ? [] : (this.#d.roles.get(ctx.session.role)?.rules ?? []);
    // The local rules of each project (those naming none come back for each: kept once).
    const local = new Map<string, Rule>();
    for (const id of ids.length === 0 ? [undefined] : ids) for (const r of this.#d.store.applicable(id, ctx.session?.id)) local.set(r.id, r);
    return {
      rules: [
        ...managedRules(this.#d.serverData),
        ...local.values(),
        ...[...projects.values()].flatMap((p) => p.rules),
        ...userRules(this.#d.serverData),
        ...role,
      ],
      projects,
    };
  }
}

/** `RULE_DENIED {rule, layer}` (exit 7: do not retry). */
export function ruleDenied(command: string, rule: Rule, layer: Layer): FinError {
  return finError(
    "RULE_DENIED",
    `${command} is denied by rule ${rule.id} (${layer} layer)`,
    { command, rule: rule.id, layer, source: rule.source },
    "A person decides the rules: `finisherd agent rules ls` lists them. Do not retry this command.",
  );
}

export interface CommandRulesOptions {
  /** A render's output minutes (`where.minutesOver`); none until a render job kind exists. */
  readonly minutes?: (tag: string, payload: unknown) => number | undefined;
  /** File sizes for `where.bytesOver` (tests); default: the size on disk. */
  readonly sizeOf?: (path: string) => Promise<number | undefined>;
}

const field = (payload: unknown, key: string): unknown => (payload !== null && typeof payload === "object" ? (payload as Record<string, unknown>)[key] : undefined);

async function diskSize(path: string): Promise<number | undefined> {
  try {
    const s = await stat(path);
    return s.isFile() ? s.size : undefined;
  } catch {
    return undefined;
  }
}

/**
 * The files a call adds (every `asset.add` op of a transaction), a relative path in the project
 * folder as the store resolves it; `undefined` when one cannot be named (fail closed).
 */
function addedPaths(input: PolicyInput): string[] | undefined {
  if (input.tag !== "tx.commit") return [];
  const ops = field(field(input.payload, "tx"), "ops");
  if (!Array.isArray(ops)) return [];
  const paths = ops.filter((o) => field(o, "op") === "asset.add").map((o) => field(o, "path"));
  if (paths.length === 0) return [];
  if (paths.some((p) => typeof p !== "string" || p === "")) return undefined;
  const projectId = field(input.payload, "projectId");
  const dir = typeof projectId === "string" ? input.projects.dirOf(projectId) : undefined;
  const out: string[] = [];
  for (const p of paths as string[]) {
    if (isAbsolute(p)) out.push(p);
    else if (dir === undefined) return undefined;
    else out.push(join(dir, p));
  }
  return out;
}

/** What `where` needs of this call, resolved only for the conditions some rule names. */
function resolveFor(rules: readonly Rule[], input: PolicyInput, projectId: string | undefined, o: CommandRulesOptions): Effect.Effect<Resolved> {
  return Effect.gen(function* () {
    const wheres = rules.flatMap((r) => (r.where === undefined ? [] : [r.where]));
    const refNames = [...new Set(wheres.flatMap((w) => (w.branch === undefined ? [] : [w.branch])))];
    const refs = new Map<string, string>();
    const branches: string[] = [];
    if (refNames.length > 0 && projectId !== undefined) {
      const quiet = <A>(e: Effect.Effect<A, FinError>) => Effect.orElseSucceed(Effect.map(e, (v): A | undefined => v), () => undefined);
      for (const ref of refNames) {
        const id = yield* quiet(input.projects.resolveBranch(projectId, ref));
        if (id !== undefined) refs.set(ref, id);
      }
      branches.push(...((yield* quiet(resolveWrites(input.rule, input.payload, input.projects))) ?? []));
      for (const key of ["from", "branch"]) {
        const v = field(input.payload, key);
        if (typeof v !== "string") continue;
        const id = yield* quiet(input.projects.resolveBranch(projectId, v));
        if (id !== undefined) branches.push(id);
      }
    }
    let bytes: number | undefined = 0;
    if (wheres.some((w) => w.bytesOver !== undefined)) {
      const paths = addedPaths(input);
      const sizes = paths === undefined ? undefined : yield* Effect.promise(() => Promise.all(paths.map((p) => (o.sizeOf ?? diskSize)(p))));
      bytes = sizes === undefined || sizes.some((s) => s === undefined) ? undefined : sizes.reduce<number>((a, s) => a + s!, 0);
    }
    const minutes = wheres.some((w) => w.minutesOver !== undefined) ? o.minutes?.(input.tag, input.payload) : undefined;
    return { branches, refs, ...(bytes === undefined ? {} : { bytes }), ...(minutes === undefined ? {} : { minutes }) };
  });
}

/** What the policy reads of the agent store (`AgentStore` is one). */
export interface RuleSessions {
  get(sessionId: string): Session | undefined;
}

/**
 * The stage-5 `PolicyRule` (appended in `handlers.ts`): agent principals only; `deny` is
 * `RULE_DENIED`, `ask` the chain's ask (durable for a running session, `ChainDeps.agentAsk`),
 * `allow` no opinion (the other policies' guards stand).
 */
export function commandRules(rules: RuleSource, sessions: RuleSessions, options: CommandRulesOptions = {}): PolicyRule {
  return (input) =>
    Effect.gen(function* () {
      if (input.principal.kind !== "agent") return undefined;
      const session = sessions.get(input.principal.id);
      // Every project the call names (stage 3's), and any `projectId` besides: more projects can only add rules.
      const payloadProject = field(input.payload, "projectId");
      const projectIds = [...new Set([...(input.projectIds ?? []), ...(typeof payloadProject === "string" ? [payloadProject] : [])])];
      const projectId = session?.projectId ?? (typeof payloadProject === "string" ? payloadProject : projectIds[0]);
      const { rules: all } = yield* Effect.try({ try: () => rules.layers({ ...(session === undefined ? {} : { session }), projectIds }), catch: (e) => finError("INTERNAL", `command rules cannot be read: ${String(e)}`) });
      if (all.length === 0) return undefined;
      const set = new RuleSet(all);
      const names = commandsOf(input.tag, input.payload);
      const resolved = yield* resolveFor(set.rules, input, projectId, options);
      const outcome = set.decide(names, session, resolved);
      if (outcome.effect === "deny") return { deny: ruleDenied(outcome.command ?? names[0]!, outcome.rule, outcome.layer) } satisfies Outcome;
      // A dry run keeps nothing: an ask lets it through (no card, no quota); a deny still refuses it.
      if (outcome.effect === "ask" && isDryRun(input.tag, input.payload)) return undefined;
      if (outcome.effect === "ask") {
        const ask = { reason: `rule ${outcome.rule.id} (${outcome.layer} layer) asks a person first`, command: outcome.command ?? names[0]!, rule: outcome.rule.id, layer: outcome.layer };
        return { ask } satisfies Outcome;
      }
      return undefined;
    });
}
