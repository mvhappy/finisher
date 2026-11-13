// The owner's agent settings over RPC (sub-project 7 §10, §12.7; plan 7A Decisions 24, 28, Task
// 12): the OpenRouter key, the local model endpoint and the closed-model-targets counsel gate.
// The console is their only client in 7A (the app's UI comes with sub-project 9). All are
// `agent.admin` or `settings.admin`, human-only: `local` and `agent` principals are refused,
// and the console sends the challenge code (`challenge`) for every change.
//
// No answer holds a key: `agent.key.*` names keys by id, label, provider and project only. The
// key itself travels once, in `agent.key.set`'s payload, over the console's own connection.
import { Schema } from "effect";
import { AgentWarning, LocalPreset } from "./agent.ts";
import { ChallengeField } from "./auth.ts";

const Count = Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0));
const Id = Schema.String;

/** The longest key accepted (OpenRouter's are far shorter). */
export const AGENT_KEY_MAX = 4096;
/** A key's label, as the console shows it. */
export const AGENT_KEY_LABEL_MAX = 64;

/** A stored key's metadata: never its value. */
export const ProviderKey = Schema.Struct({
  id: Id,
  provider: Schema.String,
  label: Schema.String,
  /** The project the key is for; absent for the server's default key. */
  projectId: Schema.optionalKey(Id),
  createdAt: Count,
});
export type ProviderKey = typeof ProviderKey.Type;

/**
 * `agent.key.set`: stores a key for `provider` (v1: `openrouter` only; `local` needs none, any
 * other provider is `PROVIDER_UNAVAILABLE`), replacing the one already set for the same project
 * (or the server's default when `projectId` is absent).
 */
export const AgentKeySetPayload = Schema.Struct({
  provider: Schema.String.check(Schema.isMaxLength(64)),
  // Bounded by the handler (`SECRET_INVALID`), not here: a schema refusal quotes the value.
  key: Schema.String,
  label: Schema.optionalKey(Schema.String.check(Schema.isMaxLength(AGENT_KEY_LABEL_MAX))),
  projectId: Schema.optionalKey(Id),
  challenge: ChallengeField,
});
export const AgentKeySetSuccess = Schema.Struct({ key: ProviderKey });
export const AgentKeyListSuccess = Schema.Struct({ keys: Schema.Array(ProviderKey) });
export const AgentKeyRemovePayload = Schema.Struct({ id: Id, challenge: ChallengeField });
export const AgentKeyRemoveSuccess = Schema.Struct({ removed: Schema.Boolean });

/**
 * `agent.local.set`: the local model endpoint, `ollama`, `lmstudio` (their usual loopback
 * addresses) or a base URL (`http` or `https`, a host, no credentials inside). Plain `http` to
 * another machine is accepted with a warning: prompts and outputs cross the network unencrypted.
 */
export const AgentLocalSetPayload = Schema.Struct({ endpoint: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(2048)), challenge: ChallengeField });
export const AgentLocalSetSuccess = Schema.Struct({ preset: LocalPreset, warnings: Schema.optionalKey(Schema.Array(AgentWarning)) });
/** `agent.local.show`: the owner's own setting, base URL included (no record holds it). */
export const AgentLocalShowSuccess = Schema.Struct({
  configured: Schema.Boolean,
  preset: Schema.optionalKey(LocalPreset),
  baseUrl: Schema.optionalKey(Schema.String),
});
/** `agent.local.test`: whether the endpoint answers, and the models it lists (`GET /models`). */
export const AgentLocalTestSuccess = Schema.Struct({
  configured: Schema.Boolean,
  reachable: Schema.Boolean,
  models: Schema.optionalKey(Schema.Array(Schema.String)),
  problem: Schema.optionalKey(Schema.Struct({ code: Schema.String, message: Schema.String, fix: Schema.optionalKey(Schema.String) })),
});
export const AgentLocalRemovePayload = Schema.Struct({ challenge: ChallengeField });
export const AgentLocalRemoveSuccess = Schema.Struct({ removed: Schema.Boolean });

/** An account's keyed hash as `agent.providers` reports it (8 hex characters; never the label). */
const AccountHash = Schema.String.check(Schema.isPattern(/^[0-9a-f]{8}$/));
/**
 * `agent.account.confirm` (plan 7D Decision 19): a person at the console takes the CLI's new
 * account as the one sessions run on (`next`: the next change, for `provider signin --confirm`).
 */
export const AgentAccountConfirmPayload = Schema.Struct({ harness: Schema.Literal("claudeCode"), next: Schema.optionalKey(Schema.Boolean), challenge: ChallengeField });
export const AgentAccountConfirmSuccess = Schema.Struct({
  /** A change was waiting and is now the account (false: nothing waited, or `next`). */
  confirmed: Schema.Boolean,
  /** `next`: the next change will be taken without a confirmation. */
  expecting: Schema.optionalKey(Schema.Boolean),
  accountHash: Schema.optionalKey(AccountHash),
  previous: Schema.optionalKey(AccountHash),
});

/**
 * The counsel gate (spec §12.7; Owner decision 1): whether closed-model outputs may become
 * training targets. `pendingCounsel` (the default) and `excluded` both drop them; nothing in
 * sub-project 7 reads it to drop data, the dataset builder and any upload path must.
 */
export const ClosedModelTargets = Schema.Literals(["pendingCounsel", "cleared", "excluded"]);
export type ClosedModelTargets = typeof ClosedModelTargets.Type;

export const TrainingClosedTargetsSuccess = Schema.Struct({ state: ClosedModelTargets });
export const TrainingClosedTargetsSetPayload = Schema.Struct({ state: ClosedModelTargets, challenge: ChallengeField });
