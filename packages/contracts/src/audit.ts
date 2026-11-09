// The security audit log's records (spec §3.3, plan 5A Decision 4). Rows live in
// `control.db` `audit`; they are read only by the owner (`auth.audit.list`, Task 8).
import { Schema } from "effect";

/**
 * Every audit kind (spec §3.3); the six after `console.refused` are reserved for sub-projects 7
 * and 8, the `plugin.*` and `registry.*` kinds are sub-project 8's (8A Decision 28), and
 * `agent.rule.*` and `agent.project.trust` are 7B's (Decisions 7 and 12).
 */
export const AUDIT_KINDS = [
  "auth.fail",
  "auth.denied",
  // Reserved: 5A Task 3 audited unproved connections run as `local` under it; since Task 4 no
  // connection runs unproved, so nothing writes it. Kept so that rows written then still read.
  "auth.legacy",
  "auth.ticket",
  "credential.mint",
  "credential.revoke",
  "credential.expire",
  // A new `credential.master` (review of 5A Tasks 8–9, M3): `count` credentials ended with it.
  "credential.rotateMaster",
  "pair.offer",
  "pair.redeem",
  "pair.approve",
  "pair.reject",
  "pair.expire",
  "device.revoke",
  "device.rename",
  "consent.change",
  "diagnostics.purge",
  "settings.write",
  "project.takeoverConfirmed",
  "project.relocated",
  "listener.start",
  "listener.stop",
  "env.moved",
  "env.adopted",
  "env.reset",
  "control.reset",
  "library.add",
  "library.remove",
  "library.set",
  "history.drop",
  "review.merge",
  "console.refused",
  // The owner purged the audit log (5A Task 8): written after the purge, with its count.
  "audit.purge",
  "grant",
  "approval",
  "hook.block",
  "hook.modify",
  "sandbox.violation",
  "plugin.install",
  "plugin.update",
  "plugin.remove",
  "plugin.enable",
  "plugin.disable",
  "plugin.revoke",
  "plugin.grant",
  "registry.add",
  "registry.remove",
  "registry.trust",
  "registry.forget",
  "registry.verifyFail",
  // Command rules (7B Decisions 7 and 12): a `local` rule added (by a person, or a remembered
  // answer: `from`) or removed, and a project's rules file trusted (its `rulesHash`, and the `promptsHash` of its saved prompts).
  "agent.rule.add",
  "agent.rule.remove",
  "agent.project.trust",
] as const;

export const AuditKindSchema = Schema.Literals(AUDIT_KINDS);
export type AuditKind = typeof AuditKindSchema.Type;

/** `pending`: a request waits for a person's answer on a durable card (7B's `APPROVAL_PENDING`), not denied. */
export const AuditOutcomeSchema = Schema.Literals(["ok", "denied", "failed", "pending"]);
export type AuditOutcome = typeof AuditOutcomeSchema.Type;

/**
 * One row. `count` > 1 only for aggregated `auth.fail` rows (one per credential kind and
 * remote address per minute). `details` never holds secrets, tokens, MACs, nonces, links,
 * paths or request payloads.
 */
export const AuditEntrySchema = Schema.Struct({
  id: Schema.Number,
  at: Schema.Number,
  kind: Schema.String,
  principalKind: Schema.String,
  principalId: Schema.String,
  credentialKind: Schema.optional(Schema.String),
  outcome: AuditOutcomeSchema,
  code: Schema.optional(Schema.String),
  remote: Schema.optional(Schema.String),
  count: Schema.Number,
  details: Schema.Record(Schema.String, Schema.Unknown),
});
export type AuditEntry = typeof AuditEntrySchema.Type;

// Closed `details` per kind (review of 5A Task 1): every kind takes only ids, RPC tags,
// scopes, rule and reason words and counts, so a name, a path or an address cannot slip into
// a row. Names (`deviceName`, `label`) are possible only for pairing, revocation and
// auth-failure kinds, addresses (`address`, and a local pipe client's `peer`) only for pairing
// and auth-failure kinds.

/** A dotted lowerCamel word: an RPC tag (`history.cherryPick`), a scope, a rule, a reason. */
const Word = Schema.String.check(Schema.isPattern(/^[a-z][a-zA-Z0-9]{0,47}(\.[a-z][a-zA-Z0-9]{0,47}){0,3}$/));
/** A ULID-shaped id (credentials, projects, devices). */
const Id = Schema.String.check(Schema.isPattern(/^[0-9A-Za-z]{26}$/));
/** An error code. */
const Code = Schema.String.check(Schema.isPattern(/^[A-Z][A-Z0-9_]{0,63}$/));
/** A person's text: a device name or credential label, ≤ 64 characters. */
const Name = Schema.String.check(Schema.isMaxLength(64));
/** A remote address (5B), ≤ 64 characters. */
const Address = Schema.String.check(Schema.isMaxLength(64));
/** A Windows SID (`S-1-…`), never an account name. */
const Sid = Schema.String.check(Schema.isPattern(/^S-1-[0-9]+(-[0-9]+){0,15}$/));
/** The process and token user of a pipe client (5A Task 11), as the OS reports them. */
const Peer = Schema.Struct({ pid: Schema.optional(Schema.Int), sid: Schema.optional(Sid) });

