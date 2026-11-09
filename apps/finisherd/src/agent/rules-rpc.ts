// The rule RPCs (sub-project 7B Decision 7): `agent.rules.list` reads every layer;
// `agent.rules.add|remove` write the `local` layer and `agent.project.trust` trusts a project's
// `finisher.rules.json`, all three the owner's (`agent.admin`, people only, challenge code: the
// chain checked it). A rule or trust names a project or a session the caller must reach: a
// session out of reach is `SESSION_NOT_FOUND`, a local rule out of reach is not listed and its
// removal answers `removed: false`, exactly as a missing one. Listing a project gives its
// file's `rulesHash`, which trusting must send back (`expectedHash`: what the person saw).
// Changes are audited `agent.rule.add|remove` (the rule id, its layer, effect, project and
// session) and `agent.project.trust` (the project and the `rulesHash` trusted); never the
// command text.
import type { FinError, ListedRule, Principal, Rule, Session } from "@finisher/contracts";
import { Effect } from "effect";
import type { Audit } from "../control/audit.ts";
import { ulid } from "../control/ulid.ts";
import { finError, toFinError } from "../errors.ts";
import type { Clock } from "./clock.ts";
import type { LayeredRules } from "./rules.ts";
import type { LocalRule, RuleStore } from "./rule-store.ts";
import { sessionInReach } from "./session-access.ts";
import type { AgentStore } from "./store.ts";
import { managedRules, trustProject, userRules } from "./trust.ts";

type Guard = <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError>;

export interface RulesRpcDeps {
  readonly guard: Guard;
  readonly store: AgentStore;
  readonly rules: LayeredRules;
  readonly ruleStore: RuleStore;
  readonly audit: Audit;
  readonly clock: Clock;
  /** The folder of a project (open, else indexed). */
  readonly dirOf: (projectId: string) => string | undefined;
  readonly principalOf: (options: object) => Principal;
}

const inReach = (p: Principal, projectId: string) => p.projects === "*" || p.projects.includes(projectId);
const sync = <A>(f: () => A) => Effect.try({ try: f, catch: toFinError });

/** A rule as listed: its own fields only, whether it applies now, and whether it is a proven remembered allow. */
function listed(r: Rule | LocalRule, applies: boolean, remembered = false): ListedRule {
  const l = r as LocalRule;
  return {
    id: r.id,
    effect: r.effect,
    command: r.command,
    ...(r.where === undefined ? {} : { where: r.where }),
    layer: r.layer,
    source: r.source,
    ...(r.createdBy === undefined ? {} : { createdBy: r.createdBy }),
    createdAt: r.createdAt,
    ...(l.projectId === undefined ? {} : { projectId: l.projectId }),
    ...(l.sessionId === undefined ? {} : { sessionId: l.sessionId }),
    applies,
    ...(remembered ? { remembered: true as const } : {}),
  };
}

