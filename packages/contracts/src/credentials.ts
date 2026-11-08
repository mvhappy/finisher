// Agent and MCP credentials (spec §4.1, §4.5, §4.8; plan 5A Task 9, Decisions 32–34): minted by
// the owner from the console, kept in `control.db` `credentials` without their key (it is
// derived from `credential.master`), handed to a harness as a credential file that `fin` and the
// MCP adapter find through `FIN_CREDENTIAL`.
import { Schema } from "effect";
import { ChallengeField } from "./auth.ts";
import { type Scope, ScopeSchema } from "./scopes.ts";

export const CREDENTIAL_KINDS = ["agent", "mcp"] as const;
export const CredentialKindSchema = Schema.Literals(CREDENTIAL_KINDS);
export type CredentialKind = typeof CredentialKindSchema.Type;

/**
 * What `finisherd agent mint` grants when no `--scope` is given (spec §4.5, Decision 33): never
 * `branch.protected`, `review.decide`, `jobs.control`, `project.admin`, `project.create` or
 * `fonts.fetch` unless asked for, and never a human-only scope.
 */
export const MINT_DEFAULT_SCOPES: ReadonlyArray<Scope> = [
  "server.read",
  "project.read",
  "project.write",
  "review.write",
  "jobs.read",
  "jobs.submit",
  "library.read",
  "library.scan",
  "diagnostics.read",
  "diagnostics.record",
];

/** Expiry defaults per kind, in seconds (spec §4.1): `null` is no absolute limit. */
export const CREDENTIAL_EXPIRY: { readonly [K in CredentialKind]: { readonly idleS: number; readonly absoluteS: number | null } } = {
  agent: { idleS: 24 * 3600, absoluteS: 7 * 24 * 3600 },
  mcp: { idleS: 30 * 24 * 3600, absoluteS: null },
};

/** The shortest and longest expiry a mint may ask for, in seconds. */
export const CREDENTIAL_MIN_EXPIRY_S = 60;
export const CREDENTIAL_MAX_EXPIRY_S = 366 * 24 * 3600;

/** A label: what the owner calls the harness (≤ 64 characters, no control characters). */
export const CREDENTIAL_LABEL_MAX = 64;

/** A branch rule: `"own"` (branches the credential created) or a branch id or name. */
export const BranchRuleSchema = Schema.String;

/**
 * A credential as the owner sees it (`auth.credential.list`, `mint`): never its key. Times are
 * ms since the epoch; `status` is `live`, `revoked` (by the owner or itself) or `expired`.
 */
export const CredentialSchema = Schema.Struct({
  id: Schema.String,
  kind: CredentialKindSchema,
  label: Schema.String,
  scopes: Schema.Array(ScopeSchema),
  projects: Schema.Union([Schema.Literal("*"), Schema.Array(Schema.String)]),
  branches: Schema.Union([Schema.Literal("*"), Schema.Array(BranchRuleSchema)]),
  createdBy: Schema.String,
  createdAt: Schema.Number,
  lastUsedAt: Schema.optional(Schema.Number),
  idleExpiryS: Schema.Number,
  absoluteExpiryAt: Schema.optional(Schema.Number),
  revokedAt: Schema.optional(Schema.Number),
  status: Schema.Literals(["live", "revoked", "expired"]),
});
export type Credential = typeof CredentialSchema.Type;

/**
 * The credential file (spec §4.1): `{"v":1,"kind","id","key","env"}`, written 0600 by the console
 * (or by sub-project 7 inside a session's sandbox). `key` is base64url of 32 bytes; `env` is the
 * server's environment id, which the handshake binds.
 */
export const CredentialFileSchema = Schema.Struct({
  v: Schema.Literal(1),
  kind: CredentialKindSchema,
  id: Schema.String,
  key: Schema.String,
  env: Schema.String,
});
export type CredentialFile = typeof CredentialFileSchema.Type;

/**
 * `auth.credential.mint` (`auth.admin`, a typed code from the console): `scopes` default to
 * `MINT_DEFAULT_SCOPES` (a human-only one is `SCOPE_DENIED`); `projects` default to every
 * project (`"*"`), `branches` to every branch (`["own"]` or ids narrow them); `idleS` and
 * `absoluteS` (`null`: none) default per kind. The answer carries the file's contents once.
 */
export const CredentialMintPayload = Schema.Struct({
  kind: CredentialKindSchema,
  label: Schema.String,
  scopes: Schema.optional(Schema.Array(ScopeSchema)),
  projects: Schema.optional(Schema.Array(Schema.String)),
  branches: Schema.optional(Schema.Array(BranchRuleSchema)),
  idleS: Schema.optional(Schema.Number),
  absoluteS: Schema.optional(Schema.NullOr(Schema.Number)),
  challenge: ChallengeField,
});
export const CredentialMintSuccess = Schema.Struct({ credential: CredentialSchema, file: CredentialFileSchema });

/** `auth.credential.list` (`auth.admin`, read-only): live credentials, or every one with `all`. */
export const CredentialListPayload = Schema.Struct({ all: Schema.optional(Schema.Boolean) });
export const CredentialListSuccess = Schema.Struct({ credentials: Schema.Array(CredentialSchema) });

/**
 * `auth.credential.revoke {id}` (`auth.admin`, a typed code): the one revoke path — the store,
 * the credential's tickets, its live connections and streams, and its writes in flight.
 * `revoked: false` when it was not live (unknown, already revoked or expired).
 */
export const CredentialRevokePayload = Schema.Struct({ id: Schema.String, challenge: ChallengeField });
export const CredentialRevokeSuccess = Schema.Struct({ revoked: Schema.Boolean });

/**
 * `auth.credential.rotateMaster {}` (`auth.admin`, a typed code; review of Tasks 8–9, M3): a new
 * `credential.master` is written, so no key derived from the old one proves again, and every
 * credential not ended yet is revoked through the one revoke path. `ended` counts them. The
 * remedy when `credential.master` may have been copied, or an old `control.db` was restored
 * (which brings back rows that were revoked after that copy).
 */
export const CredentialRotateMasterPayload = Schema.Struct({ challenge: ChallengeField });
export const CredentialRotateMasterSuccess = Schema.Struct({ ended: Schema.Number });