const COMMON = {
  rpc: Schema.optional(Word),
  scope: Schema.optional(Word),
  scopes: Schema.optional(Schema.Array(Word)),
  rule: Schema.optional(Word),
  reason: Schema.optional(Word),
  purpose: Schema.optional(Word),
  value: Schema.optional(Word),
  code: Schema.optional(Code),
  credentialId: Schema.optional(Id),
  projectId: Schema.optional(Id),
  deviceId: Schema.optional(Id),
  count: Schema.optional(Schema.Int),
  /** `audit.purge`: the cutoff (ms) an owner's purge removed entries before. */
  before: Schema.optional(Schema.Int),
  /** The transport a connection came over (5A Task 3). */
  transport: Schema.optional(Schema.Literals(["socket", "pipe", "ws", "agentTcp"])),
};
// `library.add|remove|set` (plan 5D Decision 49): the library's id, its storage kind and the
// field names a human-only `library.set` changed; never a library's name, root, alias or path.
const LIBRARY = {
  libraryId: Schema.optional(Id),
  storage: Schema.optional(Schema.Literals(["local", "network", "sync"])),
  fields: Schema.optional(Schema.Array(Word)),
};
const NAMES = { deviceName: Schema.optional(Name), label: Schema.optional(Name) };
/** A plugin id (`@publisher/name`), a registry id and a version (8A Decision 28). */
const PluginIdWord = Schema.String.check(Schema.isPattern(/^@[a-z0-9][a-z0-9-]{1,38}\/[a-z0-9][a-z0-9-]{1,63}$/));
const RegistryIdWord = Schema.String.check(Schema.isPattern(/^[a-z0-9][a-z0-9-]{0,63}$/));
const VersionWord = Schema.String.check(Schema.isMaxLength(64), Schema.isPattern(/^[0-9][0-9A-Za-z.+-]*$/));
const Sha256Word = Schema.String.check(Schema.isPattern(/^[0-9a-f]{64}$/));
/**
 * What `plugin.*` and `registry.*` rows may say: ids, a version, counts, a risk word, an error
 * code, a root's SHA-256 (`registry.forget`/`trust`) and the third-party-code acceptance. Never
 * a path, a publisher's name or a manifest field.
 */
const PLUGIN_FIELDS = {
  pluginId: Schema.optional(PluginIdWord),
  version: Schema.optional(VersionWord),
  registryId: Schema.optional(RegistryIdWord),
  rootSha256: Schema.optional(Sha256Word),
  permissionsAdded: Schema.optional(Schema.Int),
  /** An automatic update (a person's refresh took a newer version that widens nothing). */
  auto: Schema.optional(Schema.Boolean),
  /** The plan an install tried to use. */
  planId: Schema.optional(Schema.String.check(Schema.isMaxLength(64), Schema.isPattern(/^[0-9A-Za-z]+$/))),
  risk: Schema.optional(Schema.Literals(["low", "medium", "high"])),
  thirdPartyCode: Schema.optional(Schema.Struct({ accepted: Schema.Boolean, confined: Schema.Boolean })),
};
const ADDRESS = { address: Schema.optional(Address), peer: Schema.optional(Peer) };
/**
 * What `agent.rule.*` and `agent.project.trust` rows may say: the rule's id and layer, the
 * session it is scoped to, the approval it was remembered from, and a trusted file's hash.
 * Never the command pattern or a path (the effect is `value`, the project `projectId`).
 */
const RULE_FIELDS = {
  ruleId: Schema.optional(Id),
  layer: Schema.optional(Schema.Literals(["managed", "local", "project", "user", "role"])),
  sessionId: Schema.optional(Id),
  from: Schema.optional(Id),
  rulesHash: Schema.optional(Schema.String.check(Schema.isPattern(/^(?:sha256:[0-9a-f]{64}|none)$/))),
  promptsHash: Schema.optional(Schema.String.check(Schema.isPattern(/^(?:sha256:[0-9a-f]{64}|none)$/))),
};

/** What an `auth.denied` row may say besides: the card an ask opened (`APPROVAL_PENDING`, 7B Decision 8). */
const DENIAL_FIELDS = { approval: Schema.optional(Id) };

/**
 * `sandbox.violation` (spec §8.8; plan 7C Decision 24): what the launcher or the host observed,
 * for a session or a job: the operation, the profile and the kind of root it touched, never a
 * path (a keyed hash when one is needed).
 */
