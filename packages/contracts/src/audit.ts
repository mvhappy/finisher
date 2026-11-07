// The security audit log's records (spec §3.3, plan 5A Decision 4). Rows live in
// `control.db` `audit`; they are read only by the owner (`auth.audit.list`, Task 8).
import { Schema } from "effect";

/** Every audit kind (spec §3.3); the last six are reserved for sub-projects 7 and 8. */
export const AUDIT_KINDS = [
  "auth.fail",
  "auth.denied",
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
