# Sub-project 5A: Principals, local credentials and scopes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (the handshake and its crypto, private directories, the connection-aware protocol, the middleware chain, scopes and the write guard, sources and actors, credentials, the native pipe listener, validation), `sonnet` for plumbing (the control store, `fin`, tickets and the WebSocket, the console, hardening, e2e, docs).

**Revision 2** (2026-11-07), after the review of `45039ab`: reordered so that every commit is green, with the `fin` handshake right after the server's; the middleware chain with resolvers, reach, `ask` and an in-transaction write guard; no training opt-in from any command line; a TTY-gated console with server-issued challenges; fix texts with no commands; the socket in `run/`; a native Windows pipe listener; `envId` and endpoint inside the MACs; one retry after a restart; `mcp` credentials; credential expiry. Spec §15 maps every finding.

**Revision 3** (2026-11-07), after the cross-design reconciliation (spec §16): `control.db` migrations run in per-area chains so that 5C-1 (the job engine) can start right after Tasks 1 and 2 and run beside the rest of this plan; Tasks 1 and 2 are the interfaces 5C-1 consumes, so their signatures are frozen once they merge.

**Goal:** every caller of `finisherd` is a principal with a credential, every RPC declares the scopes it needs, one middleware chain checks them against resolved state, and the human-only operations are enforced by the server. After this plan:
- `<server-data>/control.db` (`node:sqlite`) holds the environment id, the server-id map, the always-on local **audit log** and **credentials**;
- a connection proves a credential with a **mutual HMAC handshake** bound to the environment and the endpoint (`server.hello` + `auth.prove`); `fin` retries once after a restart and otherwise refuses a squatted or foreign server (`SERVER_UNTRUSTED`); a connection without a credential can only say hello;
- principals `local` (`fin`), `console` (`finisherd <command>`), `agent` and `mcp` (credential files, minted from the console until sub-project 7) and `anonymous`; `device` and `plugin` are reserved for 5B and 8;
- the socket lives in `<server-data>/run/`, apart from the keys in `<server-data>/auth/`; on Windows a **native pipe listener** in `fin-node` owns the pipe and its security descriptor;
- the WebSocket's bearer token is gone: one-time **tickets** in `Sec-WebSocket-Protocol`, a Host allow-list, Origin checks;
- a **middleware chain** (decode → authenticate → resolve → authorise → policy → handler with an in-transaction guard → after), a closed **scope** vocabulary with every RPC's scopes and resolver, reach, an `ask` outcome and **human-only** operations that answer `HUMAN_ONLY` (exit 7) with fixes that address the user and contain no command;
- **no command line can opt in to training**; mutating console commands need a TTY and a typed, server-issued challenge;
- the diagnostics journal takes its **source and actor from the principal** (new `console` and `mcp` sources);
- **protocol 2**; `fin` exit code **7 = denied**; `fin whoami`;
- the "Follow-ups for sub-project 5" in `TASKS.md`.