export function rulesHandlers(deps: RulesRpcDeps) {
  const { guard, store, rules, ruleStore } = deps;
  const by = deps.principalOf;
  /** A local rule is in reach when its project (or its session's) is. */
  const ruleInReach = (p: Principal, r: LocalRule) => {
    const projectId = r.projectId ?? (r.sessionId === undefined ? undefined : store.get(r.sessionId)?.projectId);
    return projectId === undefined ? r.sessionId === undefined : inReach(p, projectId);
  };
  const audited = (kind: "agent.rule.add" | "agent.rule.remove" | "agent.project.trust", p: Principal, details: Record<string, string>) =>
    deps.audit.append({ kind, principal: { kind: p.kind, id: p.id }, outcome: "ok", details });
  const ruleDetails = (rpc: string, r: LocalRule) => ({
    rpc,
    ruleId: r.id,
    layer: r.layer,
    value: r.effect,
    ...(r.projectId === undefined ? {} : { projectId: r.projectId }),
    ...(r.sessionId === undefined ? {} : { sessionId: r.sessionId }),
  });

  return {
    "agent.rules.list": ({ projectId, sessionId }: { projectId?: string; sessionId?: string }, options: object) =>
      guard("agent.rules.list", Effect.gen(function* () {
        const p = by(options);
        const session: Session | undefined = sessionId === undefined ? undefined : yield* sessionInReach(store, p, sessionId);
        const project = session?.projectId ?? projectId;
        if (session === undefined && project === undefined) {
          // The server's layers and every local rule the caller reaches.
          const serverData = rules.serverData;
          return {
            rules: [...managedRules(serverData), ...ruleStore.all().filter((r) => ruleInReach(p, r)), ...userRules(serverData)].map((r) => listed(r, true, r.layer === "local" && ruleStore.proves(r, undefined))),
          };
        }
        const layered = yield* sync(() => rules.layers({ ...(session === undefined ? {} : { session }), ...(project === undefined ? {} : { projectIds: [project] }) }));
        const file = project === undefined ? undefined : layered.projects.get(project);
        return {
          rules: [...layered.rules.map((r) => listed(r, true, layered.lifts?.remembered.has(r) === true)), ...(file?.ignored ?? []).map((r) => listed(r, false))],
          ...(file === undefined ? {} : { projectRules: file.status }),
          ...(file?.rulesHash === undefined ? {} : { rulesHash: file.rulesHash }),
          ...(file?.reason === undefined ? {} : { projectRulesReason: file.reason }),
        };
      })),
    "agent.rules.add": (
      req: { effect: Rule["effect"]; command: string; where?: Rule["where"]; projectId?: string; sessionId?: string },
      options: object,
    ) =>
      guard("agent.rules.add", Effect.gen(function* () {
        const p = by(options);
        const session = req.sessionId === undefined ? undefined : yield* sessionInReach(store, p, req.sessionId);
        if (session !== undefined && req.projectId !== undefined && req.projectId !== session.projectId) {
          return yield* Effect.fail(finError("INVALID_REQUEST", "the session is not in that project", { field: "projectId", reason: "otherProject" }, "name the session alone, or its own project"));
        }
        const projectId = session?.projectId ?? req.projectId;
        const now = deps.clock.now();
        const rule = yield* sync(() =>
          ruleStore.add({
            id: ulid(now),
            effect: req.effect,
            command: req.command,
            ...(req.where === undefined ? {} : { where: req.where }),
            ...(projectId === undefined ? {} : { projectId }),
            ...(session === undefined ? {} : { sessionId: session.id }),
            source: "agent.rules.add",
            createdBy: `${p.kind}:${p.id}`,
            createdAt: now,
          }),
        );
        audited("agent.rule.add", p, ruleDetails("agent.rules.add", rule));
        return { rule: listed(rule, true) };
      })),
    "agent.rules.remove": ({ id }: { id: string }, options: object) =>
      guard("agent.rules.remove", sync(() => {
        const p = by(options);
        const rule = ruleStore.get(id);
        if (rule === undefined || !ruleInReach(p, rule) || !ruleStore.remove(id)) return { removed: false };
        audited("agent.rule.remove", p, ruleDetails("agent.rules.remove", rule));
        return { removed: true };
      })),
    "agent.project.trust": ({ projectId, expectedHash }: { projectId: string; expectedHash: string }, options: object) =>
      guard("agent.project.trust", sync(() => {
        const p = by(options);
        const dir = deps.dirOf(projectId);
        if (dir === undefined) throw finError("PROJECT_NOT_FOUND", "no such project on this server", { projectId }, "open the project first");
        const { trust, rules: count } = trustProject(ruleStore, projectId, dir, expectedHash, `${p.kind}:${p.id}`, deps.clock.now());
        audited("agent.project.trust", p, { rpc: "agent.project.trust", projectId, rulesHash: trust.rulesHash });
        return { projectId, rulesHash: trust.rulesHash, rules: count, trustedAt: trust.trustedAt };
      })),
  };
}
