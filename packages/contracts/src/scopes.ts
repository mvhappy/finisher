// The closed scope vocabulary (spec §4.5, plan 5A Decision 18) and each principal kind's
// default scopes (Decision 20). Task 5 adds every RPC's rule (`RPC_SCOPES`) beside these.
import { Schema } from "effect";

export const SCOPES = [
  "public",
  "server.read",
  "project.read",
  "project.create",
  "project.write",
  "project.admin",
  "project.confirm",
  "fonts.fetch",
  "branch.protected",
  "review.write",
  "review.decide",
  "jobs.read",
  "jobs.submit",
  "jobs.control",
  "library.read",
  "library.scan",
  "library.admin",
  "diagnostics.read",
  "diagnostics.record",
  "diagnostics.admin",
  "settings.admin",
  "auth.admin",
  "plugin.install",
] as const;

export const ScopeSchema = Schema.Literals(SCOPES);
export type Scope = typeof ScopeSchema.Type;

/** Scopes only a person holds (spec §4.6): never `local`, `agent` or `mcp`. */
export const HUMAN_ONLY: ReadonlySet<Scope> = new Set<Scope>([
  "project.confirm",
  "library.admin",
  "diagnostics.admin",
  "settings.admin",
  "auth.admin",
  "plugin.install",
]);
