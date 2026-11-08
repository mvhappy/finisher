// Agent rules (sub-project 7B Decisions 3-5; spec §7.2): who may run which `fin` command.
// A rule names a command glob, an effect, and optionally the conditions it applies under.
// Rules come from five layers; evaluation (a `deny` anywhere wins, then `ask`, then `allow`) is
// the server's (`agent/rules.ts`); this file is only the shape, shared by the roles' data, the
// settings files and the admin RPCs.
import { Schema } from "effect";
import { Mode } from "./agent-events.ts";

const Count = Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0));

export const RuleEffect = Schema.Literals(["allow", "ask", "deny"]);
export type RuleEffect = typeof RuleEffect.Type;

/** Where a rule lives, from the one that cannot be overridden down to a role's own. */
export const RuleLayer = Schema.Literals(["managed", "local", "project", "user", "role"]);
export type RuleLayer = typeof RuleLayer.Type;

/**
 * A command glob: segments of a command name (`render`, `asset.add`, `plugin:@acme/*`, tool
 * rules `tool:Bash(curl *)`), `*`
 * inside one segment and `**` across them. Matching is the server's; this bounds the text.
 */
export const RuleCommand = Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(200), Schema.isPattern(/^[A-Za-z0-9_.:@/*() -]+$/));

/** The conditions a rule applies under; all that are present must hold (resolved values). */
export const RuleWhere = Schema.Struct({
  /** A branch reference, resolved per project. */
  branch: Schema.optionalKey(Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(200))),
  mode: Schema.optionalKey(Mode),
  /** The model preset of the session (`openrouter`, `local`). */
  preset: Schema.optionalKey(Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(64))),
  bytesOver: Schema.optionalKey(Count),
  /** Micro-units of the session's currency. */
  costOver: Schema.optionalKey(Count),
  minutesOver: Schema.optionalKey(Count),
});
export type RuleWhere = typeof RuleWhere.Type;

export const Rule = Schema.Struct({
  id: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(128)),
  effect: RuleEffect,
  command: RuleCommand,
  where: Schema.optionalKey(RuleWhere),
  layer: RuleLayer,
  /** Where it came from: `role:<name>`, a file path, or an approval id. */
  source: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(256)),
  /** The person who added it, when a person did. */
  createdBy: Schema.optionalKey(Schema.String),
  /** Milliseconds since the epoch. */
  createdAt: Count,
});
export type Rule = typeof Rule.Type;
