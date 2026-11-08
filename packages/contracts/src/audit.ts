// The security audit log's records (spec §3.3, plan 5A Decision 4). Rows live in
// `control.db` `audit`; they are read only by the owner (`auth.audit.list`, Task 8).
import { Schema } from "effect";
import { PLATFORMS, ROUTE_KINDS, SETTINGS_PATHS } from "./devices.ts";

/**
 * Every audit kind (spec §3.3); the six after `console.refused` are reserved for sub-projects 7
 * and 8, and the `plugin.*` and `registry.*` kinds are sub-project 8's (8A Decision 28).
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
  // A new identity key for the environment (5B Decision 30): every device was revoked with it.
  "identity.rotate",
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
] as const;

export const AuditKindSchema = Schema.Literals(AUDIT_KINDS);
export type AuditKind = typeof AuditKindSchema.Type;

export const AuditOutcomeSchema = Schema.Literals(["ok", "denied", "failed"]);
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
/** An SPKI pin: `base64url(SHA-256(SubjectPublicKeyInfo))`, 43 characters (5B Decision 6). */
const Spki = Schema.String.check(Schema.isPattern(/^[A-Za-z0-9_-]{43}$/));
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
  /** The transport a connection came over (5A Task 3; `wss`: 5B's remote listener). */
  transport: Schema.optional(Schema.Literals(["socket", "pipe", "ws", "wss"])),
};
// 5B: the remote listener (`listener.start {spki, mode, source}`), the environment
// (`env.reset {devices, credentials}`), `settings.write {path}` and `identity.rotate {reason,
// count}`. A route kind and a platform are closed words; never an address outside the
// address kinds, never a setting's value.
const SERVER = {
  spki: Schema.optional(Spki),
  mode: Schema.optional(Schema.Literals(["selfSigned", "files"])),
  source: Schema.optional(Schema.Literals(["env", "settings"])),
  path: Schema.optional(Schema.Literals(SETTINGS_PATHS)),
  devices: Schema.optional(Schema.Int),
  credentials: Schema.optional(Schema.Int),
  platform: Schema.optional(Schema.Literals(PLATFORMS)),
  route: Schema.optional(Schema.Literals(ROUTE_KINDS)),
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

const PAIRING: ReadonlyArray<AuditKind> = ["pair.offer", "pair.redeem", "pair.approve", "pair.reject", "pair.expire"];
/** Kinds whose details may name a device or credential. */
export const AUDIT_NAME_KINDS: ReadonlySet<AuditKind> = new Set<AuditKind>([
  ...PAIRING,
  "device.revoke",
  "credential.revoke",
  "auth.fail",
]);
/** Kinds whose details may hold a remote address (owner decision 4 of 5B: pairing, revocation, failures). */
export const AUDIT_ADDRESS_KINDS: ReadonlySet<AuditKind> = new Set<AuditKind>([...PAIRING, "device.revoke", "auth.fail"]);

/** Kinds whose details name a library by id. */
export const AUDIT_LIBRARY_KINDS: ReadonlySet<AuditKind> = new Set<AuditKind>(["library.add", "library.remove", "library.set"]);

const PlainDetails = Schema.Struct({ ...COMMON, ...SERVER });
const PluginDetails = Schema.Struct({ ...COMMON, ...PLUGIN_FIELDS });
const LibraryDetails = Schema.Struct({ ...COMMON, ...LIBRARY });
const NamedDetails = Schema.Struct({ ...COMMON, ...SERVER, ...NAMES });
const AddressedDetails = Schema.Struct({ ...COMMON, ...SERVER, ...NAMES, ...ADDRESS });
type DetailsSchema = typeof PlainDetails | typeof LibraryDetails | typeof NamedDetails | typeof AddressedDetails | typeof PluginDetails;

/** Kinds whose details may name a plugin, a registry and a version. */
export const AUDIT_PLUGIN_KINDS: ReadonlySet<AuditKind> = new Set<AuditKind>(AUDIT_KINDS.filter((k) => k.startsWith("plugin.") || k.startsWith("registry.")));

function detailsSchema(kind: AuditKind): DetailsSchema {
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
  plugin: Schema.decodeUnknownExit(PluginDetails),
  plain: Schema.decodeUnknownExit(PlainDetails),
  library: Schema.decodeUnknownExit(LibraryDetails),
  named: Schema.decodeUnknownExit(NamedDetails),
  addressed: Schema.decodeUnknownExit(AddressedDetails),
};

/** True when `details` fits `kind`'s schema exactly (no other field). */
export function auditDetailsValid(kind: AuditKind, details: unknown): boolean {
  const decode = AUDIT_PLUGIN_KINDS.has(kind)
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
