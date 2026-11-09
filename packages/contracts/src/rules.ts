// Agent rules (sub-project 7B Decisions 3-5; spec §7.2): who may run which `fin` command.
// A rule names a command glob, an effect, and optionally the conditions it applies under.
// Rules come from five layers; evaluation (a `deny` anywhere wins, then `ask`, then `allow`) is
// the server's (`agent/rules.ts`); this file is only the shape, shared by the roles' data, the
// settings files and the admin RPCs.
import { Schema } from "effect";
import { Mode } from "./agent-events.ts";
import { ChallengeField } from "./auth.ts";

const Count = Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0));

export const RuleEffect = Schema.Literals(["allow", "ask", "deny"]);
export type RuleEffect = typeof RuleEffect.Type;

/** Where a rule lives, from the one that cannot be overridden down to a role's own. */
export const RuleLayer = Schema.Literals(["managed", "local", "project", "user", "role"]);
export type RuleLayer = typeof RuleLayer.Type;

/**
 * A command glob: segments of a command name (`render`, `asset.add`), `*` inside one segment
 * and `**` across them; `plugin:@acme/*`, `rpc:*` and tool rules `tool:Bash(curl *)` are free
 * text. Matching is the server's; this bounds the text. Command, plugin and RPC names are
 * lowercase, so an uppercase letter outside a `tool:` rule (tool names are capitalised) is
 * refused: such a pattern could match nothing.
 */
export const RuleCommand = Schema.String.check(
  Schema.isMinLength(1),
  Schema.isMaxLength(200),
  Schema.isPattern(/^[A-Za-z0-9_.:@/*() -]+$/),
  Schema.isPattern(/^(?:tool:.*|[^A-Z]*)$/, { message: "a command pattern is lowercase (only a tool: rule names a capitalised tool)" }),
);

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
  /**
   * An `asset.add` of a download without a recorded licence (plan 7B Decision 5): 7G records
   * downloads and fills it; until then it never holds.
   */
  unlicensedDownload: Schema.optionalKey(Schema.Literal(true)),
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

// The rule RPCs (sub-project 7B Decision 7): reading them is `agent.read`; adding and removing
// `local` rules and trusting a project's file are the owner's (`agent.admin`, people only,
// with the typed code). No agent credential can write any layer.

/** Whether a project's `finisher.rules.json` applies: none, trusted, untrusted or changed since it was trusted. */
export const ProjectRulesStatus = Schema.Literals(["none", "trusted", "untrusted", "changed"]);
export type ProjectRulesStatus = typeof ProjectRulesStatus.Type;

/** A rule as listed: where it applies (`local` rules) and whether it applies now. */
export const ListedRule = Schema.Struct({
  ...Rule.fields,
  projectId: Schema.optionalKey(Schema.String),
  sessionId: Schema.optionalKey(Schema.String),
  /** False for a project rule while the project is untrusted or its file changed. */
  applies: Schema.Boolean,
  /** A person's `remember` wrote this allow (the server's record proves it): it lifts a matching ask of its project. */
  remembered: Schema.optionalKey(Schema.Literal(true)),
});
export type ListedRule = typeof ListedRule.Type;

/**
 * `agent.rules.list`: the rules of every layer that apply to a project (or to one session,
 * with its role's), or every local rule and the server's when neither is named.
 */
export const AgentRulesListPayload = Schema.Struct({
  projectId: Schema.optionalKey(Schema.String),
  sessionId: Schema.optionalKey(Schema.String),
});
/** The hash a person trusts: `sha256:<hex>` of the project file's bytes, `none` for no file. */
export const RulesHash = Schema.String.check(Schema.isPattern(/^(?:sha256:[0-9a-f]{64}|none)$/));

export const AgentRulesListSuccess = Schema.Struct({
  rules: Schema.Array(ListedRule),
  /** The project's file, when a project is named (by `projectId` or the session's). */
  projectRules: Schema.optionalKey(ProjectRulesStatus),
  /** The file's hash now (what `agent.project.trust` binds); absent when it cannot be read. */
  rulesHash: Schema.optionalKey(RulesHash),
  /** Why the file does not apply as it is: deleted since trusted, unreadable, or not rules. */
  projectRulesReason: Schema.optionalKey(Schema.Literals(["deleted", "unreadable", "invalidRules"])),
});

/** `agent.rules.add`: a `local` rule for every agent call, one project's or one session's. */
export const AgentRulesAddPayload = Schema.Struct({
  effect: RuleEffect,
  command: RuleCommand,
  where: Schema.optionalKey(RuleWhere),
  projectId: Schema.optionalKey(Schema.String),
  sessionId: Schema.optionalKey(Schema.String),
  challenge: ChallengeField,
});
export const AgentRulesAddSuccess = Schema.Struct({ rule: ListedRule });

/** `agent.rules.remove`: a `local` rule by id (`removed: false` when there is none). */
export const AgentRulesRemovePayload = Schema.Struct({ id: Schema.String, challenge: ChallengeField });
export const AgentRulesRemoveSuccess = Schema.Struct({ removed: Schema.Boolean });

/**
 * `agent.project.trust` (`finisherd project trust <id>`): trusts the project's
 * `finisher.rules.json` as the person saw it: `expectedHash` is the `rulesHash` listed to them,
 * and a file changed since is `INVALID_REQUEST {field: expectedHash, reason: changed}` (a later
 * change stops it applying again).
 */
export const AgentProjectTrustPayload = Schema.Struct({
  projectId: Schema.String,
  expectedHash: RulesHash,
  /** Also trust `prompts/*.json` as the person saw it (`agent.savedPrompt.list`'s `promptsHash`); absent: they stay untrusted. */
  expectedPromptsHash: Schema.optionalKey(RulesHash),
  challenge: ChallengeField,
});
export const AgentProjectTrustSuccess = Schema.Struct({
  projectId: Schema.String,
  rulesHash: Schema.String,
  /** How many rules the file holds. */
  rules: Count,
  /** The prompt files' combined hash, when `expectedPromptsHash` trusted them. */
  promptsHash: Schema.optionalKey(Schema.String),
  trustedAt: Count,
});
