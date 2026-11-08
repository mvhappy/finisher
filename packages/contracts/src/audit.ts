// The security audit log's records (spec §3.3, plan 5A Decision 4). Rows live in
// `control.db` `audit`; they are read only by the owner (`auth.audit.list`, Task 8).
import { Schema } from "effect";

/** Every audit kind (spec §3.3); the last six are reserved for sub-projects 7 and 8. */
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
  "grant",
  "approval",
  "hook.block",
  "hook.modify",
  "sandbox.violation",
  "plugin.install",
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
// auth-failure kinds, addresses (`address`) only for pairing and auth-failure kinds.

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
  /** The transport a connection came over (5A Task 3). */
  transport: Schema.optional(Schema.Literals(["socket", "pipe", "ws"])),
};
const NAMES = { deviceName: Schema.optional(Name), label: Schema.optional(Name) };
const ADDRESS = { address: Schema.optional(Address) };

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

const PlainDetails = Schema.Struct(COMMON);
const NamedDetails = Schema.Struct({ ...COMMON, ...NAMES });
const AddressedDetails = Schema.Struct({ ...COMMON, ...NAMES, ...ADDRESS });
type DetailsSchema = typeof PlainDetails | typeof NamedDetails | typeof AddressedDetails;

function detailsSchema(kind: AuditKind): DetailsSchema {
  if (AUDIT_ADDRESS_KINDS.has(kind)) return AddressedDetails;
  return AUDIT_NAME_KINDS.has(kind) ? NamedDetails : PlainDetails;
}

/** Each kind's closed `details` schema. */
export const AUDIT_DETAILS: { readonly [K in AuditKind]: DetailsSchema } = Object.fromEntries(
  AUDIT_KINDS.map((k) => [k, detailsSchema(k)]),
) as { readonly [K in AuditKind]: DetailsSchema };

const DECODE = {
  plain: Schema.decodeUnknownExit(PlainDetails),
  named: Schema.decodeUnknownExit(NamedDetails),
  addressed: Schema.decodeUnknownExit(AddressedDetails),
};

/** True when `details` fits `kind`'s schema exactly (no other field). */
export function auditDetailsValid(kind: AuditKind, details: unknown): boolean {
  const decode = AUDIT_ADDRESS_KINDS.has(kind) ? DECODE.addressed : AUDIT_NAME_KINDS.has(kind) ? DECODE.named : DECODE.plain;
  return decode(details, { onExcessProperty: "error" })._tag === "Success";
}
