# Sub-project 5A: Principals, local credentials and scopes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (the handshake and its crypto, private directories, the connection-aware protocol, scopes and the authorisation wrapper, sources and actors, agent credentials, validation), `sonnet` for plumbing (the control store, tickets and the WebSocket, console commands, `fin`, hardening, e2e, docs).

**Goal:** every caller of `finisherd` is a principal with a credential, every RPC declares the scope it needs, and the human-only operations are enforced by the server. After this plan:
- `<server-data>/control.db` (`node:sqlite`) holds the control plane's state, starting with the always-on, local **audit log** and **agent sessions**;
- a connection proves a credential with a **mutual HMAC handshake** (`server.hello` + `auth.prove`) against a key file in server data, so `fin` refuses a squatted pipe or a stale `finisherd.json` (`SERVER_UNTRUSTED`), and a connection without a credential can only say hello;
- principals `local` (`fin`), `console` (`finisherd <command>`), `agent` (credential files; minted from the console until sub-project 7) and `anonymous`; `device` and `plugin` are reserved for 5B and 8;
- the WebSocket's bearer token is gone: one-time **tickets** bound to the principal that minted them, Origin and Host checks;
- a closed **scope** vocabulary, one scope per RPC plus payload escalations, checked by one wrapper before any handler runs; **human-only** operations (consent changes, content export, purges, clearing a never-collect flag, confirmed takeovers, relocations, audit, agent minting, `server.stop`) answer `HUMAN_ONLY` (exit 7) to `fin` and work from the console;
- the diagnostics journal takes its **source and actor from the principal** (a new `console` source);
- **protocol 2**; `fin` exit code **7 = denied**; `fin whoami`;
- the "Follow-ups for sub-project 5" in `TASKS.md`: private server data on Windows, pipe squatting, a graceful stop without SIGTERM, decode failures as `FinError`, server-side `INVALID_PATH`, a frame-size cap and a client deadline, `fin`'s test hooks out of release builds.

