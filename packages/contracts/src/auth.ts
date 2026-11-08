// Principals and the handshake (spec §3.2, §4.2; plan 5A Decisions 6–9). Nonces and MACs are
// base64url without padding on the wire; keys never are.
import { Schema } from "effect";
import { AuditEntrySchema, AuditKindSchema } from "./audit.ts";
import { HUMAN_ONLY, SCOPES, type Scope, ScopeSchema } from "./scopes.ts";

export const PrincipalKind = Schema.Literals(["anonymous", "local", "console", "device", "agent", "mcp", "plugin"]);
export type PrincipalKind = typeof PrincipalKind.Type;

export const PrincipalSchema = Schema.Struct({
  kind: PrincipalKind,
  id: Schema.String,
  scopes: Schema.Array(ScopeSchema),
  projects: Schema.Union([Schema.Literal("*"), Schema.Array(Schema.String)]),
  branches: Schema.Union([Schema.Literal("*"), Schema.Array(Schema.String)]),
  human: Schema.Boolean,
});
export type Principal = typeof PrincipalSchema.Type;

/**
 * The credential a hello names: `local` (`fin`, `<server-data>/auth/local.key`), `console`
 * (`finisherd <command>`, `console.key`), or an `agent`/`mcp` credential by id (its key is
 * derived from `credential.master`).
 */
export const CredentialRefSchema = Schema.Union([
  Schema.Struct({ kind: Schema.Literals(["local", "console"]) }),
  Schema.Struct({ kind: Schema.Literals(["agent", "mcp"]), id: Schema.String }),
]);
export type CredentialRef = typeof CredentialRefSchema.Type;

/**
 * `server.hello` (Decision 8). Without `clientNonce` it is protocol 1's hello. With one, the
 * answer carries `envId`, the canonical `endpoint`, a `serverNonce` and, when the named
 * credential is live and `env`/`endpoint` (if sent) are this server's, the `proof`.
 * `credential` defaults to `{kind: "local"}`.
 */
export const ServerHelloPayload = Schema.Struct({
  clientNonce: Schema.optional(Schema.String),
  env: Schema.optional(Schema.String),
  endpoint: Schema.optional(Schema.String),
  credential: Schema.optional(CredentialRefSchema),
});
export const ServerHelloSuccess = Schema.Struct({
  protocolVersion: Schema.Number,
  serverVersion: Schema.String,
  capabilities: Schema.Array(Schema.String),
  envId: Schema.optional(Schema.String),
  endpoint: Schema.optional(Schema.String),
  serverNonce: Schema.optional(Schema.String),
  proof: Schema.optional(Schema.String),
});

/** `auth.prove {mac}` (Decision 9): binds the hello's credential to the connection. */
export const AuthProvePayload = Schema.Struct({ mac: Schema.String });
export const AuthProveSuccess = Schema.Struct({ principal: PrincipalSchema });

/** The longest and the default life of a WebSocket ticket, in seconds (Decision 23). */
export const TICKET_MAX_TTL_S = 60;
export const TICKET_DEFAULT_TTL_S = 30;

/**
 * `auth.ticket {ttlS?}` (spec §4.3, Decision 23): a one-time ticket for one WebSocket upgrade,
 * bound to the caller's principal. `ttlS` is capped at 60; `expiresAt` is in ms since the epoch.
 */
export const AuthTicketPayload = Schema.Struct({ ttlS: Schema.optional(Schema.Number) });
export const AuthTicketSuccess = Schema.Struct({ ticket: Schema.String, expiresAt: Schema.Number });
/** `auth.whoami`: the principal this connection runs as. */
export const AuthWhoamiSuccess = Schema.Struct({ principal: PrincipalSchema });

/** How long a console challenge stays valid, in seconds (Decision 30). */
export const CHALLENGE_TTL_S = 60;

/**
 * The typed confirmation of a mutating human-only RPC from a `console` principal (Decision
 * 30): the code `console.challenge` issued to the same connection, typed back by the person.
 * Single use; ignored for every other principal.
 */
export const ChallengeField = Schema.optional(Schema.String);