const SANDBOX = {
  session: Schema.optional(Id),
  jobId: Schema.optional(Id),
  op: Schema.optional(Word),
  profile: Schema.optional(Schema.Literals(["agent", "hook", "worker", "plugin"])),
  rootKind: Schema.optional(Word),
  pathHash: Schema.optional(Schema.String.check(Schema.isPattern(/^[0-9a-f]{16,64}$/))),
};

const PAIRING: ReadonlyArray<AuditKind> = ["pair.offer", "pair.redeem", "pair.approve", "pair.reject", "pair.expire"];
/** Kinds whose details may name a device or credential. */
export const AUDIT_NAME_KINDS: ReadonlySet<AuditKind> = new Set<AuditKind>([
  ...PAIRING,
  "device.revoke",
  "device.rename",
  "credential.revoke",
  "auth.fail",
]);
/** Kinds whose details may hold a remote address. */
export const AUDIT_ADDRESS_KINDS: ReadonlySet<AuditKind> = new Set<AuditKind>([...PAIRING, "auth.fail"]);

/** Kinds whose details name a library by id. */
export const AUDIT_LIBRARY_KINDS: ReadonlySet<AuditKind> = new Set<AuditKind>(["library.add", "library.remove", "library.set"]);

const PlainDetails = Schema.Struct(COMMON);
const PluginDetails = Schema.Struct({ ...COMMON, ...PLUGIN_FIELDS });
const RuleDetails = Schema.Struct({ ...COMMON, ...RULE_FIELDS });
const DenialDetails = Schema.Struct({ ...COMMON, ...DENIAL_FIELDS });
const SandboxDetails = Schema.Struct({ ...COMMON, ...SANDBOX });
const LibraryDetails = Schema.Struct({ ...COMMON, ...LIBRARY });
const NamedDetails = Schema.Struct({ ...COMMON, ...NAMES });
const AddressedDetails = Schema.Struct({ ...COMMON, ...NAMES, ...ADDRESS });
type DetailsSchema = typeof PlainDetails | typeof LibraryDetails | typeof NamedDetails | typeof AddressedDetails | typeof PluginDetails | typeof RuleDetails | typeof DenialDetails | typeof SandboxDetails;

/** Kinds whose details may name a plugin, a registry and a version. */
export const AUDIT_PLUGIN_KINDS: ReadonlySet<AuditKind> = new Set<AuditKind>(AUDIT_KINDS.filter((k) => k.startsWith("plugin.") || k.startsWith("registry.")));

/** Kinds whose details name a command rule or a trusted rules file. */
export const AUDIT_RULE_KINDS: ReadonlySet<AuditKind> = new Set<AuditKind>(["agent.rule.add", "agent.rule.remove", "agent.project.trust"]);

function detailsSchema(kind: AuditKind): DetailsSchema {
  if (kind === "sandbox.violation") return SandboxDetails;
  if (kind === "auth.denied") return DenialDetails;
  if (AUDIT_RULE_KINDS.has(kind)) return RuleDetails;
  if (AUDIT_PLUGIN_KINDS.has(kind)) return PluginDetails;
  if (AUDIT_ADDRESS_KINDS.has(kind)) return AddressedDetails;
  if (AUDIT_LIBRARY_KINDS.has(kind)) return LibraryDetails;
  return AUDIT_NAME_KINDS.has(kind) ? NamedDetails : PlainDetails;
}

/** Each kind's closed `details` schema. */
export const AUDIT_DETAILS: { readonly [K in AuditKind]: DetailsSchema } = Object.fromEntries(
  AUDIT_KINDS.map((k) => [k, detailsSchema(k)]),
) as { readonly [K in AuditKind]: DetailsSchema };

const DECODE = {
  sandbox: Schema.decodeUnknownExit(SandboxDetails),
  rule: Schema.decodeUnknownExit(RuleDetails),
  denial: Schema.decodeUnknownExit(DenialDetails),
  plugin: Schema.decodeUnknownExit(PluginDetails),
  plain: Schema.decodeUnknownExit(PlainDetails),
  library: Schema.decodeUnknownExit(LibraryDetails),
  named: Schema.decodeUnknownExit(NamedDetails),
  addressed: Schema.decodeUnknownExit(AddressedDetails),
};

/** True when `details` fits `kind`'s schema exactly (no other field). */
export function auditDetailsValid(kind: AuditKind, details: unknown): boolean {
  const decode = kind === "sandbox.violation"
    ? DECODE.sandbox
    : kind === "auth.denied"
    ? DECODE.denial
    : AUDIT_RULE_KINDS.has(kind)
    ? DECODE.rule
    : AUDIT_PLUGIN_KINDS.has(kind)
      ? DECODE.plugin
      : AUDIT_ADDRESS_KINDS.has(kind)
        ? DECODE.addressed
        : AUDIT_LIBRARY_KINDS.has(kind)
          ? DECODE.library
          : AUDIT_NAME_KINDS.has(kind)
            ? DECODE.named
            : DECODE.plain;
  return decode(details, { onExcessProperty: "error" })._tag === "Success";
}