**Architecture:**
- **`@finisher/contracts`** gains `auth.ts` (the `Principal`, `Scope` and handshake schemas, the new RPCs) and `scopes.ts` (the scope table for every RPC, escalations, human-only and per-kind defaults).
- **`finisherd`** gains:
  - `control/` (store, migrations, audit), `auth/` (keys, handshake, tickets, agent sessions), `protocol.ts` (one connection-aware `RpcServer.Protocol` for both transports), `connections.ts`, `authorize.ts` (the wrapper);
  - `console/` (the `finisherd <command>` CLI over the local socket with the console key);
  - changes to `main.ts` (keys before listen; console dispatch), `transport.ts` (no bearer token; tickets), `handlers.ts` (one `authorizeAll` call; new handlers), `config.ts` (`finisherd.json` without a token, private directories), `diagnostics.ts` (D4's file: sources and actors from principals).
- **`fin-node`** gains `checkPrivateDir`/`makePrivateDir` (Windows DACLs through `windows-sys`; a no-op on unix, where `config.ts` keeps its mode checks).
- **`fin-core::events`** gains `Source::Console` and the new RPC names in `RPC_METHODS`.
- **`fin`** gains `auth.rs` (credential discovery and the handshake), exit class 7, `fin whoami`, a frame-size cap and an overall deadline, and its test hooks behind a `test-hooks` feature.

**Tech Stack:**
- Node 26.12 (`node:sqlite`, `node:crypto`), Effect 4.0.6 (`effect/rpc`), `ws` 8 (MIT; already installed under `@effect/platform-node`, now a direct, pinned dependency of `finisherd`), vitest.
- Rust 2024: `hmac` 0.12 and `sha2` 0.10 (MIT OR Apache-2.0) in `fin`, `base64` 0.22 (MIT OR Apache-2.0; check whether it is already in the lock file), `windows-sys` 0.61 (already in the tree) in `fin-node`.
- **No other new dependency.** No JOSE, TLS, keychain or certificate crates in 5A (they are 5B's).

**Spec:** `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` §1–4 (5A), §3 (shared foundations), §10–12. Parent: arch §4 rule 6, §8.4, §16, §21.1, §27.12.5. Related: the diagnostics plan (`docs/superpowers/plans/2026-11-07-diagnostics-and-training-data-hooks.md`) Decisions 14a, 14b, 23a, 24, 30, 32.

**Branch:** `sp5a-principals`, stacked on `sp4c-edit-breadth` **after diagnostics Tasks D1–D7 and 4C Task 29**. This plan changes D4's per-transport `RpcSource` and D6's actor headers, so it must not start before they exist. If 4C's merge order changes, rebase onto whatever holds D7.

**Out of scope:** pairing, devices, DPoP, TLS, remote listeners, environment routes, capability URLs, allowed roots for remote principals (5B); jobs (5C); libraries (5D); reviews and protected branches (5E, though `branch.protected` and `review.decide` exist as scope names here); agent sessions created by an agent runtime and the OS sandbox (sub-project 7); plugin credentials (8); the frontend (9).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code. `pnpm gates` runs cargo-deny and the JS licence gate over what each task adds.
- **Primitive ops live only in `fin-core`.** This plan adds none. `fin-core` stays pure (`Source::Console` is data).
- **TypeScript model types are generated, never hand-written.** Types that cross Rust ↔ TS (the event `Source`) are regenerated with `pnpm --filter @finisher/core-types generate`; the drift gate is `scripts/check-generated-types.mjs`. Control-plane records owned by TS (`Principal`, audit entries, agent sessions) are Effect Schemas in `@finisher/contracts`, and their TS types are `Schema.Type` of those schemas, never separate interfaces (as the RPC envelopes are today).
- **Dependency rules:** `fin` links no TLS or HTTP stack; `fin-node` gains only `windows-sys` features; `finisherd` adds only `ws` directly.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. Near the limit: `apps/finisherd/src/projects.ts` 520 (call-site lines only), `crates/fin/src/client.rs` 525 (call-site lines only: the handshake goes into `auth.rs`), `crates/fin/src/main.rs` 489, `crates/fin/src/backend.rs` 493. New logic goes into new files.
- Errors are `{ code, message, details, fix? }`. New codes go into `crates/fin/src/output.rs` with the exit codes of spec §4.10 and Decision 13, and into `@finisher/contracts`' error-code list.
- **No secret in a tracked file, a log line, a journal entry or an audit row.** Keys, MACs, tickets and nonces are never printed, not even at debug level. Test vectors use fixed, obviously fake keys.

**Testing and process**
- Tests never use the network. Every handshake test uses a temp server data.
- Crypto agreement between TS and Rust is pinned by one shared vector file, `tests/fixtures/auth/handshake.json` (fixed keys and nonces → expected proofs and MACs), read by a vitest test and a Rust test.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`. Tasks that touch TS or `fin` also run `rtk pnpm e2e` once at the end.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh`.
- **The repo is public:** no host names, IPs, user names or key names in tracked files, fixtures included.
- **Tracker:** each task ticks its line in `TASKS.md` under a new heading "Sub-project 5A: principals, local credentials and scopes", in the same commit. Task 12 moves the "Follow-ups for sub-project 5" lines it closes.
- **Batches.** Each batch must be green before the next starts:
  - A, store and crypto: Tasks 1–2;
  - B, connections and scopes: Tasks 3–5;
  - C, consumers: Tasks 6–10 (Tasks 8 and 10 may run beside 6, 7 and 9);
  - D, e2e, docs and validation: Tasks 11–13.

## Decisions taken here (the spec fixes the shape; these fix the details)

### Store

1. **`control.db` is opened with `node:sqlite` `DatabaseSync`**, `journal_mode = WAL`, `synchronous = NORMAL`, `foreign_keys = ON`, `busy_timeout = 250`. A statement that waits past it is `CONTROL_DB_BUSY` (exit 5). The file is created 0600 (umask 0177 around the open, as `makeLocalSocketServer` does for the socket).
2. **Versioning:** `meta(key TEXT PRIMARY KEY, value TEXT)` with `formatVersion` 1, `minReaderVersion` 1, `serverId`, `createdAt`. Migrations are frozen modules `control/migrations/m0001.ts` (exporting `n` and `up(db)` with literal SQL). Before the first pending migration, `VACUUM INTO control.db.pre-m<n>.bak` (the newest two kept). A `minReaderVersion` above ours fails the start with `CONTROL_DB_TOO_NEW`.
3. **`PRAGMA quick_check` at open**; a failure moves the file (and `-wal`, `-shm`) to `control.db.corrupt-<ulid>`, starts empty, and writes `audit {kind: "control.reset", details: {moved}}` as the first row.
4. **Audit rows:** `audit(id TEXT PRIMARY KEY, at INTEGER, kind TEXT, principalKind TEXT, principalId TEXT, outcome TEXT, code TEXT, remote TEXT, count INTEGER, details TEXT)`, index on `at` and `kind`. `auth.fail` rows aggregate: at most one row per `(kind, connection)` per minute, with `count`. Retention: 180 days or 100,000 rows, swept at start and hourly. `remote` stays `NULL` in 5A (only local transports exist).

### Keys and the handshake

5. **Key files:** `<server-data>/auth/` is a private directory (0700; owner-only DACL on Windows). `local.key` and `console.key` hold base64url (no padding) of 32 random bytes plus `\n`, 0600, written temp-then-rename **before** either transport listens, at every start. `agent.master` is created once and kept. Readers trim whitespace and refuse anything that does not decode to 32 bytes.
6. **Labels and MACs** (HMAC-SHA-256, raw bytes concatenated, nonces as their 32 decoded bytes):
   - `proof = HMAC(K, "finisherd/hello/v1" ‖ clientNonce ‖ serverNonce)`;
   - `mac = HMAC(K, "fin/prove/v1" ‖ serverNonce ‖ clientNonce)`;
   - `agentKey(id) = HMAC(agentMaster, "fin/agent-key/v1" ‖ id)` (the ULID's 26 ASCII bytes).
   - Comparison is constant time (`timingSafeEqual`; `subtle`-style equality in Rust through `hmac`'s `verify_slice`).
7. **`server.hello {clientNonce?, credential?}`.** `credential` defaults to `{kind: "local"}`; `{kind: "console"}` and `{kind: "agent", id}` name the others. With a `clientNonce`, the answer carries `serverNonce` and `proof` for the named credential; an unknown or revoked agent id gets a `serverNonce` and no `proof`, so a client cannot tell a revoked id from a missing one. Without a `clientNonce` (a protocol-1 style `{}`), the answer has no proof and the connection stays anonymous.
8. **`auth.prove {mac}`** binds the principal to the connection and answers `{principal}`. A second `auth.prove` on a proved connection is `INVALID_REQUEST`. Five failures close the connection; each is audited (aggregated, Decision 4).
9. **`fin` sends `server.hello` and `auth.prove` on every connection** before its first request. It verifies `proof` first and sends nothing else on a missing or wrong proof (`SERVER_UNTRUSTED`, exit 7, `details {endpoint, reason: noProof|badProof}`, fix: "another process is serving this socket; stop it and start finisherd").
10. **Credential discovery in `fin`:** `FIN_CREDENTIAL` (a path to an agent credential file, spec §4.1), else `<server-data>/auth/local.key` (the server data `fin` already resolves). Unreadable or malformed → no credential: `fin` still connects, and any RPC but the hello answers `UNAUTHENTICATED` with `details.reason: noCredential` and a fix naming both sources. `FIN_SERVER` alone (no server data) means "no credential" unless `FIN_CREDENTIAL` is set.

### Connections, principals and scopes

11. **One protocol for both transports.** `protocol.ts` implements `RpcServer.Protocol` over (a) the existing `SocketServer` for the local socket and (b) a `ws` `WebSocketServer` in `noServer` mode behind a `node:http` server on `127.0.0.1:0` (so the upgrade request is ours). Each connection gets a `clientId` and a `Connection` in `Connections`. Incoming frames are decoded by the protocol; a frame that does not decode, an unknown tag or a payload that fails its schema is answered `INVALID_REQUEST` (exit 3), never a defect. Frames over **16 MiB** (`MAX_FRAME`) close the connection after an `Exit` with `FRAME_TOO_LARGE`. Any request header whose name starts with `x-fin-` is dropped.
12. **The order of checks** for every request (`authorize.ts`): decode → `UNAUTHENTICATED` (the connection has no principal and the RPC is not `public`) → `SCOPE_DENIED` (the RPC's scope is not in the principal's set) → `HUMAN_ONLY` (an escalation or the RPC's scope is human- or owner-only and the principal is not) → `PROJECT_NOT_FOUND` for a project outside `principal.projects` → the handler. A denial is audited as `auth.denied` with the RPC tag, scope and principal kind.
13. **Exit class 7** (`DENIED`): `UNAUTHENTICATED`, `SERVER_UNTRUSTED`, `SCOPE_DENIED`, `HUMAN_ONLY`, `CREDENTIAL_REVOKED`. Exit 3: `INVALID_REQUEST`, `INVALID_PATH`. Exit 2: `FRAME_TOO_LARGE`. Exit 5: `CONTROL_DB_BUSY`.
14. **The scope table** lives in `packages/contracts/src/scopes.ts` as `RPC_SCOPES: { [Tag in FinRpcTag]: Scope }`; the mapped type makes a missing RPC a type error, and `scopes.test.ts` checks the runtime keys equal `FinRpcs.requests.keys()` too. Escalations are `ESCALATIONS: Partial<{ [Tag in FinRpcTag]: (payload) => Scope | null }>`; 5A defines them for `project.takeover {confirm: true}`, `project.open {relocate: true}`, `diagnostics.project {collect: "default"}` and `diagnostics.export` with content.
15. **Default scope sets** (spec §4.4): `console` all; `local` all but `project.confirm`, `diagnostics.admin`, `settings.admin`, `auth.admin`, `library.admin` and `plugin.install` (it keeps `branch.protected` and `review.decide`: Owner question 5's recommendation); `agent` exactly the scopes in its session row, minus every human-only scope (a mint that asks for one is refused `SCOPE_DENIED`); `anonymous` only `public`.
16. **Non-owner `diagnostics.export` is redacted, not refused:** `Journal::export` is called with the diagnostics-only redaction (the upload view's content removal) and the result carries `redacted: true`. The escalation of Decision 14 applies only when the caller asks for content explicitly (`content: true`), which `fin` never does.

### Tickets and the WebSocket

17. **`auth.ticket {ttlS?}`** (default 30, max 60) answers `{ticket, url, expiresAt}`; the ticket is 32 random bytes base64url, kept in memory as `SHA-256(ticket) → {principal snapshot, expiresAt}`, swept every 10 s, used once. `url` is `ws://127.0.0.1:<port>/rpc?v=2&ticket=<t>`.
18. **The upgrade** checks, in order: path `/rpc`; `v` equals `PROTOCOL_VERSION` (else HTTP 426 with `{"protocol":{"min":2,"max":2}}`); `Host` is `127.0.0.1:<port>`, `localhost:<port>` or `[::1]:<port>` (else 403); `Origin` absent or one of those origins or `server.origins` in `settings.json` (5A reads the key; 5B writes it; else 403); the ticket is live (else 401). The connection then carries the ticket's principal.
19. **`finisherd.json`** becomes `{pid, wsPort, socket, protocolVersion: 2, envId, diagnostics: {enabled}}`. The `token` key is gone. `fin` still reads `socket` and `protocolVersion` only.

### Protocol 2

20. **`PROTOCOL_VERSION = 2`** in contracts and `fin` (`endpoint.rs`). `tx.commit.generation` and `ops.subscribe.generation` become required in their schemas; `fin` already sends both; the e2e WebSocket subscriber is updated in Task 5.

### Diagnostics

21. **Sources and actors** (spec §3.4): D4's `RpcSource` service is replaced by `principal.source`; D6's `callMeta(headers)` becomes `actorFor(principal, headers)`. `fin_core::events::Source` gains `Console` (`"console"`), allowed to emit no kinds through `diagnostics.record` (console commands record nothing). The D4 rate limit becomes per principal key (`local`, `console`, `agent:<id>`).
22. **Consent** (`diagnostics.consent`) requires an owner principal for **every** purpose and value (spec §4.5). The D4 rule "training on only from `ui`" is subsumed: `console` is an owner, and there is no `ui` principal until 5B. `fin diagnostics consent training on` stays a local usage error (D14b); the other `fin diagnostics consent …` calls reach the server and answer `HUMAN_ONLY` with the fix `ask the user to run: finisherd diagnostics consent <purpose> <value>`.

### Agent credentials

23. **Agent sessions** (spec §4.1): `agent_sessions(id TEXT PRIMARY KEY, label TEXT, scopes TEXT, projects TEXT, createdBy TEXT, createdAt INTEGER, expiresAt INTEGER, revokedAt INTEGER)`. `auth.agent.mint` (owner) answers `{session, credential}` where `credential` is the file's JSON; the key is derived (Decision 6), so it can be shown once and recomputed for verification without being stored. An expired or revoked session is refused at the hello (no proof) and its live connections are closed at revocation.

### Console

24. **`finisherd <command>`:** `main.ts` dispatches on the first argument: none or `serve` (or a flag such as `--server-data`) serves; `status`, `stop`, `diagnostics`, `project`, `audit`, `agent` run a console command. The console reads `console.key` from the server data named by `--server-data` or `FIN_SERVER_DATA` or the default, connects through `finisherd.json`'s `socket`, runs the handshake as `console`, and prints text (`--json` prints the result document). Exit codes are `fin`'s.

### Hardening

25. **`INVALID_PATH`:** `project.create.dir`, `project.open.dir`, `asset.probe.path` and `project.recover.importDir` must be absolute and normalised (no `.`/`..` segments, no trailing separator except a root); otherwise `INVALID_PATH {field, path}`. The server never resolves a relative path against its own working directory.
26. **`fin` deadline and frame cap:** every `fin` invocation has an overall deadline (`FIN_DEADLINE_S`, default 300 s; streams excluded) on top of the per-operation 30 s; exceeding it is `SERVER_TIMEOUT` with `details.deadline`. A server line over 16 MiB is `PROTOCOL_ERROR` with `details.max`.
27. **`fin`'s test hooks** (`FIN_OFFLINE_SNAPSHOT`, `FIN_TEST_COMMIT_GATE`) are read only when `fin` is built with its new `test-hooks` feature; `fin`'s dev-dependencies enable it for its own tests (as `fin-store` does), and `scripts/check-test-hooks.mjs` already fails a default-members build that enables any `test-hooks`. `FIN_NODE_BINARY_DIR` (the JS loader) stays until sub-project 9's packaging, recorded in TASKS.

## Questions for the owner

This plan implements the recommended answers to spec §12 **Questions 2 and 5** (human-only now, with console commands; `local` keeps `branch.protected` and `review.decide`). If the owner answers otherwise:
- **Q2 "wait for sub-project 7":** drop Decision 22's owner rule for diagnostics and the escalations of Decision 14 (keep the scopes, mark them non-human); Tasks 6 and 7 shrink, the console keeps `status`, `stop`, `audit`, `agent`.
- **Q5 "protect `main` from `local` now":** remove `branch.protected` and `review.decide` from `local` in Decision 15; that only takes effect when 5E adds protected branches, so nothing in 5A changes beyond the default table and its tests.

Questions 1, 3 and 4 do not affect 5A (Q4 only decides whether 5B fills `audit.remote`).

## Review Focus

1. **The handshake is mutual and replay-proof:** fresh nonces on both sides, domain-separated labels, constant-time comparison, no request before the proof is verified, keys rotated at every start before listening.
   → Task 2 `handshake_vectors_match_rust`, `keys_rotate_before_listen`; Task 5 `fin_refuses_server_without_proof`, `replayed_prove_is_refused`; Task 8 `squatter_never_receives_a_request`.
2. **Nothing trusts a transport:** a socket connection without a proof can only say hello; a WebSocket needs a ticket; a `local` ticket yields `local`, never `ui`; `x-fin-` headers cannot set the principal.
   → Task 3 `unproved_connection_only_hello`, `x_fin_headers_are_stripped`; Task 5 `ws_without_ticket_is_401`, `ticket_is_one_time`, `local_ticket_is_local_principal`.
3. **Every RPC has a scope, and human-only holds for every non-human principal:** the mapped type and the runtime key check; the human-only matrix over every principal kind and escalation.
   → Task 4 `rpc_scopes_cover_every_rpc`, `human_only_matrix`, `escalations_apply_before_handlers`, `denials_are_audited`.
4. **Sources and actors from principals:** `agent` principals cannot claim another session; `console` is an owner; consent from `fin` is `HUMAN_ONLY`; redacted export for non-owners.
   → Task 6 `agent_actor_is_its_session`, `consent_needs_owner`, `non_owner_export_is_redacted`, `rate_limit_is_per_principal`.
5. **Windows:** private server data (owner-only DACL), the pipe squatter, a graceful stop.
   → Task 2 `private_dir_refuses_everyone_ace` (Windows), Task 8, Task 13.
6. **No secrets leak:** keys, MACs, tickets and nonces never in logs, audit rows, journal entries, `finisherd.json` or error details.
   → Task 11 leak grep.
7. **The control store never stalls the event loop:** the 1 ms per-statement budget with 100k audit rows.
   → Task 1 `control_store_budget`.

---

### Task 1: The control store and the audit log — tier: sonnet

**Files:**
- Create: `apps/finisherd/src/control/{db.ts, migrations.ts, migrations/m0001.ts, audit.ts}`.
- Test: `apps/finisherd/test/control.test.ts`, `apps/finisherd/test/audit.test.ts`, `apps/finisherd/scripts/control-budget.ts` (the budget measurement, run by the test with a reduced row count and by hand with the full one).
- Modify: `apps/finisherd/src/main.ts` (open the store in `serve`, before the transports; a layer), `TASKS.md`.

**Interfaces:**
```ts
// control/db.ts
export const CONTROL_FORMAT_VERSION = 1;
export const CONTROL_MIN_READER_VERSION = 1;
export interface ControlStore {
  readonly db: DatabaseSync;
  /** Runs `f` in BEGIN IMMEDIATE … COMMIT; rolls back on throw. */
  tx<A>(f: () => A): A;
  close(): void;
}
export function openControlStore(serverData: string, opts?: { serverId?: string }): ControlStore; // ServerStartError CONTROL_DB_TOO_NEW
export class ControlStoreTag extends Context.Tag("ControlStore")<ControlStoreTag, ControlStore>() {}
export const layerControlStore: (serverData: string) => Layer.Layer<ControlStoreTag, ServerStartError>;

// control/audit.ts
export const AuditKind: Schema.Literals<[...]>;   // spec §3.3 kinds, 5A subset plus reserved names
export interface AuditInput { kind: AuditKind; principal: { kind: string; id: string }; outcome: "ok" | "denied" | "failed"; code?: string; connection?: number; details?: Record<string, unknown> }
export class Audit {
  constructor(store: ControlStore, clock?: () => number);
  append(e: AuditInput): void;            // never throws: a failure is logged and counted
  list(q: { since?: number; kind?: AuditKind; cursor?: string; limit?: number }): { entries: AuditEntry[]; next?: string };
  purge(q: { before?: number; all?: boolean }): number;
  sweep(): void;                          // retention: 180 days, 100k rows
  failures(): number;
}
```
Decisions 1–4.

- [ ] **Step 1: Write failing tests:**
  - `opens_wal_0600_with_meta` (unix mode; `journal_mode` is `wal`; `meta` has both versions and the server id);
  - `migrations_run_once_in_order_with_backup` (a fake `m0002` in a test-only list; `.pre-m2.bak` exists; the newest two are kept);
  - `newer_min_reader_refuses_start` (`CONTROL_DB_TOO_NEW`);
  - `corrupt_db_is_moved_aside_and_audited` (bytes overwritten in the middle of the file);
  - `busy_maps_to_control_db_busy` (a second connection holding `BEGIN IMMEDIATE`);
  - `audit_append_list_cursor_purge`, `auth_fail_aggregates_per_minute`, `retention_by_age_and_rows`, `audit_failure_never_throws` (read-only file);
  - `audit_rows_hold_no_secret_fields` (the schema has no column for keys, MACs or tickets; `details` of every 5A writer is checked against an allow-list in Tasks 4–7);
  - `control_store_budget` (10k jobs-sized rows and 100k audit rows; p99 per prepared statement ≤ 1 ms on Linux; the test asserts ≤ 5 ms to stay stable on CI and prints the measured p99).
- [ ] **Step 2: Implement.** Confirm `node:sqlite` loads without an experimental warning on Node 26.12 and that FTS5 is compiled in (`CREATE VIRTUAL TABLE … USING fts5`, needed by 5D); record both in the commit body.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): control store with frozen migrations and a local audit log`.

### Task 2: Key files, private directories and the handshake crypto — tier: opus

**Files:**
- Create: `apps/finisherd/src/auth/{keys.ts, crypto.ts}`, `crates/fin-node/src/private_dir.rs`, `tests/fixtures/auth/handshake.json` (fixed fake keys, nonces and the expected proof, MAC and agent key).
- Modify: `crates/fin-node/src/lib.rs` (export `checkPrivateDir`, `makePrivateDir`), `crates/fin-node/Cargo.toml` (`windows-sys` features `Win32_Security`, `Win32_Security_Authorization`, `Win32_System_Threading`), `packages/fin-node` (typings), `apps/finisherd/src/config.ts` (`ensureServerData` uses them on Windows), `apps/finisherd/src/main.ts` (rotate keys after the server lock, before `claimSocketPath`).
- Test: `apps/finisherd/test/keys.test.ts`, `apps/finisherd/test/handshake-vectors.test.ts`, `crates/fin-node/tests/private_dir.rs` (Windows-only cases behind `#[cfg(windows)]`).

**Interfaces:**
```ts
// auth/keys.ts
export interface Keys { readonly local: Buffer; readonly console: Buffer; readonly agentMaster: Buffer }
export function authDir(serverData: string): string;               // <server-data>/auth
export function rotateKeys(serverData: string): Keys;               // local+console new, agentMaster kept; 0600; private dir
export function readKeyFile(path: string): Buffer;                  // 32 bytes or throws

// auth/crypto.ts
export const LABEL = { hello: "finisherd/hello/v1", prove: "fin/prove/v1", agentKey: "fin/agent-key/v1" } as const;
export function nonce(): Buffer;                                    // 32 random bytes
export function helloProof(key: Buffer, clientNonce: Buffer, serverNonce: Buffer): Buffer;
export function proveMac(key: Buffer, serverNonce: Buffer, clientNonce: Buffer): Buffer;
export function agentKey(master: Buffer, sessionId: string): Buffer;
export function sameMac(a: Buffer, b: Buffer): boolean;             // constant time, false on length mismatch
export const b64u: { encode(b: Buffer): string; decode(s: string): Buffer | null };
```
```rust
// fin-node: private_dir.rs (napi)
#[napi] pub fn check_private_dir(path: String) -> PrivateDirReport; // {ok, owner: "self"|"other", broadAces: [..]} on Windows; {ok: true} elsewhere
#[napi] pub fn make_private_dir(path: String) -> napi::Result<()>;  // Windows: protected DACL (user, SYSTEM, Administrators); elsewhere mkdir 0700
```
Decisions 5–6; spec §4.11.

- [ ] **Step 1: Write failing tests:**
  - `handshake_vectors_match` (TS over `handshake.json`; Task 8 adds the Rust twin `handshake_vectors_match_rust`);
  - `keys_rotate_on_every_start`, `agent_master_survives_restart`, `key_files_are_0600_dir_0700` (unix), `malformed_key_file_is_refused`;
  - `keys_rotate_before_listen` (a test `serve` with a hook that records the order: lock → keys → socket);
  - `same_mac_is_constant_time_shaped` (equal length check first; no early exit on content: asserted by code review and a unit test of mismatching lengths);
  - Windows: `private_dir_refuses_everyone_ace`, `private_dir_refuses_other_owner`, `made_private_dir_passes_check`, `server_data_unsafe_on_windows` (through `ensureServerData`).
- [ ] **Step 2: Implement.** On Windows `make_private_dir` sets `D:P(A;OICI;FA;;;<current user SID>)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)` with `SetNamedSecurityInfoW` and `PROTECTED_DACL_SECURITY_INFORMATION`; `check_private_dir` reads the owner and DACL with `GetNamedSecurityInfoW` and reports any allow ACE for `Everyone` (S-1-1-0), `Users` (S-1-5-32-545), `Authenticated Users` (S-1-5-11) or another user's SID. Key files inherit the directory's DACL.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): rotating local and console keys, private server data on Windows`.

### Task 3: The connection-aware RPC protocol and principals — tier: opus

**Files:**
- Create: `apps/finisherd/src/{protocol.ts, connections.ts, principal.ts}`, `packages/contracts/src/auth.ts` (`PrincipalSchema`, `CredentialRefSchema`, `ServerHelloPayload`/`Success` v2, `AuthProvePayload`/`Success`).
- Modify: `apps/finisherd/src/transport.ts` (`serveRpc` takes the protocol instead of `layerProtocolSocketServer`), `apps/finisherd/src/main.ts`, `packages/contracts/src/{rpc.ts, index.ts}` (re-export; `server.hello` uses the new schemas; `auth.prove` added).
- Test: `apps/finisherd/test/protocol.test.ts`, `apps/finisherd/test/support/ndjson-client.ts` (gains `hello` and `prove` helpers).

**Interfaces:**
```ts
// contracts/auth.ts
export const PrincipalKind = Schema.Literals(["anonymous", "local", "console", "device", "agent", "plugin"]);
export const PrincipalSchema = Schema.Struct({
  kind: PrincipalKind, id: Schema.String, role: Schema.optional(Schema.Literals(["owner", "editor", "viewer"])),
  scopes: Schema.Array(ScopeSchema), projects: Schema.Union([Schema.Literal("*"), Schema.Array(Schema.String)]),
  human: Schema.Boolean, owner: Schema.Boolean,
});
export type Principal = typeof PrincipalSchema.Type;

// connections.ts
export interface Connection {
  readonly clientId: number; readonly transport: "socket" | "ws"; readonly openedAt: number;
  principal: Principal;                      // anonymous until proved, or the ticket's
  hello?: { clientNonce: Buffer; serverNonce: Buffer; credential: CredentialRef };
  failures: number;
}
export class Connections {
  open(transport: Connection["transport"], principal?: Principal): Connection;
  get(clientId: number): Connection | undefined;
  close(clientId: number): void;
  closeWhere(pred: (c: Connection) => boolean): number;   // revocation; ends streams
  count(): Record<string, number>;                         // by principal kind
}

// protocol.ts
export const MAX_FRAME = 16 * 1024 * 1024;
export const makeSocketProtocol: (server: SocketServer["Service"], conns: Connections) => Effect.Effect<RpcServer.Protocol["Service"]>;
export const makeWsProtocol: (opts: { conns: Connections; upgrade: UpgradeCheck }) => Effect.Effect<{ protocol: RpcServer.Protocol["Service"]; port: number }>;

// principal.ts
export class CurrentConnections extends Context.Tag("Connections")<CurrentConnections, Connections>() {}
export function principalOf(conns: Connections, clientId: number): Principal;      // anonymous if unknown
export function principalFor(kind: "local" | "console", keys: Keys): Principal;    // the default scopes of Task 4
```
Decisions 7–8, 11.

- [ ] **Step 1: Spike, then write it down.** Read Effect 4.0.6's `RpcServer` sources (`node_modules/effect/dist/…/rpc/RpcServer.js`: `Protocol`, `makeProtocolSocketServer`, the handler options). Confirm (a) that handlers receive `{ clientId, headers }` and (b) how a custom `Protocol` reports connects and disconnects. Record the answer at the top of `protocol.ts`. **If handlers cannot see `clientId`,** the protocol injects a server-only header `x-fin-conn: <clientId>` into each decoded request after stripping client `x-fin-*` headers, and `principalOf` reads it; the rest of the plan is unchanged.
- [ ] **Step 2: Write failing tests:**
  - `each_connection_gets_its_own_principal` (two sockets, one proved as `local`, one anonymous);
  - `unproved_connection_only_hello` (any other RPC → `UNAUTHENTICATED`; the RPC handler never runs, checked with a counting handler);
  - `prove_binds_local_and_console` (`auth.prove` → `{principal: {kind: "local"}}`; with the console key, `console`);
  - `wrong_mac_fails_and_five_failures_close`;
  - `second_prove_is_invalid_request`;
  - `hello_without_nonce_is_anonymous_and_proofless`;
  - `x_fin_headers_are_stripped` (a client sending `x-fin-conn` cannot become another connection);
  - `bad_frame_is_invalid_request_not_defect` (invalid JSON, unknown tag, a payload missing a field);
  - `oversized_frame_is_frame_too_large_then_closed`;
  - `disconnect_removes_connection`.
- [ ] **Step 3: Implement.** Keep framing identical on the wire (NDJSON `Request`/`Exit`/`Chunk`/`Ack`/`Ping`/`Pong`), so the captured fixtures in `crates/fin/tests/fixtures/rpc/` still describe it; only `server.hello`'s payload and result grow.
- [ ] **Step 4: Run** the gates (the existing `server.test.ts` and `fin`'s `rpc_contract.rs` must pass unchanged except for the hello) → PASS. **Commit** `feat(finisherd): connection-aware RPC protocol with proved principals`.

### Task 4: Scopes, escalations and the authorisation wrapper — tier: opus

**Files:**
- Create: `packages/contracts/src/scopes.ts`, `apps/finisherd/src/authorize.ts`.
- Modify: `apps/finisherd/src/handlers.ts` (`FinRpcs.toLayer(Effect.map(handlers, authorizeAll))`: one call-site change), `packages/contracts/src/{errors.ts (code list), index.ts}`.
- Test: `packages/contracts/src/scopes.test.ts`, `apps/finisherd/test/authorize.test.ts`.

**Interfaces:**
```ts
// contracts/scopes.ts
export const SCOPES = ["public", "server.read", "project.read", "project.write", "project.admin", "project.confirm",
  "branch.protected", "review.write", "review.decide", "jobs.read", "jobs.submit", "jobs.control",
  "library.read", "library.scan", "library.admin", "diagnostics.read", "diagnostics.admin",
  "settings.admin", "auth.admin", "plugin.install"] as const;
export type Scope = (typeof SCOPES)[number];
export const ScopeSchema: Schema.Literals<typeof SCOPES>;
export const HUMAN_ONLY: Readonly<Partial<Record<Scope, "human" | "owner">>>;
// project.confirm: human; library.admin, diagnostics.admin, settings.admin, auth.admin, plugin.install: owner
export type FinRpcTag = RpcGroup.Rpcs<typeof FinRpcs>["_tag"];
export const RPC_SCOPES: { readonly [T in FinRpcTag]: Scope };
export const ESCALATIONS: { readonly [T in FinRpcTag]?: (payload: unknown) => Scope | null };
export function defaultScopes(kind: PrincipalKind, role?: "owner" | "editor" | "viewer"): ReadonlyArray<Scope>;
export function humanOnlyFor(scope: Scope): "human" | "owner" | undefined;

// finisherd/authorize.ts
export type Denial = FinError; // UNAUTHENTICATED | SCOPE_DENIED | HUMAN_ONLY | PROJECT_NOT_FOUND
export function check(principal: Principal, tag: FinRpcTag, payload: unknown): Denial | undefined;
export function authorizeAll<H extends Record<FinRpcTag, (p: any, o: any) => any>>(handlers: H, deps: { conns: Connections; audit: Audit }): H;
```
Decisions 12, 14–16; spec §4.4 tables (the RPCs that exist after D1–D7, the 4C RPCs and this plan's).

- [ ] **Step 1: Write failing tests:**
  - `rpc_scopes_cover_every_rpc` (runtime keys = `FinRpcs.requests.keys()`; the mapped type is the compile-time half);
  - `streams_are_scoped_too` (`ops.subscribe`, `project.events` refuse an anonymous connection before the stream starts);
  - `human_only_matrix` (every scope with `HUMAN_ONLY`, every escalation, × principals `anonymous`, `local`, `agent` (any scopes), `console` → only `console` passes; an `agent` session minted with a human-only scope is impossible, Task 9);
  - `escalations_apply_before_handlers` (`project.takeover {confirm: true}` from `local` → `HUMAN_ONLY`, `{confirm: false}` → the handler; `project.open {relocate: true}`; `diagnostics.project {collect: "default"}` vs `"never"`; `diagnostics.export {content: true}`);
  - `check_order_is_fixed` (a request that is both unauthorised and malformed answers in Decision 12's order);
  - `project_reach_hides_other_projects` (an agent principal with `projects: [A]` gets `PROJECT_NOT_FOUND` for B on a unary RPC and on `ops.subscribe`);
  - `denials_are_audited` (`auth.denied` row with tag, scope and principal kind; no payload);
  - `fix_names_the_console_command` (each human-only RPC's `HUMAN_ONLY.fix` contains a `finisherd …` command; the table lives beside the scopes).
- [ ] **Step 2: Implement.** `authorizeAll` wraps each handler (unary and stream) so the existing `guard` stays inside it. Denials never reach `rpc.stats` as `INTERNAL`: they carry their own codes.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(contracts,finisherd): a scope for every RPC, escalations and human-only checks`.

### Task 5: Handshake RPCs, tickets, the WebSocket without a token, protocol 2 — tier: sonnet

**Files:**
- Create: `apps/finisherd/src/auth/{handshake.ts (server.hello, auth.prove handlers), tickets.ts}`, `apps/finisherd/src/upgrade.ts` (the HTTP upgrade checks).
- Modify: `apps/finisherd/src/{transport.ts (remove `hasBearer`, `makeWebSocketServer`), config.ts (`ServerInfo` without `token`, with `envId`), main.ts, handlers.ts}`, `packages/contracts/src/{index.ts (PROTOCOL_VERSION = 2), rpc.ts (`generation` required; `auth.ticket`, `auth.whoami`)}`, `crates/fin/src/endpoint.rs` (`PROTOCOL_VERSION = 2`), `scripts/e2e/lib.mjs` (`connectWs` mints a ticket over the socket as `local`), `apps/finisherd/scripts/capture-rpc-fixtures.ts` and the fixtures it writes.
- Test: `apps/finisherd/test/tickets.test.ts`, `apps/finisherd/test/server.test.ts` (updated), `scripts/e2e.test.mjs`.

**Interfaces:**
```ts
// auth/tickets.ts
export class Tickets {
  constructor(clock?: () => number);
  mint(principal: Principal, ttlS?: number): { ticket: string; expiresAt: number };   // ≤ 60 s
  take(ticket: string): Principal | undefined;                                       // one-time
  sweep(): void;
}
// upgrade.ts
export interface UpgradeCheck { (req: IncomingMessage): { ok: true; principal: Principal } | { ok: false; status: 401 | 403 | 404 | 426; body?: unknown } }
export function makeUpgradeCheck(opts: { port: () => number; tickets: Tickets; origins: () => ReadonlyArray<string> }): UpgradeCheck;
```
Decisions 7–8, 17–20.

- [ ] **Step 1: Write failing tests:**
  - `hello_returns_proof_for_named_credential` (local, console; an unknown agent id gets no proof);
  - `replayed_prove_is_refused` (a MAC captured on one connection fails on another: new nonces);
  - `ticket_is_one_time`, `ticket_expires`, `ticket_ttl_capped_at_60`;
  - `ws_without_ticket_is_401`, `ws_wrong_version_is_426_with_range`, `ws_bad_host_is_403`, `ws_browser_origin_matrix` (absent ok; own origins ok; another origin 403; a `server.origins` entry ok);
  - `local_ticket_is_local_principal` (`auth.whoami` over the WebSocket answers `local`);
  - `finisherd_json_has_no_token` (and holds `envId`, `protocolVersion: 2`);
  - `generation_is_required` (`tx.commit` without it → `INVALID_REQUEST`).
- [ ] **Step 2: Implement.** Re-capture the RPC fixtures (`pnpm --filter finisherd capture-rpc-fixtures`) and update `crates/fin/tests/rpc_contract.rs` for the new hello.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): mutual handshake, one-time WebSocket tickets, protocol 2`.

### Task 6: Diagnostics: sources and actors from principals, owner-only operations — tier: opus

**Files:**
- Modify: `crates/fin-core/src/events/mod.rs` (`Source::Console`), `crates/fin-core/src/events/names.rs` (`RPC_METHODS` += `auth.prove`, `auth.ticket`, `auth.whoami`, `auth.agent.mint`, `auth.agent.list`, `auth.agent.revoke`, `auth.audit.list`, `auth.audit.purge`, `server.status`, `server.stop`), `crates/fin-core/src/events/kinds.rs` (`console` emits nothing through `diagnostics.record`), the regenerated schema and `generated.ts`, `apps/finisherd/src/diagnostics.ts` (D4's file: `RpcSource` → `principal.source`; `callMeta` → `actorFor`; rate limit keyed by principal; consent rule; redacted export), `apps/finisherd/src/main.ts` and `transport.ts` (drop the per-transport `RpcSource` layers D4 added).
- Test: `crates/fin-core/tests/events.rs` (golden for `console`), `apps/finisherd/test/diagnostics-principals.test.ts`.

**Interfaces:**
```ts
export function sourceOf(p: Principal): Source | undefined;     // local|agent → fin, console → console, device → ui, anonymous → undefined
export function actorFor(p: Principal, headers: Headers): Actor;  // spec §3.4
```
Decisions 16, 21–22.

- [ ] **Step 1: Write failing tests:**
  - `source_follows_principal` (a `local` socket connection and a `local`-ticket WebSocket both record `source: fin`; `console` records `console`);
  - `agent_actor_is_its_session` (an agent principal sending `fin-actor-kind: user` and another session id is recorded as `{kind: agent, session: <its id>}`, with the header's `call` kept);
  - `local_actor_keeps_headers` (D6 behaviour unchanged for `local`);
  - `consent_needs_owner` (`diagnostics.consent` from `local` for both purposes and every value → `HUMAN_ONLY`; from `console` → applied; training `on` from `console` writes `basis: user`);
  - `purge_and_project_default_need_owner`, `project_never_open_to_writers`;
  - `non_owner_export_is_redacted` (training on, a `log.commit` with `intent`; `local` export has no `intent` and `redacted: true`; `console` export has it);
  - `rate_limit_is_per_principal` (two agent sessions each get their own bucket; a thousand `local` runs share one);
  - `rpc_methods_table_matches_contracts` (the D4 drift test, now with the new RPCs).
- [ ] **Step 2: Implement.** Update the D-plan's `fin diagnostics consent|purge|project` tests in `crates/fin/tests/cli_diagnostics.rs`: they now expect exit 7 `HUMAN_ONLY` (except `consent training on`, still exit 2), and the cases that changed state move to console tests in Task 7.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` (the D7 diagnostics scenario uses the console for consent and purge now; update its steps 1, 4 and 5) → PASS. **Commit** `feat(diagnostics): sources and actors from principals; consent, purge and content export need an owner`.

### Task 7: Console commands, `server.stop`, human-only storage operations — tier: sonnet

**Files:**
- Create: `apps/finisherd/src/console/{main.ts (dispatch, flags, exit codes), client.ts (an NDJSON RPC client over the local socket with the console handshake; reuse `test/support/ndjson-client.ts`'s framing by moving the shared part to `src/console/ndjson.ts`), format.ts (text and `--json`), commands/{status.ts, stop.ts, diagnostics.ts, project.ts, audit.ts, agent.ts}}`, `apps/finisherd/src/server-admin.ts` (`server.status`, `server.stop`, `auth.audit.*` handlers).
- Modify: `apps/finisherd/src/main.ts` (Decision 24 dispatch; `server.stop` interrupts the main fiber), `apps/finisherd/src/handlers.ts` (new handlers), `packages/contracts/src/{auth.ts, rpc.ts}` (`server.status`, `server.stop`, `auth.audit.list`, `auth.audit.purge`), `apps/finisherd/package.json` (`bin` unchanged: `finisherd`).
- Test: `apps/finisherd/test/console.test.ts`.

**Interfaces:**
```text
finisherd [serve] [--server-data <dir>]
finisherd status [--json]
finisherd stop
finisherd diagnostics consent <diagnostics|training> <on|off|unset>
finisherd diagnostics purge (--all | --project <id> | --before <date>)
finisherd diagnostics project default <projectId>
finisherd diagnostics export [--content] [--since] [--project] [--kind]… [--limit]   (JSONL on stdout)
finisherd project takeover <dir|projectId> --confirm
finisherd project open <dir> --relocate
finisherd audit [--since <date>] [--kind <k>] [--limit <n>] [--json]
finisherd agent mint --label <s> --scope <scope>… [--project <id>]… [--expires <date>] [--out <file>]
finisherd agent ls | revoke <id>
```
Decision 24; spec §4.5–4.7.

- [ ] **Step 1: Write failing tests:**
  - `serve_is_default_and_flags_still_serve` (`finisherd --server-data X` serves as before);
  - `console_proves_as_console` (`finisherd status --json` shows `connections.console: 1` while it runs);
  - `console_without_server_is_server_unavailable` (exit 5, the fix says to start `finisherd`);
  - `stop_shuts_down_gracefully` (exit 0, `finisherd.json` removed, engines closed: the same finalizers as SIGINT; runs on Windows too);
  - `takeover_confirm_and_relocate_via_console` (and `fin project takeover --confirm` / `fin project open --relocate` → exit 7 `HUMAN_ONLY` naming these commands);
  - `diagnostics_commands_via_console` (consent, purge, project default, export with content);
  - `audit_lists_denials_and_purges`;
  - `console_never_prints_secrets` (keys and MACs absent from stdout and stderr in every test above).
- [ ] **Step 2: Implement.** Move the existing tests that used `fin project takeover --confirm` or `fin project open --relocate` (`crates/fin/tests/cli_online.rs`, `cli_history.rs`, `scripts/e2e/storage.mjs`, `scripts/e2e/history*.mjs`; grep for `--confirm` and `--relocate`) to the console, or to a test-support helper that proves as `console` with the key from the test's server data.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): console commands for human-only operations; graceful stop`.

### Task 8: `fin`: credentials, the handshake, exit 7, `whoami` — tier: sonnet

**Files:**
- Create: `crates/fin/src/auth.rs` (credential discovery, `Credential {kind, id?, key}`, `handshake(&mut Client)`, MAC helpers), `crates/fin/src/commands/whoami.rs`, `crates/fin/tests/cli_auth.rs`.
- Modify: `crates/fin/src/client.rs` (call `auth::handshake` after connect: call-site lines only), `crates/fin/src/output.rs` (`DENIED = 7`, `DENIED_CODES`; exit-code tests), `crates/fin/src/{registry.rs, map.rs, main.rs}` (`whoami`), `crates/fin/Cargo.toml` (`hmac`, `sha2`, `base64` if not present).
- Test: `crates/fin/tests/cli_auth.rs`, `crates/fin/tests/support/mod.rs` (a fake server that holds no key; a helper to read the test server's keys).

**Interfaces:**
```rust
pub enum CredentialKind { Local, Console, Agent }
pub struct Credential { pub kind: CredentialKind, pub id: Option<String>, key: [u8; 32] }
pub fn discover(server_data: Option<&Path>) -> Option<Credential>;   // FIN_CREDENTIAL, else <server-data>/auth/local.key
pub fn handshake(client: &mut Client, cred: Option<&Credential>) -> Result<Principal, CoreError>; // SERVER_UNTRUSTED, UNAUTHENTICATED
pub fn hello_proof(key: &[u8; 32], client_nonce: &[u8; 32], server_nonce: &[u8; 32]) -> [u8; 32];
pub fn prove_mac(key: &[u8; 32], server_nonce: &[u8; 32], client_nonce: &[u8; 32]) -> [u8; 32];
```
`fin whoami` → `{principal}` (an anonymous connection answers `{principal: {kind: "anonymous"}}`, exit 0). Decisions 9–10, 13; spec §3.5.

- [ ] **Step 1: Write failing tests:**
  - `handshake_vectors_match_rust` (the shared vector file);
  - `fin_works_as_local` (the existing online suites pass unchanged: the harness's server data holds the key);
  - `fin_refuses_server_without_proof` (a fake NDJSON server that answers `server.hello` without `proof`, or with a wrong one: exit 7 `SERVER_UNTRUSTED`, and the fake records no request after the hello);
  - `squatter_never_receives_a_request` (the fake server listens on the socket named by a stale `finisherd.json`);
  - `no_credential_is_unauthenticated` (`FIN_SERVER` only: exit 7, fix names `FIN_CREDENTIAL` and server data);
  - `agent_credential_file_is_used` (`FIN_CREDENTIAL` from `finisherd agent mint --out`; `fin whoami` answers `agent`);
  - `denied_codes_exit_7` (`exit_codes_follow_the_contract` gains the five codes);
  - `map_still_under_budget`.
- [ ] **Step 2: Implement.** `handshake` uses the connection's normal per-operation timeout; its round trip is excluded from nothing (it is part of every run) and measured in Task 13.
- [ ] **Step 3: Run** `rtk cargo test -p fin` (`FIN_REQUIRE_SERVER=1`), the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(fin): mutual handshake with key files, exit code 7, fin whoami`.

### Task 9: Agent credentials — tier: opus

**Files:**
- Create: `apps/finisherd/src/auth/agents.ts`, `apps/finisherd/src/control/migrations/m0002.ts` (`agent_sessions`).
- Modify: `apps/finisherd/src/auth/handshake.ts` (agent hello), `apps/finisherd/src/handlers.ts`, `packages/contracts/src/{auth.ts, rpc.ts}` (`auth.agent.mint|list|revoke`), `apps/finisherd/src/console/commands/agent.ts`.
- Test: `apps/finisherd/test/agents.test.ts`.

**Interfaces:**
```ts
export const AgentCredentialSchema = Schema.Struct({ v: Schema.Literal(1), kind: Schema.Literal("agent"), id: Schema.String, key: Schema.String, env: Schema.String });
export class AgentSessions {
  constructor(store: ControlStore, master: Buffer, audit: Audit, clock?: () => number);
  mint(by: Principal, req: { label: string; scopes: Scope[]; projects?: string[]; expiresAt?: number }): { session: AgentSession; credential: AgentCredential };
  keyFor(id: string): Buffer | undefined;        // undefined when unknown, expired or revoked
  principal(id: string): Principal | undefined;
  list(): AgentSession[];
  revoke(by: Principal, id: string, conns: Connections): boolean;   // closes live connections
}
```
Decision 23; spec §4.1.

- [ ] **Step 1: Write failing tests:**
  - `mint_then_prove_as_agent` (scopes exactly as minted, `human: false`);
  - `mint_refuses_human_only_scopes` (`SCOPE_DENIED` naming them);
  - `mint_needs_owner` (`local` → `HUMAN_ONLY`);
  - `key_is_never_stored` (no column holds it; the credential is recomputed from the master);
  - `revoke_closes_live_connections` (an open `ops.subscribe` ends with `CREDENTIAL_REVOKED` within 100 ms);
  - `expired_session_gets_no_proof`;
  - `agent_survives_server_restart` (the master is kept; the same credential file proves after a restart);
  - `agent_scopes_limit_rpcs` (`project.read` only: `tx.commit` → `SCOPE_DENIED`);
  - `mint_and_revoke_are_audited`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): agent credentials derived from a master key, with scopes and revocation`.

### Task 10: RPC hardening from the sub-project 5 follow-ups — tier: sonnet

**Files:**
- Create: `apps/finisherd/src/paths.ts` (`requireAbsolute(field, path)`).
- Modify: `apps/finisherd/src/handlers.ts` (call sites), `crates/fin/src/{client.rs (frame cap; call-site lines), conn.rs (a line-length bound in `read_line`), main.rs (the overall deadline)}`, `crates/fin/src/backend.rs` and `crates/fin/Cargo.toml` (the `test-hooks` feature; dev-dependency on itself with the feature), `TASKS.md`.
- Test: `apps/finisherd/test/paths.test.ts`, `crates/fin/tests/cli_hardening.rs`, `scripts/check-test-hooks.test.mjs` (a case for `fin`).

**Interfaces:** `requireAbsolute(field: string, path: string): Effect.Effect<string, FinError>` (`INVALID_PATH {field, path}`); `FIN_DEADLINE_S`. Decisions 25–27.

- [ ] **Step 1: Write failing tests:**
  - `relative_paths_are_invalid_path` (`project.create {dir: "x"}`, `project.open {dir: "./x"}`, `asset.probe {path: "a/../b"}`, `project.recover {importDir: "rel"}`; Windows: `C:foo`, `\foo` without a drive);
  - `fin_frame_cap` (a fake server sending a 17 MiB line → `PROTOCOL_ERROR {max}`);
  - `fin_overall_deadline` (a fake server that answers every 25 s: the third call hits `FIN_DEADLINE_S=60` → `SERVER_TIMEOUT {deadline}`);
  - `test_hooks_ignored_without_feature` (a `fin` built without the feature ignores `FIN_OFFLINE_SNAPSHOT`; run through `cargo run -p fin --no-default-features` in the test, or a unit test on the cfg'd function);
  - `check_test_hooks_flags_fin` (the gate fails if `fin`'s normal build enables `test-hooks`).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `fix(finisherd,fin): invalid paths, frame caps and deadlines; fin test hooks behind a feature`.

### Task 11: e2e `auth` scenario — tier: sonnet

**Files:**
- Create: `scripts/e2e/auth.mjs`.
- Modify: `scripts/e2e.mjs` (`SCENARIOS.auth`), `scripts/e2e.test.mjs`, `scripts/e2e/lib.mjs` (helpers: `console(args)`, `rawConnect()` for an unproved socket).

- [ ] **Step 1: Write the scenario,** driven by `fin`, the console and raw sockets:
  1. Start `finisherd`. A raw socket connection that sends only `snapshot.get` gets `UNAUTHENTICATED`; `server.hello` answers.
  2. `fin project create`, three edits and an undo work (`local`). `fin whoami` says `local`.
  3. `fin diagnostics purge --all`, `fin diagnostics consent diagnostics off` and `fin project takeover --confirm` exit 7 with `HUMAN_ONLY`, each `fix` naming a `finisherd …` command; `finisherd diagnostics consent diagnostics off` then `on` works; `finisherd audit --json` lists the three denials.
  4. A ticketed WebSocket subscriber (`lib.mjs` `connectWs`) streams the ops of step 2 from rev 0; reusing its ticket is refused (401).
  5. `finisherd agent mint --scope project.read --out <tmp>/cred.json`; with `FIN_CREDENTIAL`, `fin whoami` says `agent`, `fin view` works and an edit exits 7 `SCOPE_DENIED`; `finisherd agent revoke` makes the next `fin` run exit 7.
  6. Restart `finisherd`: a copy of the old `local.key` no longer proves (a raw handshake with it fails); `fin` works (it reads the new key).
  7. **Leak grep** over `finisherd.json`, the audit export, the journal (`events/`), stdout and stderr of every step: none of the key files' contents, no ticket, no MAC.
- [ ] **Step 2: Run** `rtk pnpm e2e` (all scenarios) → PASS. **Commit** `test(e2e): auth scenario for principals, tickets, human-only and agent credentials`.

### Task 12: Spec, architecture pointer, README and tracker — tier: sonnet

**Files:**
- Modify: `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` (§3 and §4 "as built": the Task 3 spike answer, the measured handshake cost and statement budget, any deviation), `docs/specs/2026-11-01-finisher-architecture-design.md` (§27 item 15 "Sub-project 5A as built", pointing at the spec, and §27.12.5's WebSocket token sentence marked superseded), `README.md` (a "Security model" section: principals, key files, the console, human-only operations, exit 7), `TASKS.md`.

- [ ] **Step 1:** Write the as-built notes from the code, not from this plan; list every deviation.
- [ ] **Step 2: TASKS:** tick the closed "Follow-ups for sub-project 5" lines (pipe squatting by proof, server-data ACL, graceful stop, decode failures, `INVALID_PATH`, deadline and frame cap, `fin` test hooks); keep open: `childEnv` provider variables (7), `FIN_NODE_BINARY_DIR` in the JS loader (9), the pipe DACL itself (the proof makes it non-critical; per-project AppContainer SIDs are sub-project 7); add 5B–5E as headings with one line each.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `docs: record sub-project 5A as built`.

### Task 13: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

Runs on macOS, Windows 11 and the Linux VM. Host details stay in `archive/hosts.md`.

- [ ] **Step 1:** `cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e` on each OS.
- [ ] **Step 2: Windows-specific:**
  - as a **non-elevated** user: `finisherd` creates private server data that passes `checkPrivateDir`; a pre-existing directory with an `Everyone` ACE is refused;
  - **pipe squatting:** a second local user (or a process under a different token) creates `\\.\pipe\finisher-<user>` first; `finisherd` reports `SERVER_RUNNING`; a `fin` that reads a stale `finisherd.json` naming that pipe exits 7 `SERVER_UNTRUSTED` and the squatter logs no request beyond the hello;
  - another user opening the real pipe cannot get past `UNAUTHENTICATED`;
  - `finisherd stop` from a second console.
- [ ] **Step 3: Measure** and record in TASKS and the spec: the handshake's added p50/p99 to a `fin view` run on each OS; the control-store statement p99 on each OS (budget 1 ms); `node:sqlite` with FTS5 on each OS.
- [ ] **Step 4:** Fix what fails (each fix its own commit with a test); re-run Step 1.
