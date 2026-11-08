// The `local` rule layer and project trust in `control.db` (sub-project 7B Decisions 4, 6 and 7;
// agent migration 0004): the only module that reads or writes `agent_rules` and `project_trust`.
//
// - **Local rules** are each person's own: they never leave the server and always apply (no
//   trust). A rule with neither `projectId` nor `sessionId` applies to every agent call, one with
//   `projectId` to that project's, one with `sessionId` to that session's. Only people write
//   them (`agent.rules.add|remove`, human-only, and 7B's remembered answers).
//   A session's own rules go with it (`AgentStore`'s delete and retention sweep remove its
//   `agent_rules` rows with the session's other rows).
// - **Trust** records the hash of a project's `finisher.rules.json` a person trusted
//   (`trust.ts`), so a changed file stops applying until it is trusted again.
import type { DatabaseSync, StatementSync } from "node:sqlite";
import { type Rule, RuleWhere } from "@finisher/contracts";
import { Schema } from "effect";

/** A local rule as it is added: the store fills `layer` and keeps the rest. */
export interface NewLocalRule {
  readonly id: string;
  readonly effect: Rule["effect"];
  readonly command: string;
  readonly where?: Rule["where"];
  readonly projectId?: string;
  readonly sessionId?: string;
  readonly source: string;
  readonly createdBy?: string;
  readonly createdAt: number;
}

/** A local rule with where it applies. */
export type LocalRule = Rule & { readonly projectId?: string; readonly sessionId?: string };

export interface ProjectTrust {
  readonly projectId: string;
  readonly rulesHash: string;
  readonly hooksHash?: string;
  readonly trustedAt: number;
  readonly by: string;
}

interface RuleRow {
  id: string;
  projectId: string | null;
  sessionId: string | null;
  effect: string;
  command: string;
  where: string | null;
  source: string;
  createdBy: string | null;
  createdAt: number;
}

const COLUMNS = `id, projectId, sessionId, effect, command, "where", source, createdBy, createdAt`;
const decodeWhere = Schema.decodeUnknownSync(RuleWhere);

const SQL = {
  insert: `INSERT INTO agent_rules (${COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  remove: "DELETE FROM agent_rules WHERE id = ?",
  get: `SELECT ${COLUMNS} FROM agent_rules WHERE id = ?`,
  all: `SELECT ${COLUMNS} FROM agent_rules ORDER BY createdAt, id`,
  applicable: `SELECT ${COLUMNS} FROM agent_rules WHERE (projectId IS NULL OR projectId = ?) AND (sessionId IS NULL OR sessionId = ?) ORDER BY createdAt, id`,
  trustOf: "SELECT projectId, rulesHash, hooksHash, trustedAt, by FROM project_trust WHERE projectId = ?",
  trust: `INSERT INTO project_trust (projectId, rulesHash, trustedAt, by) VALUES (?, ?, ?, ?)
    ON CONFLICT (projectId) DO UPDATE SET rulesHash = excluded.rulesHash, trustedAt = excluded.trustedAt, by = excluded.by`,
} as const;

function toRule(r: RuleRow): LocalRule {
  return {
    id: r.id,
    effect: r.effect as Rule["effect"],
    command: r.command,
    ...(r.where === null ? {} : { where: decodeWhere(JSON.parse(r.where)) }),
    layer: "local",
    source: r.source,
    ...(r.createdBy === null ? {} : { createdBy: r.createdBy }),
    createdAt: r.createdAt,
    ...(r.projectId === null ? {} : { projectId: r.projectId }),
    ...(r.sessionId === null ? {} : { sessionId: r.sessionId }),
  };
}

export class RuleStore {
  readonly #s: Record<keyof typeof SQL, StatementSync>;

  /** Over `control.db` (the agent chain's migration 0004 made the tables). */
  constructor(db: DatabaseSync) {
    this.#s = Object.fromEntries(Object.entries(SQL).map(([k, sql]) => [k, db.prepare(sql)])) as Record<keyof typeof SQL, StatementSync>;
  }

  add(r: NewLocalRule): LocalRule {
    this.#s.insert.run(
      r.id,
      r.projectId ?? null,
      r.sessionId ?? null,
      r.effect,
      r.command,
      r.where === undefined ? null : JSON.stringify(r.where),
      r.source,
      r.createdBy ?? null,
      r.createdAt,
    );
    return this.get(r.id)!;
  }

  get(id: string): LocalRule | undefined {
    const row = this.#s.get.get(id) as unknown as RuleRow | undefined;
    return row === undefined ? undefined : toRule(row);
  }

  /** Whether a rule was removed. */
  remove(id: string): boolean {
    return Number(this.#s.remove.run(id).changes) > 0;
  }

  /** Every local rule, oldest first. */
  all(): LocalRule[] {
    return (this.#s.all.all() as unknown as RuleRow[]).map(toRule);
  }

  /** The local rules that apply to a call in `projectId` (when known) by `sessionId` (when a session's). */
  applicable(projectId: string | undefined, sessionId: string | undefined): LocalRule[] {
    // `''` is no id: a missing project or session matches only the rules that name none.
    return (this.#s.applicable.all(projectId ?? "", sessionId ?? "") as unknown as RuleRow[]).map(toRule);
  }

  trustOf(projectId: string): ProjectTrust | undefined {
    const r = this.#s.trustOf.get(projectId) as { projectId: string; rulesHash: string; hooksHash: string | null; trustedAt: number; by: string } | undefined;
    if (r === undefined) return undefined;
    return { projectId: r.projectId, rulesHash: r.rulesHash, ...(r.hooksHash === null ? {} : { hooksHash: r.hooksHash }), trustedAt: r.trustedAt, by: r.by };
  }

  trust(projectId: string, rulesHash: string, by: string, at: number): ProjectTrust {
    this.#s.trust.run(projectId, rulesHash, at, by);
    return this.trustOf(projectId)!;
  }
}
