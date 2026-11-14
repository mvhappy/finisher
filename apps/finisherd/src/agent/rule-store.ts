// The `local` rule layer and project trust in `control.db` (sub-project 7B Decisions 4, 6 and 7;
// agent migration 0004): the only module that reads or writes `agent_rules` and `project_trust`.
//
// - **Local rules** are each person's own: they never leave the server and always apply (no
//   trust). A rule with neither `projectId` nor `sessionId` applies to every agent call, one with
//   `projectId` to that project's, one with `sessionId` to that session's. Only people write
//   them (`agent.rules.add|remove`, human-only, and 7B's remembered answers).
//   A session's own rules go with it (`AgentStore`'s delete and retention sweep remove its
//   `agent_rules` rows with the session's other rows).
// - **Remembered** (`agent_remembered`, agent migration 0006): the record of each rule a
//   person's `remember` wrote (`remember.ts`), written only by `add(…, from)` and `remembered`,
//   never by an RPC that takes a rule from its caller. It is what proves a remembered `allow`
//   for the lift (`rules.ts`); a rule's `source` text proves nothing.
// - **Trust** records the hash of a project's `finisher.rules.json` a person trusted
//   (`trust.ts`), so a changed file stops applying until it is trusted again; with it, that of
//   its saved prompts (`promptsHash`) and its `finisher.hooks.json` (`hooksHash`, 7F), each
//   recorded only when the person trusted it too.
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

/** The record of a rule a `remember` wrote (agent migration 0006). */
export interface RememberedRule {
  readonly ruleId: string;
  readonly layer: "local" | "project";
  readonly projectId: string;
  /** A `session`-scoped local rule's session. */
  readonly sessionId?: string;
  /** Only an `allow` is recorded (only an allow lifts). */
  readonly effect: "allow";
  readonly command: string;
  readonly approvalId: string;
  readonly createdAt: number;
}

export interface ProjectTrust {
  readonly projectId: string;
  readonly rulesHash: string;
  readonly hooksHash?: string;
  /** The trusted `prompts/*.json` as a whole (7B Task 10); absent: the project's prompt files are not trusted. */
  readonly promptsHash?: string;
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
  record: "INSERT INTO agent_remembered (ruleId, layer, projectId, sessionId, effect, command, approvalId, createdAt) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
  forget: "DELETE FROM agent_remembered WHERE layer = ? AND projectId = ? AND ruleId = ?",
  forgetLocal: "DELETE FROM agent_remembered WHERE layer = 'local' AND ruleId = ?",
  proves: "SELECT 1 AS ok FROM agent_remembered WHERE layer = ? AND projectId = ? AND ruleId = ? AND effect = 'allow' AND command = ? AND sessionId IS ?",
  projectRecords: "SELECT ruleId, effect, command FROM agent_remembered WHERE layer = 'project' AND projectId = ?",
  get: `SELECT ${COLUMNS} FROM agent_rules WHERE id = ?`,
  all: `SELECT ${COLUMNS} FROM agent_rules ORDER BY createdAt, id`,
  applicable: `SELECT ${COLUMNS} FROM agent_rules WHERE (projectId IS NULL OR projectId = ?) AND (sessionId IS NULL OR sessionId = ?) ORDER BY createdAt, id`,
  trustOf: "SELECT projectId, rulesHash, hooksHash, promptsHash, trustedAt, by FROM project_trust WHERE projectId = ?",
  trustPrompts: "UPDATE project_trust SET promptsHash = ? WHERE projectId = ?",
  trustHooks: "UPDATE project_trust SET hooksHash = ? WHERE projectId = ?",
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
  readonly #db: DatabaseSync;

  /** Over `control.db` (the agent chain's migration 0004 made the tables). */
  constructor(db: DatabaseSync) {
    this.#db = db;
    this.#s = Object.fromEntries(Object.entries(SQL).map(([k, sql]) => [k, db.prepare(sql)])) as Record<keyof typeof SQL, StatementSync>;
  }