/**
 * `console.challenge {}` (console principals only): a fresh 6-character Crockford code,
 * valid for 60 s on this connection only; `expiresAt` in ms since the epoch.
 */
export const ConsoleChallengeSuccess = Schema.Struct({ code: Schema.String, expiresAt: Schema.Number });

/**
 * `server.status {}` (spec §4.8; `server.read`): the server's version, protocol and
 * environment, its listeners, live connections per principal kind, whether the journal
 * records, audit writes that failed since start, what the job host enforces (5C-2: `jobs.caps`,
 * `JobEngine.workers().caps`) and standing warnings (`{code, …}`; `SANDBOX_DISABLED` while job
 * workers run unconfined).
 */
export const JobHostCapsSchema = Schema.Struct({
  killTree: Schema.String,
  watchdog: Schema.Boolean,
  cgroupMemory: Schema.Boolean,
  leafLimits: Schema.Boolean,
  sandbox: Schema.String,
  sandboxed: Schema.Boolean,
});
export const ServerStatusSuccess = Schema.Struct({
  version: Schema.String,
  protocol: Schema.Number,
  envId: Schema.String,
  pid: Schema.Number,
  listeners: Schema.Array(Schema.Struct({ transport: Schema.Literals(["socket", "pipe", "ws"]), endpoint: Schema.String })),
  connections: Schema.Record(Schema.String, Schema.Number),
  journal: Schema.Struct({ enabled: Schema.Boolean }),
  auditFailures: Schema.Number,
  jobs: Schema.Struct({ caps: JobHostCapsSchema }),
  warnings: Schema.Array(Schema.Record(Schema.String, Schema.Unknown)),
});

/** `server.stop {}` (`auth.admin`; a service command, spec §4.7): answers, then stops gracefully. */
export const ServerStopSuccess = Schema.Struct({ stopping: Schema.Literal(true) });

/** At most this many audit entries per `auth.audit.list` page. */
export const AUDIT_PAGE_MAX = 1000;

/**
 * `auth.audit.list` (`auth.admin`, read-only): newest first; `since` in ms since the epoch;
 * `cursor`: a page's `next`; `limit` 1–1000 (default 100).
 */
export const AuditListPayload = Schema.Struct({
  since: Schema.optional(Schema.Number),
  kind: Schema.optional(AuditKindSchema),
  cursor: Schema.optional(Schema.String),
  limit: Schema.optional(Schema.Number),
});
export const AuditListSuccess = Schema.Struct({ entries: Schema.Array(AuditEntrySchema), next: Schema.optional(Schema.String) });

/**
 * `auth.audit.purge` (`auth.admin`, confirmed): every entry (`all: true`) or those before
 * `before` (ms since the epoch), exactly one of the two. The purge itself is audited
 * (`audit.purge {reason: all|before, count}`) after it, so the log never ends empty.
 */
export const AuditPurgePayload = Schema.Struct({
  all: Schema.optional(Schema.Boolean),
  before: Schema.optional(Schema.Number),
  challenge: ChallengeField,
});
export const AuditPurgeSuccess = Schema.Struct({ removed: Schema.Number });

/**
 * The WebSocket subprotocols (spec §4.3): a client offers `finisher.v2` and one
 * `finisher.ticket.<ticket>`; the server answers `finisher.v2`.
 */
export const WS_PROTOCOL_PREFIX = "finisher.v";
export const WS_TICKET_PREFIX = "finisher.ticket.";

/** The RPCs a connection may call before it proves (spec §4.2). */
export const PUBLIC_RPCS: ReadonlySet<string> = new Set(["server.hello", "auth.prove"]);

/**
 * A kind's default scopes (Decision 20): `console` every scope; `local` every scope but the
 * human-only ones; `agent`/`mcp` none by default (their credential lists them); `anonymous` only
 * `public`. `device` (5B) and `plugin` (8) hold none yet.
 */
export function defaultScopes(kind: PrincipalKind): ReadonlyArray<Scope> {
  switch (kind) {
    case "console":
      return SCOPES;
    case "local":
      return SCOPES.filter((s) => !HUMAN_ONLY.has(s));
    case "anonymous":
      return ["public"];
    default:
      return [];
  }
}
