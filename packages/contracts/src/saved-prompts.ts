// Saved prompts (sub-project 7B Task 10, Decision 15; spec §6.4): `/make-shorts 5` and similar
// are data a person starts, never an agent. A prompt names a role and a mode, a template whose
// `{{name}}` slots are typed parameters (no expressions, no code), and optionally a budget.
// They come from the bundle, the owner's `agents.savedPrompts` and a trusted project's
// `prompts/*.json`; expansion and validation are the server's (`agent/saved-prompts.ts`).
import { Schema } from "effect";
import { BudgetSpec, Harness, Mode } from "./agent-events.ts";
import { AgentSessionStartSuccess, BuiltinPreset } from "./agent.ts";
import { ProjectRulesStatus, RulesHash } from "./rules.ts";

const Slug = Schema.String.check(Schema.isPattern(/^[a-z][a-z0-9]*(-[a-z0-9]+)*$/), Schema.isMaxLength(64));
const Text = Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(200));
const Whole = Schema.Number.check(Schema.isInt());

/** The longest template, in characters, and most parameters and enum values. */
export const PROMPT_TEMPLATE_MAX = 20_000;
export const PROMPT_PARAMS_MAX = 16;
export const PROMPT_VALUES_MAX = 32;
/** The longest string parameter a run may give, in characters. */
export const PROMPT_STRING_MAX = 2_000;

/** A typed parameter: `int` (optionally `min`/`max`), `string`, or `enum` (one of `values`). */
export const SavedPromptParam = Schema.Struct({
  name: Schema.String.check(Schema.isPattern(/^[a-z][A-Za-z0-9]{0,31}$/)),
  type: Schema.Literals(["int", "string", "enum"]),
  values: Schema.optionalKey(Schema.Array(Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(64))).check(Schema.isMaxLength(PROMPT_VALUES_MAX))),
  min: Schema.optionalKey(Whole),
  max: Schema.optionalKey(Whole),
  default: Schema.optionalKey(Schema.Union([Whole, Schema.String.check(Schema.isMaxLength(PROMPT_STRING_MAX))])),
});
export type SavedPromptParam = typeof SavedPromptParam.Type;

export const SavedPrompt = Schema.Struct({
  name: Slug,
  title: Text,
  role: Slug,
  mode: Mode,
  template: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(PROMPT_TEMPLATE_MAX)),
  params: Schema.Array(SavedPromptParam).check(Schema.isMaxLength(PROMPT_PARAMS_MAX)),
  budget: Schema.optionalKey(BudgetSpec),
});
export type SavedPrompt = typeof SavedPrompt.Type;

/** Where a prompt comes from; a clash resolves settings over project over bundle. */
export const SavedPromptSource = Schema.Literals(["bundle", "settings", "project"]);
export type SavedPromptSource = typeof SavedPromptSource.Type;

export const ListedSavedPrompt = Schema.Struct({ ...SavedPrompt.fields, source: SavedPromptSource });
export type ListedSavedPrompt = typeof ListedSavedPrompt.Type;

/** `agent.savedPrompt.list`: the prompts a project can run (its own only once trusted). */
export const AgentSavedPromptListPayload = Schema.Struct({ projectId: Schema.optionalKey(Schema.String) });
export const AgentSavedPromptListSuccess = Schema.Struct({
  prompts: Schema.Array(ListedSavedPrompt),
  /** A project's prompts that do not apply (untrusted, or changed since trusted), for a person to read before trusting them. */
  ignored: Schema.optionalKey(Schema.Array(ListedSavedPrompt)),
  /** Prompts that did not load (settings or project files), with why: never silently dropped. */
  invalid: Schema.optionalKey(Schema.Array(Schema.Struct({ source: Schema.String, name: Schema.optionalKey(Schema.String), reason: Schema.String }))),
  /** The project's `prompts/*.json` as a whole: none, trusted, untrusted, or changed since trusted. */
  projectPrompts: Schema.optionalKey(ProjectRulesStatus),
  /** Their combined hash now (what `agent.project.trust` binds as `expectedPromptsHash`). */
  promptsHash: Schema.optionalKey(RulesHash),
});

/**
 * `agent.savedPrompt.run`: expands the prompt with `params` (an int may come as digits) and
 * starts a session of its role and mode with the expansion as its first prompt. The starter's
 * own choices of harness, preset, model and title apply as for `agent.session.start`.
 */
export const AgentSavedPromptRunPayload = Schema.Struct({
  projectId: Schema.String,
  name: Schema.String,
  params: Schema.optionalKey(Schema.Record(Schema.String, Schema.Union([Whole, Schema.String]))),
  harness: Schema.optionalKey(Harness),
  provider: Schema.optionalKey(BuiltinPreset),
  model: Schema.optionalKey(Schema.String),
  title: Schema.optionalKey(Schema.String.check(Schema.isMaxLength(200))),
});
export const AgentSavedPromptRunSuccess = AgentSessionStartSuccess;