  /**
   * Adds a local rule; with `from` (the approval a `remember` answered) also its record, in one
   * transaction, so the rule never stands without the record that proves it (or the reverse).
   * Only an `allow` has a record (only an allow lifts).
   */
  add(r: NewLocalRule, from?: string): LocalRule {
    if (from === undefined) return this.#insert(r);
    if (r.effect !== "allow") throw new Error("only a remembered allow is recorded");
    if (r.projectId === undefined) throw new Error("a remembered rule names its project");
    const projectId = r.projectId;
    return this.#atomic(() => {
      const rule = this.#insert(r);
      this.remembered({ ruleId: r.id, layer: "local", projectId, ...(r.sessionId === undefined ? {} : { sessionId: r.sessionId }), effect: "allow", command: r.command, approvalId: from, createdAt: r.createdAt });
      return rule;
    });
  }

  /** Records a rule a `remember` wrote (a project file's, or through `add`). */
  remembered(m: RememberedRule): void {
    this.#s.record.run(m.ruleId, m.layer, m.projectId, m.sessionId ?? null, m.effect, m.command, m.approvalId, m.createdAt);
  }

  /**
   * Drops the records of a project's remembered rules that `rules` (the file being trusted) no
   * longer holds as written: the same id, effect and command, no `where`. A rule a person took
   * out and wrote back by hand is theirs, not a `remember`'s.
   */
  keepRemembered(projectId: string, rules: readonly Rule[]): void {
    const rows = this.#s.projectRecords.all(projectId) as unknown as Array<{ ruleId: string; effect: string; command: string }>;
    for (const row of rows) {
      const held = rules.some((r) => r.id === row.ruleId && r.effect === row.effect && r.command === row.command && r.where === undefined);
      if (!held) this.forget(projectId, row.ruleId);
    }
  }

  /** Drops the record of a project file's remembered rule (its write was taken back). */
  forget(projectId: string, ruleId: string): void {
    this.#s.forget.run("project", projectId, ruleId);
  }

  /**
   * Whether `rule` is exactly what a `remember` wrote: an `allow` with no `where`, and an
   * `allow` record of its id, layer, project, session and command. A local rule is checked against its own
   * row's project and session; a project file's against the project whose file it is in.
   */
  proves(rule: Rule | LocalRule, projectId: string | undefined): boolean {
    if (rule.effect !== "allow" || rule.where !== undefined) return false;
    const l = rule as LocalRule;
    if (rule.layer === "local") return l.projectId !== undefined && this.#s.proves.get("local", l.projectId, rule.id, rule.command, l.sessionId ?? null) !== undefined;
    if (rule.layer === "project") return projectId !== undefined && this.#s.proves.get("project", projectId, rule.id, rule.command, null) !== undefined;
    return false;
  }

  #atomic<A>(f: () => A): A {
    this.#db.exec("SAVEPOINT remembered");
    try {
      const out = f();
      this.#db.exec("RELEASE remembered");
      return out;
    } catch (e) {
      this.#db.exec("ROLLBACK TO remembered");
      this.#db.exec("RELEASE remembered");
      throw e;
    }
  }

  #insert(r: NewLocalRule): LocalRule {
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
    this.#s.forgetLocal.run(id);
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
    const r = this.#s.trustOf.get(projectId) as { projectId: string; rulesHash: string; hooksHash: string | null; promptsHash: string | null; trustedAt: number; by: string } | undefined;
    if (r === undefined) return undefined;
    return { projectId: r.projectId, rulesHash: r.rulesHash, ...(r.hooksHash === null ? {} : { hooksHash: r.hooksHash }), ...(r.promptsHash === null ? {} : { promptsHash: r.promptsHash }), trustedAt: r.trustedAt, by: r.by };
  }

  trust(projectId: string, rulesHash: string, by: string, at: number): ProjectTrust {
    this.#s.trust.run(projectId, rulesHash, at, by);
    return this.trustOf(projectId)!;
  }

  /** Records the trusted prompt files' hash on the project's trust record (made by `trust`). */
  trustPrompts(projectId: string, promptsHash: string): ProjectTrust {
    this.#s.trustPrompts.run(promptsHash, projectId);
    return this.trustOf(projectId)!;
  }

  /** Records the trusted hooks file's hash on the project's trust record (made by `trust`; 7F). */
  trustHooks(projectId: string, hooksHash: string): ProjectTrust {
    this.#s.trustHooks.run(hooksHash, projectId);
    return this.trustOf(projectId)!;
  }
}