**Architecture:**
- **`@finisher/contracts`** gains `auth.ts` (`Principal`, credentials, handshake and admin RPC schemas) and `scopes.ts` (every RPC's scopes, escalations, human-only rules, per-kind defaults).
- **`finisherd`** gains `control/` (store, migrations, audit), `auth/` (keys, crypto, handshake, tickets, credentials, challenges), `connections.ts`, `chain/` (the middleware stages: `resolve.ts`, `authorize.ts`, `policy.ts`, `chain.ts`), `console/` (the console CLI), `server-admin.ts`; it changes `main.ts`, `transport.ts`, `handlers.ts` (one `withChain` call), `config.ts`, `diagnostics.ts` (D4's file) and `env.ts` (`FIN_CREDENTIAL`).
- **`fin-node`** gains `private_dir.rs` (Windows DACLs), `pipe_server.rs` (the Windows pipe listener) and the write guard argument on every write call.
- **`fin-store`** gains `WriteGuard` checked inside `Store::write`'s transaction.
- **`fin-core::events`** gains `Source::Console`, `Source::Mcp` and the new RPC names.
- **`fin`** gains `auth.rs` (credential discovery, the handshake, the retry), exit class 7, `fin whoami`, `projectId` on probes, a frame cap and an overall deadline, and its test hooks behind a `test-hooks` feature.

**Tech Stack:**
- Node 24, the target (owner decision 2026-11-07, spec §1.6: Electron 44/45 embed Node 24.22). The repository still pins Node 26.12 until the `TASKS.md` switch lands; until then use only APIs present in both, so this plan does not wait for the switch (`node:sqlite`, `node:crypto`); Effect 4.0.6 (`effect/rpc`); `ws` 8 (MIT; already installed under `@effect/platform-node`, now a direct, pinned dependency); vitest.
- Rust 2024: `hmac` 0.12 and `sha2` 0.10 (MIT OR Apache-2.0) in `fin`; `base64` 0.22 (MIT OR Apache-2.0) if not already in the lock file; `windows-sys` 0.61 (already in the tree) with `Win32_Security`, `Win32_Security_Authorization`, `Win32_System_Pipes`, `Win32_System_Threading`, `Win32_System_IO`, `Win32_Storage_FileSystem` in `fin-node`.
- **No other new dependency.** No JOSE, TLS, keychain or certificate crates (5B).

**Spec:** `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` revision 4, §1–4, §10–12 (§12: owner decisions of 2026-11-07), §15, §17. Parent: arch §4 rule 6, §8.3, §8.4 (amended: `FIN_CREDENTIAL`), §16, §21.1, §27.12.5. Related: the diagnostics plan (`docs/superpowers/plans/2026-11-07-diagnostics-and-training-data-hooks.md`) Decisions 14a, 14b, 23a, 24, 30, 32.

**Branch:** `sp5a-principals`, stacked on `sp4c-edit-breadth`.
- **Can start now** (they need only what exists: D1, D2 and 4C as it stands): **Tasks 1, 2 and 10.**
- **5C-1 (the job engine, its own plan) can start once Tasks 1 and 2 have merged,** and runs beside Tasks 3–14 and beside sub-project 6's 6A-1. It uses `openControlStore`/`registerChain` (its `jobs` chain), `Audit`, the `<server-data>` layout of Task 2 and `childEnv`, nothing else from this plan. 5C-2 (the `jobs.*` RPCs) waits for Tasks 3–6.
- **Wait for diagnostics D4 and D6:** Tasks 3–9 and 11–14. They change D4's per-transport `RpcSource`, D4's `diagnostics.*` handlers and D6's actor headers, which do not exist yet. When D4 and D6 land, rebase the branch onto them and continue at Task 3.

**Out of scope:** pairing, devices, DPoP, TLS, remote listeners, routes, allowed roots for devices (5B); jobs (5C); libraries (5D); reviews, protected-branch policy and `drop` records (5E; the guard and the `branch.protected` scope exist here); agent sessions created by an agent runtime, approvals, hooks and the OS sandbox (sub-project 7); plugin credentials (8); the frontend (9).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code. `pnpm gates` runs cargo-deny and the JS licence gate over what each task adds.
- **Primitive ops live only in `fin-core`.** This plan adds none. `fin-core` stays pure.
- **TypeScript model types are generated, never hand-written.** Rust ↔ TS types (`Source`, the write guard, `WriteRefused`) are regenerated with `pnpm --filter @finisher/core-types generate` (drift gate `scripts/check-generated-types.mjs`). TS-owned control-plane records are Effect Schemas in `@finisher/contracts` and their TS types are `Schema.Type` of them.
- **Dependency rules:** `fin` links no TLS or HTTP stack; `finisherd` adds only `ws` directly.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. Near the limit: `apps/finisherd/src/projects.ts` 520, `crates/fin/src/client.rs` 525, `crates/fin/src/main.rs` 489, `crates/fin/src/backend.rs` 493, `crates/fin-store/src/tick.rs` 590: call-site lines only; new logic in new files.
- Errors are `{ code, message, details, fix? }`. New codes go into `crates/fin/src/output.rs` (spec §4.13) and the contracts' code list.
- **Fix texts** (spec §3.5): a human-only code's `fix` addresses the user ("Only you, the owner of this Finisher server, can …: use the Finisher app or the server's console.") and never contains a command line; `details.where` names `app | console | settingsFile | env`. A test enforces it (Task 5).
- **No secret in a tracked file, a log line, a journal entry, an audit row or an error.** Keys, MACs, tickets, challenges and nonces are never printed. Test vectors use fixed, obviously fake keys.

**Every commit is green.** No task leaves the server requiring something its clients do not yet send. Where a task changes both sides (the handshake, tickets, protocol 2, human-only refusals), the server change, the client change and every test-support change land in **one commit**.

**Testing and process**
- Tests never use the network. Every handshake test uses a temp server data.
- TS ↔ Rust crypto agreement is pinned by `tests/fixtures/auth/handshake.json` (fixed keys, nonces, `envId` and endpoint → proofs and MACs), read by a vitest test and a Rust test.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`. Tasks that touch TS or `fin` also run `rtk pnpm e2e` once at the end.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh`.
- **The repo is public:** no host names, IPs, user names or key names in tracked files.
- **Tracker:** each task ticks its line in `TASKS.md` under "Sub-project 5A: principals, local credentials and scopes", in the same commit.
- **Batches:**
  - A, now: Tasks 1, 2, 10 (in any order; each green alone);
  - B, after D4 and D6: Tasks 3 → 4 → 5 → 6 (strictly in order);
  - C: Tasks 7, 8, 9, 11 (7 before 8; 9 and 11 may run beside them);
  - D: Tasks 12–14.

## Decisions taken here (the spec fixes the shape; these fix the details)

### Store

1. **`control.db`:** `node:sqlite` `DatabaseSync`, `journal_mode = WAL`, `synchronous = NORMAL`, `foreign_keys = ON`, `busy_timeout = 250` (`CONTROL_DB_BUSY`, exit 5, past it), **`wal_autocheckpoint = 0`** on the main connection. A `worker_thread` with its own connection runs `PRAGMA wal_checkpoint(PASSIVE)` every 30 s or when `control.db-wal` passes 4 MiB, and `TRUNCATE` after 60 s without writes. The file is created 0600.
2. **Versioning:** `meta(key, value)` with `envId` (a ULID minted on first open, never changed by this plan) and `createdAt`; `migrations(chain, n, appliedAt)`. **Migrations come in independent per-area chains** (spec §3.1): `control/migrations/<chain>/0001.ts…`, each frozen and ordered within its chain, each chain with its own `formatVersion.<chain>`/`minReaderVersion.<chain>` in `meta`, no foreign keys across chains. This plan owns `core` (Task 1: `meta`, `migrations`, `server_ids`, `audit`) and `auth` (Task 9: `credentials`); 5C-1 owns `jobs`. A chain registers itself with `registerChain(name, migrations)` before `openControlStore` runs them. `server_ids(serverId PRIMARY KEY, envId, since)` gets the current `server-id` at every start. `VACUUM INTO control.db.pre-migrate.bak` before the first pending migration (the newest two kept); a chain's `minReaderVersion` above ours → `CONTROL_DB_TOO_NEW`.
3. **`PRAGMA quick_check` at open;** a failure moves the files to `control.db.corrupt-<ulid>`, starts empty (a new `envId`) and writes `audit {kind: "control.reset"}` first.
4. **Audit:** `audit(id, at, kind, principalKind, principalId, credentialKind, outcome, code, remote, count, details)`, indexed on `at` and `kind`. `auth.fail` aggregates **per `(credentialKind, remote)` per minute** into one row with `count`. Retention 180 days or 100,000 rows, swept at start and hourly, **≤ 500 rows per `DELETE`**, one batch per event-loop turn. `remote` stays `NULL` in 5A.

### Keys, crypto and the handshake

5. **Layout:** `<server-data>/auth/` (0700; owner-only DACL on Windows) holds `local.key`, `console.key` (base64url of 32 random bytes plus `\n`, 0600, rewritten temp-then-rename at every start **before** any listener opens) and `credential.master` (created once). `<server-data>/run/` (0700) holds only `fin.sock`.
6. **MACs** (HMAC-SHA-256; each variable field preceded by its length as `u16` big-endian; nonces as their 32 raw bytes; `envId` and `endpoint` as UTF-8):
   - `proof = HMAC(K, "finisherd/hello/v2" ‖ envId ‖ endpoint ‖ clientNonce ‖ serverNonce)`;
   - `mac = HMAC(K, "fin/prove/v2" ‖ envId ‖ endpoint ‖ serverNonce ‖ clientNonce)`;
   - `credentialKey(kind, id) = HMAC(credential.master, "fin/credential-key/v1" ‖ kind ‖ id)`.
   Comparison is constant time (`timingSafeEqual` after a length check; `Mac::verify_slice` in Rust).
7. **The canonical endpoint** is the socket's `realpath`, or the pipe name lower-cased (`\\.\pipe\finisher-<user>[-<hash>]`). The server returns it in the hello; the client checks it equals the canonical form of what it connected to (`endpointMismatch` otherwise).
8. **`server.hello {clientNonce?, env?, endpoint?, credential?}`.** `credential` defaults to `{kind: "local"}`; `{kind: "console"}`, `{kind: "agent"|"mcp", id}` name the others. With a `clientNonce` the answer carries `envId`, `endpoint`, `serverNonce` and the `proof` for the named credential; an unknown, expired or revoked credential gets a `serverNonce` and no `proof`. A client-sent `env` that is not ours gets no proof either.
9. **`auth.prove {mac}`** binds the principal and answers `{principal}`; a second `auth.prove` on a proved connection is `INVALID_REQUEST`; five failures close the connection; each failure is audited (Decision 4).
10. **`fin`'s handshake and retry** (spec §4.2): on `noProof`, `badProof`, `envMismatch` or `endpointMismatch`, `fin` re-reads `finisherd.json` and its key file **once**, reconnects and retries the handshake. A second failure is `SERVER_UNTRUSTED` (exit 7, `details {endpoint, reason}`); a successful retry after a changed key is silent (`keyStale` is logged at debug level only).
11. **Credential discovery in `fin`:** `FIN_CREDENTIAL` (a credential file, spec §4.1), else `<server-data>/auth/local.key`. Unreadable or malformed → no credential. *Amended after the Task 3+4 review:* with no credential `fin` sends nothing: it fails locally with `UNAUTHENTICATED {reason: noCredential}` (exit 7), `fin whoami` answers `anonymous` and diagnostics send nothing, so an unproved server never receives a request.

### Connections and the chain

12. **Connection ids come from Effect RPC:** the server protocol's per-connection client id, passed to handlers in their options. `Connections` maps it to `{transport, principal, hello?, failures, peer?}` and drops the entry on disconnect. Task 3 starts by reading Effect 4.0.6's `RpcServer` sources to confirm the field names and the disconnect hook, and records them at the top of `connections.ts`.
13. **Decoding stays at the protocol level.** A frame that does not decode, an unknown tag or a payload that fails its schema answers `INVALID_REQUEST` (exit 3) from the protocol layer, never a defect. Frames over 16 MiB answer `FRAME_TOO_LARGE` (exit 2) and close the connection. Client headers with the `x-fin-` prefix are dropped.
14. **The chain** (`chain/chain.ts`, spec §4.4) wraps every handler once (`withChain(handlers)` in `handlers.ts`), unary and stream alike: authenticate → resolve → authorise → policy → handler → after. Resolvers and policies are pure functions of `(principal, payload, deps)` returning `Effect`s; the handler receives the **resolved** payload.
15. **Resolvers** (`chain/resolve.ts`): branch references through `engine.resolveBranch` (the existing 4B call) to ids; `project.open.dir` → `{projectId, dir}` from `finisher.project.json` (a missing or unreadable file: the handler's existing error); `project.create.dir` → `{dir, parent}` canonicalised; paths → `realpath`ed absolute paths (`INVALID_PATH` for relative or non-normalised input, Task 10's helper); each RPC's project. A resolver that needs an engine on a project not open answers `PROJECT_NOT_FOUND` before authorisation, except `project.open` itself.
16. **Policy** (`chain/policy.ts`) returns `allow | deny(code) | ask(reason)`. 5A ships **agent branch limits**: for `agent`/`mcp` principals with `branches` other than `"*"`, a write whose resolved target is outside them is `BRANCH_NOT_ALLOWED` (`"own"` = branches whose `branch` record's author is this credential's author). `ask` answers `APPROVAL_REQUIRED {approvalId}` (exit 7) with an approval id minted in memory; no 5A rule asks, and a test rule covers it.
17. **The write guard** (spec §4.4 stage 6): every write call from `finisherd` to `fin-node` (`commit`, every 4B history and branch call, `recover` when it writes) passes `guard {denyBranches: [ids], allowBranches?: [ids]}` computed by the policy stage. `fin-store` checks the record's target branch id against it **inside `Store::write`'s IMMEDIATE transaction**, before planning; a refusal is `BRANCH_NOT_ALLOWED` or `BRANCH_PROTECTED` (5E) and writes nothing. In 5A `denyBranches` is always empty (5E fills it) and `allowBranches` comes from Decision 16.

### Scopes

18. **`RPC_SCOPES: { [T in FinRpcTag]: { scopes: Scope[]; resolve?: ResolverName; escalate?: (resolved) => Scope[] } }`** in `packages/contracts/src/scopes.ts`; the mapped type makes a missing RPC a type error, and a test compares the runtime keys with `FinRpcs.requests.keys()`. All listed scopes are required.
19. **The table** is spec §4.5. In particular: `project.open` needs `project.read` when the resolved project is already open here and `project.write` otherwise; `asset.probe` needs `project.write` and a `projectId` (protocol 2), and its resolved path must be inside the project folder, its `workspace/`, or (5D) a library root for principals with roots; `fonts.fetch` needs `fonts.fetch`; `diagnostics.record` needs `diagnostics.record`; `auth.revokeSelf` needs `server.read` and takes no id.
20. **Defaults** (spec §4.5): `console` all; `local` all but the human-only scopes (it keeps `branch.protected` and `review.decide`); `agent`/`mcp` exactly their credential's scopes, never a human-only one; `anonymous` only `public`.
21. **Escalations in 5A:** `project.takeover {confirm: true}` and `project.open {relocate: true}` → `project.confirm`; `diagnostics.project {collect: "default"}`, `diagnostics.export {content: true}`, every `diagnostics.consent` → `diagnostics.admin`. **Training on** is refused for every principal kind but `device` (none exist in 5A) with `CONSENT_UI_ONLY`, before the human check.
22. **Non-human `diagnostics.export`** without `content: true` is served redacted (content and blob references removed) with `redacted: true`.

### Tickets and the WebSocket

23. **`auth.ticket {ttlS?}`** (default 30, max 60) → `{ticket, expiresAt}`; 32 random bytes base64url; kept as `SHA-256(ticket) → {principal, expiresAt}`; swept every 10 s; one use.
24. **The upgrade** (`upgrade.ts`), in order: path `/rpc`; `Sec-WebSocket-Protocol` must offer `finisher.v2` (another `finisher.v<n>` → HTTP 426 with `{"protocol":{"min":2,"max":2}}`) and one `finisher.ticket.<t>`; `Host` in the allow-list `127.0.0.1:<port>`, `localhost:<port>`, `[::1]:<port>` (403); `Origin` absent, one of those origins, or in `settings.json` `server.origins` (403); then the ticket is taken (401 if not live). The server answers with `Sec-WebSocket-Protocol: finisher.v2`.
25. **`finisherd.json`** becomes `{pid, wsPort, socket, protocolVersion: 2, envId, diagnostics: {enabled}}`; no `token`.

### Protocol 2

26. `PROTOCOL_VERSION = 2` in contracts and `fin`; `tx.commit.generation` and `ops.subscribe.generation` required; `asset.probe {projectId, path}`; the new hello.

### Diagnostics

27. **Sources and actors** (spec §3.4): `sourceOf(principal)` replaces D4's per-transport `RpcSource`; `actorFor(principal, headers)` replaces D6's `callMeta(headers)`. `Source` gains `console` and `mcp`; `diagnostics.record` allows `mcp` the kinds `fin` has (`cli.command`, `error.panic`) and `console` none. The D4 rate limit keys by principal (`local`, `console`, `agent:<id>`, `mcp:<id>`).
28. `fin diagnostics consent training on` stays a local usage error (D14b, exit 2). `fin diagnostics consent diagnostics …`, `purge`, `project default` and `export --content` reach the server and answer `HUMAN_ONLY`.

### Console

29. **Dispatch:** `finisherd` with no argument, `serve` or only flags serves; `status`, `stop`, `audit`, `diagnostics`, `project`, `agent`, `mcp` run console commands against `finisherd.json`'s socket with `console.key`.
30. **Read-only and service commands** (`status`, `audit` listing, `stop`) run without a TTY. **Mutating human-only commands** require `process.stdin.isTTY && process.stdout.isTTY` (`CONSOLE_NEEDS_TTY`, exit 7) and a typed challenge: the console calls `console.challenge` (→ a 6-character Crockford code valid 60 s, bound to this connection), shows what will happen and the code, and sends the RPC with `challenge: <typed code>` only after the person types it. The server refuses a mutating human-only RPC from a `console` principal without a valid, unused challenge of the same connection (`CONFIRMATION_REQUIRED`, exit 7). Prompts and reads go through an injected `Prompt` interface so unit tests drive them without a terminal.
31. **`finisherd diagnostics consent training on`** answers `CONSENT_UI_ONLY` (exit 2) with D14b's fix in words; the console never offers it.

### Credentials

32. **`credentials(id, kind, label, scopes, projects, branches, createdBy, createdAt, lastUsedAt, idleExpiryS, absoluteExpiryAt, revokedAt)`.** Defaults: `agent` 24 h idle and 7 days absolute; `mcp` 30 days idle and no absolute limit (spec §4.1 gives the reasons). `lastUsedAt` is written at most once a minute. Expired or revoked credentials get no proof; revocation and expiry close live connections (a sweep every 60 s for expiry).
33. **Mint defaults** (spec §4.5) when no scopes are given: `server.read`, `project.read`, `project.write`, `review.write`, `jobs.read`, `jobs.submit`, `library.read`, `library.scan`, `diagnostics.read`, `diagnostics.record`; `branches` defaults to `"*"` for hand-run harnesses (`--branch own` or ids narrow it); human-only scopes are refused (`SCOPE_DENIED` naming them). No scope grants auto merge: sub-project 7's `auto` merge policy is a server-internal actor (spec §8.5).
34. **`FIN_CREDENTIAL`** joins `childEnv`'s `ChildEnvExtra` (arch §8.4 amended); its value is a path.

### Hardening and the pipe

35. **`INVALID_PATH`** (Task 10): `project.create.dir`, `project.open.dir`, `asset.probe.path` and `project.recover.importDir` must be absolute and normalised; Windows drive-relative (`C:foo`) and rooted-without-drive (`\foo`) forms are refused too.
36. **`fin` deadline and frame cap** (Task 10): `FIN_DEADLINE_S` (default 300 s, streams excluded) → `SERVER_TIMEOUT {deadline}`; a server line over 16 MiB → `PROTOCOL_ERROR {max}`.
37. **`fin`'s test hooks** (`FIN_OFFLINE_SNAPSHOT`, `FIN_TEST_COMMIT_GATE`) are read only with `fin`'s `test-hooks` feature, enabled for its own tests through a self dev-dependency; `scripts/check-test-hooks.mjs` covers `fin`. `FIN_NODE_BINARY_DIR` (the JS loader) stays a TASKS item for sub-project 9.
38. **The Windows pipe listener** (Task 11, spec §4.3): `fin-node` `PipeServer(name, {allow: [sid]})` creates instances with `PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED`, `FILE_FLAG_FIRST_PIPE_INSTANCE` on the first, `PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_REJECT_REMOTE_CLIENTS`, and the SDDL `D:P(A;;GA;;;<user SID>)(A;;GA;;;SY)` plus `(A;;GRGW;;;<sid>)` per allowed SID; it accepts on a dedicated thread, reads each client's pid and token user SID, closes clients that are neither the user nor an allowed SID, and hands the rest to JS as duplex streams that `transport.ts` wraps as an Effect `Socket`. A first-instance failure at start is `PIPE_SQUATTED` (exit 1).

## Owner decisions (2026-11-07)

The owner answered spec §12 as recommended. This plan implements **decisions 2, 5 and 6**:
- **2:** human-only operations are human-only now, with the TTY-gated `finisherd` console as the human interface (Decision 21's escalations, Tasks 7 and 8);
- **5:** `local` keeps `branch.protected` and `review.decide` (Decision 20);
- **6:** human-only is advisory against raw-shell agents until sub-project 7's sandbox (7C), **documented in the README's security model** (Task 13); agent and MCP credentials are offered, not forced, so `fin` run by a person needs no mint.

Decisions 1, 3 and 4 do not affect 5A (review state is 5E; browsers and the audit log's addresses are 5B; 5A's `auth.fail` rows carry no remote address, since every 5A transport is local).

**Cross-cutting decisions that touch 5A:** Node 24 is the target (Tech Stack above; the switch itself is a `TASKS.md` item, not a task here); the `auto` merge policy adds no scope (it is a server-internal `policy` actor, spec §8.5) and is 5E's and sub-project 7's, so 5A changes nothing for it.

## Review Focus

1. **The handshake is mutual, bound and replay-proof:** fresh nonces, `envId` and endpoint in both MACs, constant-time comparison, no request before the proof, keys rotated before listening, one retry after a restart and no more.
   → Task 2 `handshake_vectors_match`, `keys_rotate_before_listen`; Task 3 `relayed_proof_fails_on_other_endpoint`; Task 4 `restart_between_read_and_connect_is_retried`, `squatter_never_receives_a_request`.
2. **Nothing trusts a transport:** unproved connections only say hello (from Task 4 on); a WebSocket needs a ticket in `Sec-WebSocket-Protocol`; a `local` ticket yields `local`; `x-fin-` headers cannot set anything.
   → Task 4 `unproved_connection_only_hello`; Task 6 `ticket_not_accepted_in_query`, `local_ticket_is_local_principal`.
3. **The chain checks resolved state:** a branch name, alias or rename cannot slip past a rule; the guard re-checks inside the write transaction; reach covers `project.open`/`create` by directory and `asset.probe` by project.
   → Task 5 `rename_between_resolve_and_write_is_caught`, `name_and_id_resolve_the_same`, `probe_outside_project_is_path_not_allowed`, `open_needs_write_unless_attached`.
4. **Every RPC has scopes and a resolver; human-only holds for every non-human principal; fix texts carry no command.**
   → Task 5 `rpc_scopes_cover_every_rpc`, `human_only_matrix`, `fix_texts_contain_no_command`.
5. **No training opt-in from any command line:** `fin`, the console and every RPC source but a device.
   → Task 7 `training_on_refused_for_every_non_device`; Task 8 `console_training_on_is_consent_ui_only`.
6. **Console confirmations:** no TTY → refused; the server requires the connection's own fresh challenge.
   → Task 8 `mutation_without_challenge_is_refused`, `challenge_is_single_use_and_per_connection`, `non_tty_is_console_needs_tty`.
7. **Sources and actors from principals;** `agent`/`mcp` cannot claim another session.
   → Task 7.
8. **Windows:** private server data, the native pipe (first instance, remote rejection, DACL, peer SID), graceful stop, all non-elevated.
   → Tasks 2, 11, 14.
9. **No secrets leak** anywhere. → Task 12 leak grep.
10. **The event loop:** no checkpoint on the main connection; statement p99 ≤ 1 ms; batched deletes. → Task 1.
11. **Every commit green.** → each task's Step "Run"; the reviewer checks out each commit of batch B and runs the gates.

---

### Task 1: The control store, the environment id and the audit log — tier: sonnet — can start now

**Files:**
- Create: `apps/finisherd/src/control/{db.ts, checkpoint-worker.ts, migrations.ts (chains, `registerChain`), migrations/core/0001.ts, audit.ts}`.
- Modify: `apps/finisherd/src/main.ts` (open the store in `serve` after the server lock and before the transports; a layer; record `server_ids`), `TASKS.md`.
- Test: `apps/finisherd/test/control.test.ts`, `apps/finisherd/test/audit.test.ts`, `apps/finisherd/scripts/control-budget.ts`.

**Interfaces:**
```ts
// control/db.ts
export const CONTROL_FORMAT_VERSION = 1;
export const CONTROL_MIN_READER_VERSION = 1;
export interface ControlStore {
  readonly db: DatabaseSync;
  readonly envId: string;
  tx<A>(f: () => A): A;                                   // BEGIN IMMEDIATE … COMMIT; rollback on throw
  deleteInBatches(sql: string, params: unknown[], batch?: number): Effect.Effect<number>; // ≤ 500 rows per turn
  close(): Promise<void>;                                 // stops the checkpoint worker, final TRUNCATE
}
export function openControlStore(serverData: string, opts: { serverId: string }): ControlStore; // CONTROL_DB_TOO_NEW
export class ControlStoreTag extends Context.Tag("ControlStore")<ControlStoreTag, ControlStore>() {}
export const layerControlStore: (serverData: string, serverId: string) => Layer.Layer<ControlStoreTag, ServerStartError>;

// control/audit.ts
export interface AuditInput { kind: AuditKind; principal: { kind: string; id: string }; credentialKind?: string;
  outcome: "ok" | "denied" | "failed"; code?: string; details?: Record<string, unknown> }
export class Audit {
  constructor(store: ControlStore, clock?: () => number);
  append(e: AuditInput): void;                            // never throws; failures counted
  list(q: { since?: number; kind?: AuditKind; cursor?: string; limit?: number }): { entries: AuditEntry[]; next?: string };
  purge(q: { before?: number; all?: boolean }): Effect.Effect<number>;
  sweep(): Effect.Effect<void>;
  failures(): number;
}
```
Decisions 1–4.

- [ ] **Step 1: Write failing tests:**
  - `opens_wal_0600_with_meta_and_env_id` (unix mode; `wal`; `envId` is a ULID and survives a reopen);
  - `main_connection_never_autocheckpoints` (`PRAGMA wal_autocheckpoint` reads 0; 10k inserts leave the WAL growing until the worker runs; the worker's checkpoint shrinks it);
  - `server_ids_map_every_server_id` (two different server ids over two starts → two rows, one `envId`);
  - `migrations_run_once_in_order_with_backup`, `chains_are_independent` (two test chains registered in either order, each applied once; a newer `minReaderVersion` in one chain refuses the start, naming the chain), `newer_min_reader_refuses_start`, `corrupt_db_is_moved_aside_and_audited` (and gets a new `envId`), `busy_maps_to_control_db_busy`;
  - `audit_append_list_cursor_purge`, `auth_fail_aggregates_per_credential_kind_per_minute` (100 failures on 100 connections in one minute → one row, `count` 100), `retention_by_age_and_rows`, `deletes_are_batched` (a purge of 10k rows runs ≥ 20 statements and yields between them), `audit_failure_never_throws`;
  - `control_store_budget` (100k audit rows; prepared-statement p99 ≤ 1 ms on Linux; the test asserts ≤ 5 ms for CI stability and prints the measure).
- [ ] **Step 2: Implement** with APIs present in Node 24.22, the target (check each against Node 24's `node:sqlite` docs; note them in `db.ts`, where the Node 24 switch's verification reads them).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): control store with an environment id, frozen migrations and a local audit log`.

### Task 2: Key files, private directories, `run/` and the handshake crypto — tier: opus — can start now

**Files:**
- Create: `apps/finisherd/src/auth/{keys.ts, crypto.ts}`, `crates/fin-node/src/private_dir.rs`, `tests/fixtures/auth/handshake.json`.
- Modify: `crates/fin-node/src/lib.rs`, `crates/fin-node/Cargo.toml` (`windows-sys` features), `packages/fin-node` (typings), `apps/finisherd/src/config.ts` (`socketPath` → `<server-data>/run/fin.sock` on unix; `ensureServerData` uses the private-dir checks on Windows; `finisherd.json` gains `envId`), `apps/finisherd/src/main.ts` (create `auth/` and `run/`; rotate keys after the server lock, before `claimSocketPath`), every test and script that builds the socket path by hand (grep `fin.sock`).
- Test: `apps/finisherd/test/keys.test.ts`, `apps/finisherd/test/handshake-vectors.test.ts`, `crates/fin-node/tests/private_dir.rs`.

**Interfaces:**
```ts
// auth/keys.ts
export interface Keys { readonly local: Buffer; readonly console: Buffer; readonly credentialMaster: Buffer }
export function authDir(serverData: string): string;      // <server-data>/auth
export function runDir(serverData: string): string;       // <server-data>/run
export function rotateKeys(serverData: string): Keys;     // local+console new; master kept; private dir; 0600
export function readKeyFile(path: string): Buffer;        // 32 bytes or throws

// auth/crypto.ts
export const LABEL = { hello: "finisherd/hello/v2", prove: "fin/prove/v2", credentialKey: "fin/credential-key/v1" } as const;
export function nonce(): Buffer;
export function helloProof(key: Buffer, envId: string, endpoint: string, clientNonce: Buffer, serverNonce: Buffer): Buffer;
export function proveMac(key: Buffer, envId: string, endpoint: string, serverNonce: Buffer, clientNonce: Buffer): Buffer;
export function credentialKey(master: Buffer, kind: "agent" | "mcp", id: string): Buffer;
export function sameMac(a: Buffer, b: Buffer): boolean;
export function canonicalEndpoint(endpoint: string, platform?: NodeJS.Platform): string;
export const b64u: { encode(b: Buffer): string; decode(s: string): Buffer | null };
```
```rust
#[napi] pub fn check_private_dir(path: String) -> PrivateDirReport;   // Windows: owner and broad ACEs; elsewhere ok
#[napi] pub fn make_private_dir(path: String) -> napi::Result<()>;    // Windows: protected owner-only DACL; elsewhere 0700
```
Decisions 5–7.

- [ ] **Step 1: Write failing tests:**
  - `handshake_vectors_match` (TS over the vector file; Task 4 adds the Rust twin);
  - `length_prefixes_make_fields_unambiguous` (moving a byte between `envId` and `endpoint` changes the MAC);
  - `keys_rotate_on_every_start`, `credential_master_survives_restart`, `key_files_are_0600_dirs_0700` (unix), `malformed_key_file_is_refused`;
  - `keys_rotate_before_listen` (the start order: lock → store → keys → socket);
  - `socket_lives_in_run_dir`, `run_dir_holds_only_the_socket`;
  - `canonical_endpoint_resolves_symlinks_and_lowercases_pipes`;
  - Windows: `private_dir_refuses_everyone_ace`, `private_dir_refuses_other_owner`, `made_private_dir_passes_check`, `server_data_unsafe_on_windows`.
- [ ] **Step 2: Implement.** Windows DACL: `D:P(A;OICI;FA;;;<user SID>)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)` with `SetNamedSecurityInfoW` and `PROTECTED_DACL_SECURITY_INFORMATION`; `check_private_dir` reports allow ACEs for S-1-1-0, S-1-5-32-545, S-1-5-11 or another user. Nothing verifies a proof yet; the server only rotates and holds the keys.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` (the socket moved) → PASS. **Commit** `feat(finisherd): rotating keys in auth/, the socket in run/, private server data on Windows`.

### Task 3: The connection-aware protocol, principals and the handshake RPCs (not yet enforced) — tier: opus — after D4 and D6

**Files:**
- Create: `apps/finisherd/src/{connections.ts, principal.ts}`, `apps/finisherd/src/auth/handshake.ts`, `packages/contracts/src/auth.ts`.
- Modify: `apps/finisherd/src/{transport.ts, main.ts, handlers.ts}`, `packages/contracts/src/{rpc.ts, index.ts}` (the new hello fields, optional so protocol-1 `fin` keeps working; `auth.prove`).
- Test: `apps/finisherd/test/protocol.test.ts`, `apps/finisherd/test/support/ndjson-client.ts` (`hello`, `prove` helpers).

**Interfaces:**
```ts
// contracts/auth.ts
export const PrincipalKind = Schema.Literals(["anonymous", "local", "console", "device", "agent", "mcp", "plugin"]);
export const PrincipalSchema = Schema.Struct({ kind: PrincipalKind, id: Schema.String, scopes: Schema.Array(ScopeSchema),
  projects: Schema.Union([Schema.Literal("*"), Schema.Array(Schema.String)]),
  branches: Schema.Union([Schema.Literal("*"), Schema.Array(Schema.String)]), human: Schema.Boolean });
export type Principal = typeof PrincipalSchema.Type;

// connections.ts
export interface Connection { readonly clientId: number; readonly transport: "socket" | "pipe" | "ws"; readonly openedAt: number;
  principal: Principal; hello?: { clientNonce: Buffer; serverNonce: Buffer; credential: CredentialRef }; failures: number;
  peer?: { pid?: number; sid?: string } }
export class Connections {
  get(clientId: number): Connection;                     // created on first sight
  close(clientId: number): void;
  closeWhere(pred: (c: Connection) => boolean): number;  // ends streams with CREDENTIAL_REVOKED
  count(): Record<string, number>;
}
export const ENFORCE: { proof: boolean };                // false in this task; Task 4 sets it true
```
Decisions 8–9, 12–13. **Not enforced yet:** a connection that never proves is treated as `local` (the status quo) and audited once per connection as `auth.legacy`, so today's `fin`, tests and e2e keep working in this commit.

- [ ] **Step 1: Read Effect 4.0.6's `RpcServer`** (`node_modules/effect/dist/esm/unstable/rpc/RpcServer.js` or wherever the package puts it): the handler options (`clientId`, `headers`), the protocol's client-connect/disconnect hooks, and where payload decoding fails. Record the answer at the top of `connections.ts`.
- [ ] **Step 2: Write failing tests:**
  - `each_connection_gets_its_own_principal`;
  - `prove_binds_local_and_console`, `wrong_mac_fails_and_five_failures_close`, `second_prove_is_invalid_request`;
  - `hello_without_nonce_has_no_proof`, `unknown_credential_gets_no_proof`, `foreign_env_gets_no_proof`;
  - `relayed_proof_fails_on_other_endpoint` (a MAC computed for one canonical endpoint is refused on another);
  - `x_fin_headers_are_dropped`;
  - `bad_frame_is_invalid_request_not_defect` (invalid JSON, unknown tag, a payload missing a field);
  - `oversized_frame_is_frame_too_large_then_closed`;
  - `disconnect_removes_connection`;
  - `unproved_connection_is_legacy_local_and_audited` (this task only; Task 4 replaces it).
- [ ] **Step 3: Implement.** The wire format stays NDJSON `Request`/`Exit`/`Chunk`/`Ack`/`Ping`/`Pong`.
- [ ] **Step 4: Run** the gates and `rtk pnpm e2e` (unchanged clients still pass) → PASS. **Commit** `feat(finisherd): connection-aware RPC with proved principals and the handshake RPCs`.

### Task 4: `fin` proves; the server requires proofs — tier: sonnet — after Task 3

**Files:**
- Create: `crates/fin/src/auth.rs`, `crates/fin/src/commands/whoami.rs`, `crates/fin/tests/cli_auth.rs`.
- Modify: `crates/fin/src/client.rs` (`auth::handshake` after connect: call-site lines), `crates/fin/src/output.rs` (`DENIED = 7`, `DENIED_CODES`), `crates/fin/src/{registry.rs, map.rs, main.rs}` (`whoami`), `crates/fin/Cargo.toml` (`hmac`, `sha2`, `base64`), `apps/finisherd/src/connections.ts` (`ENFORCE.proof = true`; remove the legacy principal), `apps/finisherd/test/support/ndjson-client.ts` and every TS test that opens a raw socket (prove first), `scripts/e2e/lib.mjs` (its socket helper proves), `crates/fin/tests/support/mod.rs` (a fake server without a key; a helper that reads the test server's keys).
- Test: `crates/fin/tests/cli_auth.rs`, `apps/finisherd/test/protocol.test.ts` (enforced cases).

**Interfaces:**
```rust
pub enum CredentialKind { Local, Console, Agent, Mcp }
pub struct Credential { pub kind: CredentialKind, pub id: Option<String>, pub env: Option<String>, key: [u8; 32] }
pub fn discover(server_data: Option<&Path>) -> Option<Credential>;     // FIN_CREDENTIAL, else <server-data>/auth/local.key
pub fn handshake(conn: &mut Client, cred: Option<&Credential>, expect: &Expect) -> Result<Principal, CoreError>;
pub struct Expect { pub env_id: Option<String>, pub endpoint: String }
pub fn hello_proof(key: &[u8; 32], env: &str, endpoint: &str, cn: &[u8; 32], sn: &[u8; 32]) -> [u8; 32];
pub fn prove_mac(key: &[u8; 32], env: &str, endpoint: &str, sn: &[u8; 32], cn: &[u8; 32]) -> [u8; 32];
```
`fin whoami` → `{principal}` (exit 0, `anonymous` included). Decisions 10–11; spec §3.5.

- [ ] **Step 1: Write failing tests:**
  - `handshake_vectors_match_rust`;
  - `fin_works_as_local` (every existing online suite passes with proofs required);
  - `unproved_connection_only_hello` (TS: any other RPC → `UNAUTHENTICATED`, the handler never runs);
  - `fin_refuses_server_without_proof` (a fake server: no `proof`, a wrong one, a foreign `envId`, another endpoint → exit 7 `SERVER_UNTRUSTED` with the reason, after exactly one retry; the fake records nothing after the second hello);
  - `restart_between_read_and_connect_is_retried` (rotate the keys between `fin` reading `local.key` and connecting: the retry succeeds, exit 0);
  - `squatter_never_receives_a_request` (a fake listening on the socket a stale `finisherd.json` names);
  - `no_credential_is_unauthenticated` (`FIN_SERVER` only: exit 7, the fix names `FIN_CREDENTIAL` and server data in words);
  - `denied_codes_exit_7`, `map_still_under_budget`.
- [ ] **Step 2: Implement**, and flip `ENFORCE.proof` in the same commit.
- [ ] **Step 3: Run** `rtk cargo test -p fin` (`FIN_REQUIRE_SERVER=1`), the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(fin,finisherd): fin proves its credential; the server requires proofs; exit code 7`.

### Task 5: The middleware chain, scopes, reach, policy and the write guard — tier: opus — after Task 4

**Files:**
- Create: `packages/contracts/src/scopes.ts`, `apps/finisherd/src/chain/{chain.ts, resolve.ts, authorize.ts, policy.ts, fixes.ts}`, `crates/fin-store/src/guard.rs`, `crates/fin/tests/support/console.rs` (a console-proved RPC helper for Rust tests).
- Modify: `apps/finisherd/src/handlers.ts` (`withChain`), `crates/fin-store/src/write.rs` (check the guard inside the transaction: a few lines), `crates/fin-node/src/{tasks.rs, history.rs}` (accept `guard` beside the request; generated type), `apps/finisherd/src/{projects.ts, history.ts}` (pass the guard: call-site lines), `packages/contracts/src/errors.ts`; tests that used `fin project takeover --confirm` or `fin project open --relocate` (`crates/fin/tests/cli_online.rs`, the `project-open-relocate` RPC fixture in `crates/fin/tests/rpc_contract.rs`, `scripts/e2e/*.mjs`) move to the console helper **in this commit**.
- Test: `packages/contracts/src/scopes.test.ts`, `apps/finisherd/test/chain.test.ts`, `crates/fin-store/tests/guard.rs`.

**Interfaces:**
```ts
// contracts/scopes.ts
export const SCOPES = ["public", "server.read", "project.read", "project.create", "project.write", "project.admin",
  "project.confirm", "fonts.fetch", "branch.protected", "review.write", "review.decide", "jobs.read", "jobs.submit",
  "jobs.control", "library.read", "library.scan", "library.admin", "diagnostics.read", "diagnostics.record",
  "diagnostics.admin", "settings.admin", "auth.admin", "plugin.install"] as const;
export type Scope = (typeof SCOPES)[number];
export const HUMAN_ONLY: ReadonlySet<Scope>;   // project.confirm, library.admin, diagnostics.admin, settings.admin, auth.admin, plugin.install
export type FinRpcTag = RpcGroup.Rpcs<typeof FinRpcs>["_tag"];
export interface RpcRule { scopes: Scope[]; resolve?: ResolverName; escalate?: (resolved: unknown) => Scope[] }
export const RPC_SCOPES: { readonly [T in FinRpcTag]: RpcRule };
export function defaultScopes(kind: PrincipalKind): ReadonlyArray<Scope>;

// chain/chain.ts
export type Outcome = { allow: true; guard: WriteGuard } | { deny: FinError } | { ask: { reason: string } };
export function withChain<H extends Record<FinRpcTag, (p: any, o: any) => any>>(handlers: H, deps: ChainDeps): H;
```
```rust
// fin-store/src/guard.rs
#[derive(Deserialize, JsonSchema, Default)] pub struct WriteGuard { pub deny_branches: Vec<String>, pub allow_branches: Option<Vec<String>> }
pub(crate) fn check(guard: &WriteGuard, target: &BranchId) -> Result<(), StoreError>; // BRANCH_NOT_ALLOWED / BRANCH_PROTECTED
```
Decisions 14–22.

- [ ] **Step 1: Write failing tests:**
  - `rpc_scopes_cover_every_rpc` (runtime keys = `FinRpcs.requests.keys()`; every rule names its resolver where the payload has a branch, directory, path or project);
  - `streams_pass_the_chain` (`ops.subscribe` refused before it starts);
  - `human_only_matrix` (every human-only scope and escalation × `anonymous`, `local`, `agent`, `mcp`, `console`: only `console` passes);
  - `all_listed_scopes_are_required` (a rule with two scopes refuses a principal holding one);
  - `open_needs_write_unless_attached` (an `agent` with `project.read` only: attaching to an open project works, opening a closed one is `SCOPE_DENIED`);
  - `create_and_open_reach_by_directory` (an `agent` with `projects: [A]`: opening B's folder is `PROJECT_NOT_FOUND`; creating outside its roots is `PATH_NOT_ALLOWED`);
  - `probe_outside_project_is_path_not_allowed` (and inside the project folder or `workspace/` works; `local` unrestricted);
  - `name_and_id_resolve_the_same` (a rule on `br3` applies to `feature`, `feature#br3`, `br3`);
  - `rename_between_resolve_and_write_is_caught` (an injected rename between the policy stage and the store call: the in-transaction guard refuses, nothing written; `crates/fin-store/tests/guard.rs` covers every write path: commit, undo, redo, checkpoint, merge, pick, revert, branch create/set, recover);
  - `agent_branch_limits` (`branches: ["own"]`: a commit to `main` is `BRANCH_NOT_ALLOWED`, to a branch it created works);
  - `ask_answers_approval_required` (a test policy);
  - `check_order_is_fixed` (a request both unauthorised and malformed answers in the chain's order);
  - `denials_are_audited` (tag, scope or rule, principal kind; no payload);
  - `fix_texts_contain_no_command` (every human-only code's fix: no `finisherd <word>`, `fin <word>` or backticks; addresses the user; `details.where` set).
- [ ] **Step 2: Implement.** Add `WriteGuard` to `crates/fin-store/examples/export_schema.rs` and regenerate. Move the confirm/relocate tests to the console helper in the same commit.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd,store): one middleware chain with scopes, resolvers, reach and an in-transaction write guard`.

### Task 6: Tickets, the WebSocket without a token, protocol 2 — tier: sonnet — after Task 5

**Files:**
- Create: `apps/finisherd/src/auth/tickets.ts`, `apps/finisherd/src/upgrade.ts`.
- Modify: `apps/finisherd/src/{transport.ts (remove `hasBearer`, `makeWebSocketServer`; `ws` in `noServer` mode behind `node:http`), config.ts (`ServerInfo` without `token`), main.ts, handlers.ts}`, `apps/finisherd/package.json` (`ws` pinned), `packages/contracts/src/{index.ts (`PROTOCOL_VERSION = 2`), rpc.ts (required generations; `asset.probe {projectId}`; `auth.ticket`, `auth.whoami`, `auth.revokeSelf`)}`, `crates/fin/src/endpoint.rs` (`PROTOCOL_VERSION = 2`), `crates/fin/src/commands/{asset.rs, media.rs}` (send `projectId`), `scripts/e2e/lib.mjs` (`connectWs` mints a ticket over the socket and sends it in `Sec-WebSocket-Protocol`), `apps/finisherd/scripts/capture-rpc-fixtures.ts`, the RPC fixtures, `crates/fin/tests/rpc_contract.rs`.
- Test: `apps/finisherd/test/tickets.test.ts`, `apps/finisherd/test/server.test.ts`, `scripts/e2e.test.mjs`.

**Interfaces:**
```ts
export class Tickets { mint(p: Principal, ttlS?: number): { ticket: string; expiresAt: number }; take(t: string): Principal | undefined; sweep(): void }
export type UpgradeResult = { ok: true; principal: Principal; protocol: "finisher.v2" } | { ok: false; status: 401 | 403 | 404 | 426; body?: unknown };
export function makeUpgradeCheck(opts: { port: () => number; tickets: Tickets; origins: () => ReadonlyArray<string> }): (req: IncomingMessage) => UpgradeResult;
```
Decisions 23–26.

- [ ] **Step 1: Write failing tests:**
  - `ticket_is_one_time`, `ticket_expires`, `ticket_ttl_capped_at_60`;
  - `ticket_in_subprotocol_header`, `ticket_not_accepted_in_query` (`?ticket=` is ignored: 401);
  - `ws_without_ticket_is_401`, `ws_wrong_version_is_426_with_range`, `ws_bad_host_is_403`, `ws_origin_matrix`, `host_and_origin_checked_before_ticket_is_consumed`;
  - `local_ticket_is_local_principal` (`auth.whoami` over the WebSocket);
  - `revoke_self_needs_no_id` (an agent credential revokes itself; it cannot name another);
  - `finisherd_json_has_no_token`;
  - `generation_is_required`, `probe_requires_project_id`.
- [ ] **Step 2: Implement** both sides and re-capture the fixtures in this commit.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd,fin): one-time WebSocket tickets, Host and Origin checks, protocol 2`.

### Task 7: Diagnostics from principals; human-only diagnostics; training on only from a device — tier: opus — after Task 6

**Files:**
- Modify: `crates/fin-core/src/events/{mod.rs (`Source::Console`, `Source::Mcp`), names.rs (`RPC_METHODS` += every new RPC), kinds.rs (`mcp` emits `cli.command` and `error.panic`; `console` none)}`, the regenerated schema and `generated.ts`, `apps/finisherd/src/diagnostics.ts` (`sourceOf`, `actorFor`, per-principal rate limit, consent rules, redacted export), `apps/finisherd/src/{main.ts, transport.ts}` (drop D4's per-transport `RpcSource` layers), `crates/fin/tests/cli_diagnostics.rs` and `scripts/e2e/diagnostics.mjs` (D6/D7: the refused calls now expect exit 7 or 2; the state-changing steps move to the console helper), and the console helpers of Task 5 (`crates/fin/tests/support/console.rs`, the TS test client, `scripts/e2e/lib.mjs`) fetch a `console.challenge` and send it, **in this commit**.
- Test: `crates/fin-core/tests/events.rs` (goldens), `apps/finisherd/test/diagnostics-principals.test.ts`.

**Interfaces:**
```ts
export function sourceOf(p: Principal): Source | undefined;     // local|agent → fin, mcp → mcp, console → console, device → ui
export function actorFor(p: Principal, headers: Headers): Actor;
```
Decisions 21–22, 27–28.

- [ ] **Step 1: Write failing tests:**
  - `source_follows_principal` (socket `local` and `local`-ticket WebSocket → `fin`; console → `console`; an `mcp` credential → `mcp`);
  - `agent_actor_is_its_session` (headers claiming `user` or another session are overridden; `call` kept);
  - `local_actor_keeps_headers`;
  - `training_on_refused_for_every_non_device` (`CONSENT_UI_ONLY` for `local`, `agent`, `mcp`, `console`);
  - `consent_off_and_unset_need_a_human` (`HUMAN_ONLY` for `local`; works for `console` with a challenge, Task 8; until Task 8 the console helper passes a test challenge through the same server check);
  - `purge_project_default_and_content_export_need_a_human`, `project_never_open_to_writers`;
  - `non_human_export_is_redacted`;
  - `rate_limit_is_per_principal`;
  - `rpc_methods_table_matches_contracts`.
- [ ] **Step 2: Implement.** `console.challenge` and the server's challenge check land here (Decision 30's server half), so the consent tests are real; Task 8 adds the console CLI around them.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(diagnostics): sources and actors from principals; human-only diagnostics; no training opt-in outside the app`.

### Task 8: The console — tier: sonnet — after Task 7

**Files:**
- Create: `apps/finisherd/src/console/{main.ts, ndjson.ts (shared with the test client), client.ts, prompt.ts (the `Prompt` interface, a TTY implementation), format.ts, commands/{status.ts, stop.ts, audit.ts, diagnostics.ts, project.ts, agent.ts, mcp.ts}}`, `apps/finisherd/src/server-admin.ts` (`server.status`, `server.stop`, `auth.audit.*`).
- Modify: `apps/finisherd/src/main.ts` (Decision 29 dispatch; `server.stop` interrupts the main fiber), `apps/finisherd/src/handlers.ts`, `packages/contracts/src/{auth.ts, rpc.ts}`.
- Test: `apps/finisherd/test/console.test.ts`.

**Interfaces:**
```text
finisherd [serve] [--server-data <dir>]
finisherd status [--json]              finisherd stop              finisherd audit [--since] [--kind] [--limit] [--json]
finisherd audit purge (--all | --before <date>)
finisherd diagnostics consent diagnostics <on|off|unset>     finisherd diagnostics consent training <off|unset>
finisherd diagnostics purge (--all | --project <id> | --before <date>)
finisherd diagnostics project default <projectId>            finisherd diagnostics export --content [filters]
finisherd project takeover <dir|projectId> --confirm         finisherd project open <dir> --relocate
finisherd agent mint --label <s> [--scope <s>]… [--project <id>]… [--branch own|<id>]… [--out <file>]
finisherd agent ls | revoke <id>       finisherd mcp setup --out <file> [--label <s>]
```
```ts
export interface Prompt { isInteractive(): boolean; show(text: string): void; ask(question: string): Promise<string> }
```
Decisions 29–31. The help text of each mutating command says it needs an interactive terminal and a typed code.

- [ ] **Step 1: Write failing tests** (through an injected `Prompt`, and by spawning the console for the non-TTY cases):
  - `serve_is_default_and_flags_still_serve`;
  - `console_proves_as_console`, `console_without_server_is_server_unavailable`;
  - `non_tty_is_console_needs_tty` (spawned with piped stdio, every mutating command exits 7 and changes nothing);
  - `typed_code_must_match` (a wrong code changes nothing; `yes |` is impossible because stdin is not a TTY);
  - `mutation_without_challenge_is_refused` (a raw console-proved RPC without `challenge` → `CONFIRMATION_REQUIRED`), `challenge_is_single_use_and_per_connection`, `challenge_expires_after_60s`;
  - `console_training_on_is_consent_ui_only` (exit 2; nothing written to `settings.json`);
  - `read_only_commands_run_without_tty` (`status`, `audit`), `stop_shuts_down_gracefully` (runs on Windows too);
  - `takeover_confirm_and_relocate_via_console`;
  - `console_never_prints_secrets`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): TTY-gated console with typed confirmations; graceful stop`.

### Task 9: Agent and MCP credentials — tier: opus — after Task 5

**Files:**
- Create: `apps/finisherd/src/auth/credentials.ts`, `apps/finisherd/src/control/migrations/auth/0001.ts` (`credentials`).
- Modify: `apps/finisherd/src/auth/handshake.ts` (credential hellos), `apps/finisherd/src/handlers.ts`, `apps/finisherd/src/env.ts` (`FIN_CREDENTIAL` in `ChildEnvExtra`), `packages/contracts/src/{auth.ts, rpc.ts}` (`auth.credential.mint|list|revoke`), `apps/finisherd/src/console/commands/{agent.ts, mcp.ts}` if Task 8 is in (otherwise Task 8 wires them).
- Test: `apps/finisherd/test/credentials.test.ts`, `apps/finisherd/test/env.test.ts`.

**Interfaces:**
```ts
export const CredentialFileSchema = Schema.Struct({ v: Schema.Literal(1), kind: Schema.Literals(["agent", "mcp"]),
  id: Schema.String, key: Schema.String, env: Schema.String });
export class Credentials {
  constructor(store: ControlStore, master: Buffer, audit: Audit, clock?: () => number);
  mint(by: Principal, req: { kind: "agent" | "mcp"; label: string; scopes?: Scope[]; projects?: string[];
    branches?: Array<"own" | string>; idleS?: number; absoluteS?: number | null }): { credential: CredentialRow; file: CredentialFile };
  keyFor(kind: "agent" | "mcp", id: string): Buffer | undefined;   // undefined when unknown, expired or revoked
  principal(kind: "agent" | "mcp", id: string): Principal | undefined;
  touch(id: string): void;                                         // lastUsedAt, at most once a minute
  revoke(by: Principal, id: string, conns: Connections): boolean;  // closes live connections
  sweepExpired(conns: Connections): number;
}
```
Decisions 32–34.

- [ ] **Step 1: Write failing tests:**
  - `mint_then_prove_as_agent_and_mcp` (scopes exactly as minted; `human: false`; source `fin` / `mcp`);
  - `mint_defaults_and_branches_kept` (no scopes → the default set; `branches: ["own"]` stored and enforced by the policy stage);
  - `mint_refuses_human_only_scopes`, `mint_needs_a_human`;
  - `key_is_never_stored`;
  - `idle_and_absolute_expiry` (injected clock: 24 h idle and 7 d absolute for `agent`; 30 d idle for `mcp`; expiry closes live connections within the 60 s sweep);
  - `revoke_closes_live_connections` (an open `ops.subscribe` ends with `CREDENTIAL_REVOKED` within 100 ms);
  - `credential_survives_server_restart`;
  - `agent_scopes_limit_rpcs`;
  - `mint_revoke_expire_are_audited`;
  - `child_env_passes_fin_credential_path`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): agent and MCP credentials with scopes, branch limits and expiry`.

### Task 10: RPC hardening from the sub-project 5 follow-ups — tier: sonnet — can start now

**Files:**
- Create: `apps/finisherd/src/paths.ts` (`requireAbsolute`).
- Modify: `apps/finisherd/src/handlers.ts` (call sites; Task 5's resolvers later call the same helper), `crates/fin/src/{client.rs (frame cap; call-site lines), conn.rs (a bounded `read_line`), main.rs (the overall deadline)}`, `crates/fin/src/backend.rs` and `crates/fin/Cargo.toml` (`test-hooks` feature; self dev-dependency), `scripts/check-test-hooks.mjs` (cover `fin`), `TASKS.md`.
- Test: `apps/finisherd/test/paths.test.ts`, `crates/fin/tests/cli_hardening.rs`, `scripts/check-test-hooks.test.mjs`.

**Interfaces:** `requireAbsolute(field: string, path: string, platform?): Effect.Effect<string, FinError>` (`INVALID_PATH {field, path}`); `FIN_DEADLINE_S`. Decisions 35–37. Decode failures as `FinError` are Task 3's (they need the protocol layer).

- [ ] **Step 1: Write failing tests:**
  - `relative_paths_are_invalid_path` (`project.create {dir: "x"}`, `project.open {dir: "./x"}`, `asset.probe {path: "a/../b"}`, `project.recover {importDir: "rel"}`; Windows `C:foo` and `\foo`);
  - `fin_frame_cap` (a fake server sending a 17 MiB line → `PROTOCOL_ERROR {max}`);
  - `fin_overall_deadline` (a fake server answering every 25 s; `FIN_DEADLINE_S=60`: the third call → `SERVER_TIMEOUT {deadline}`);
  - `test_hooks_ignored_without_feature` (a unit test on the cfg'd reader);
  - `check_test_hooks_flags_fin`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `fix(finisherd,fin): invalid paths, frame caps and deadlines; fin test hooks behind a feature`.

### Task 11: The native Windows pipe listener — tier: opus — after Task 3

**Files:**
- Create: `crates/fin-node/src/pipe_server.rs` (Windows only; a stub that errors elsewhere), `apps/finisherd/src/pipe.ts` (wraps it as an Effect `Socket` for the protocol).
- Modify: `crates/fin-node/src/lib.rs`, `crates/fin-node/Cargo.toml`, `packages/fin-node` (typings), `apps/finisherd/src/transport.ts` (Windows uses `PipeServer`; unix keeps `NodeSocketServer`), `apps/finisherd/src/connections.ts` (`peer`).
- Test: `crates/fin-node/tests/pipe_server.rs` (`#[cfg(windows)]`), `apps/finisherd/test/pipe.test.ts` (skipped off Windows).

**Interfaces:**
```rust
#[napi] pub struct PipeServer { /* accept thread, instances */ }
#[napi] impl PipeServer {
  #[napi(constructor)] pub fn new(name: String, opts: PipeOptions) -> napi::Result<Self>;    // PIPE_SQUATTED if not first
  #[napi] pub fn on_connection(&self, cb: ThreadsafeFunction<PipeConnectionInfo>) -> napi::Result<()>; // {id, pid, sid}
  #[napi] pub fn write(&self, id: u32, data: Buffer) -> AsyncTask<…>;
  #[napi] pub fn on_data(&self, id: u32, cb: ThreadsafeFunction<Option<Buffer>>) -> napi::Result<()>; // None = closed
  #[napi] pub fn close(&self, id: u32);
  #[napi] pub fn allow(&self, sid: String) -> napi::Result<()>;                               // sub-project 7: AppContainer SIDs
  #[napi] pub fn shutdown(&self);
}
```
Decision 38; spec §4.3, §4.11.

- [ ] **Step 1: Write failing tests (Windows):**
  - `first_instance_or_pipe_squatted` (a pipe of the same name created first → `PIPE_SQUATTED`);
  - `remote_clients_rejected` (the flag is set: checked through `GetNamedPipeInfo`/handle state);
  - `dacl_is_user_and_system_only` (read back with `GetSecurityInfo`);
  - `other_user_cannot_open` (a process under a restricted token or another account is closed before its first frame; skipped with a notice when the host has no second account);
  - `peer_pid_and_sid_reported`;
  - `allow_adds_an_appcontainer_sid` (a test AppContainer SID can connect after `allow`);
  - `round_trip_through_protocol` (the existing `server.test.ts` suite passes over the native pipe);
  - `shutdown_closes_all_connections`.
- [ ] **Step 2: Implement** with overlapped I/O on one accept thread and one I/O completion port; JS sees ordered `Buffer` chunks per connection.
- [ ] **Step 3: Run** the gates on Windows and Linux (stub) and `rtk pnpm e2e` on Windows → PASS. **Commit** `feat(fin-node,finisherd): native Windows pipe listener with its own security descriptor`.

### Task 12: e2e `auth` scenario — tier: sonnet — after Tasks 8, 9 and 11

**Files:**
- Create: `scripts/e2e/auth.mjs`.
- Modify: `scripts/e2e.mjs` (`SCENARIOS.auth`), `scripts/e2e.test.mjs`, `scripts/e2e/lib.mjs` (`consoleRpc(tag, payload)` proving as console and fetching a fresh `console.challenge` for each mutation; `rawConnect()`).

- [ ] **Step 1: Write the scenario:**
  1. Start `finisherd`. A raw connection that sends `snapshot.get` gets `UNAUTHENTICATED`; `server.hello` answers; `run/` holds only the socket.
  2. `fin project create`, three edits and an undo (`local`); `fin whoami` says `local`.
  3. `fin diagnostics purge --all`, `fin diagnostics consent diagnostics off` and `fin project takeover --confirm` exit 7 `HUMAN_ONLY`; no `fix` contains a command line; `finisherd diagnostics consent training on` exits 2; the console (spawned without a TTY) exits 7 `CONSOLE_NEEDS_TTY` for a purge; `consoleRpc` with a challenge purges; `finisherd audit --json` lists the denials.
  4. A ticketed WebSocket subscriber streams step 2's ops; reusing the ticket → 401; a ticket in the query string → 401.
  5. `finisherd agent mint` (through `consoleRpc`) with `--scope project.read` and `--branch own`: with `FIN_CREDENTIAL`, `fin whoami` says `agent`, `fin view` works, an edit → `SCOPE_DENIED`; a second credential with write scopes and `branch own` → an edit on `main` is `BRANCH_NOT_ALLOWED`, on its own branch works; its journal entries carry its credential id as `actor.session`; revoking it makes the next run exit 7.
  6. Restart: a copy of the old `local.key` no longer proves; `fin` works; a `fin` started just before the restart succeeds through the retry.
  7. **Leak grep** over `finisherd.json`, the audit export, `events/`, and every step's stdout and stderr: no key contents, ticket, MAC or challenge.
- [ ] **Step 2: Run** `rtk pnpm e2e` (all scenarios) → PASS. **Commit** `test(e2e): auth scenario for principals, the chain, tickets, the console and credentials`.

### Task 13: Spec, architecture pointer, README and tracker — tier: sonnet

**Files:** `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` (§3–4 as built: the Task 3 reading of Effect's RPC server, measured costs, deviations), `docs/specs/2026-11-01-finisher-architecture-design.md` (§27 item 15 "Sub-project 5A as built"; §27.12.5's WebSocket-token sentence superseded), `README.md` ("Security model": principals, key files, `run/`, the console and its confirmations, human-only operations, why they are advisory before sub-project 7, agent and MCP credentials, exit 7), `TASKS.md`.

- [ ] **Step 1:** Write the as-built notes from the code; list every deviation.
- [ ] **Step 2: TASKS:** tick the closed "Follow-ups for sub-project 5" (pipe squatting and DACL, server-data ACL, graceful stop, decode failures, `INVALID_PATH`, deadline and frame cap, `fin` test hooks); keep open: `childEnv` provider variables (7), `FIN_NODE_BINARY_DIR` (9), the sandbox rules of spec §4.11 (7), approvals behind `APPROVAL_REQUIRED` (7); add 5B–5E headings.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `docs: record sub-project 5A as built`.

### Task 14: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

On macOS, Windows 11 and the Linux VM; host details in `archive/hosts.md`.

- [ ] **Step 1:** `cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e` on each OS.
- [ ] **Step 2: `node:sqlite` under Electron's Node** on macOS and Windows (Linux x64 was checked on 2026-11-07 with Electron 44.9.3 and 45.0.0: Node 24.22.0, SQLite 3.53.4, FTS5, WAL, no warning): run `apps/finisherd/test/control.test.ts`'s store through `ELECTRON_RUN_AS_NODE=1 <electron> …` with the Electron version the spike used, and record the result.
- [ ] **Step 3: Windows, as a non-elevated user:** private server data; `PIPE_SQUATTED` when a second account creates the pipe first; that account cannot open the real pipe; a `fin` reading a stale `finisherd.json` that names a squatted pipe exits 7 after one retry, and the squatter sees nothing beyond the hellos; `finisherd stop`.
- [ ] **Step 4: Measure** and record (TASKS and spec): the handshake's added p50/p99 to `fin view` per OS (budget 1 ms); control-store statement p99 per OS (budget 1 ms); checkpoint-worker WAL size under the e2e load.
- [ ] **Step 5:** Fix what fails (one commit each, with a test); re-run Step 1.
