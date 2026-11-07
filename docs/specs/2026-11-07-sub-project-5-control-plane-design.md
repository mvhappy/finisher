# Sub-project 5: Control plane breadth — Design

- **Status:** draft for review (2026-11-07). Nothing here is built yet.
- **Parent:** `docs/specs/2026-11-01-finisher-architecture-design.md` (cited below as *arch §n*). This document refines arch §4 rule 6, §5 (`AuthProvider`, `ConnectRelay`), §6.4 ("conflicts become review items"), §6.7 (media libraries and indexing jobs), §8.4 (token delivery), §9.9 (jobs), §11.3 (per-transaction accept/reject), §16 (pairing, auth, security), §20 (library root mapping), §21.1 (sources and human-only operations) and §27.12.5 (the local transport and its WebSocket token). Where the two disagree, this document wins for sub-project 5, and arch §27 gets a pointer when each batch merges.
- **Builds on:** sub-projects 2 and 4 (4A, 4B, 4C, all on stacked branches; 4C is `sp4c-edit-breadth`) and the diagnostics hooks D1–D7 on the 4C branch. Sub-project 5 starts once D1–D7 are done, because it changes how D4 decides an event's source (§3.4).
- **House style:** as `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md`. Each batch gets its own plan; each batch section is rewritten "as built" when it lands.

---

## 1. Goal, scope and batches

### 1.1 Goal

Sub-project 5 turns `finisherd` from a single-user local server into the control plane that the agent runtime (7), plugins (8), the frontend (9) and the reviewer (10) consume:
- **every caller is a principal** with a credential and scopes, on every transport, and every RPC declares the scope it needs;
- **human-only operations** are enforced by the server, not by convention;
- devices **pair** with a server and reach it **remotely** over pinned TLS; a server is an **environment** with a stable id and ordered routes;
- long work runs as **jobs**: persisted, scheduled by resource, cancellable with a hard kill, resumable, observable;
- registered **media libraries** are catalogued, fingerprinted, probed and kept current on local disks, NAS shares and sync folders, through jobs;
- agent branches go through a **review workflow**: a change list per transaction with accept/reject, merge conflicts as review items, protected branches.

### 1.2 Non-goals

- **Frontend** (sub-project 9). §9 defines the server and RPC surfaces it needs; nothing here renders UI. The desktop shell's keychain storage of its device key, SSH/WSL launch and forwarding are also sub-project 9 (arch §16).
- **Agent sessions, modes, roles, the OS sandbox and command-pattern rules** (sub-project 7). Sub-project 5 defines the `agent` credential, its file format and its verifier (§4.2), so sub-project 7 only mints credentials and maps modes to scope sets.
- **Plugin credentials and permissions** (sub-project 8). The `plugin` principal kind is reserved.
- **Analysis stages** beyond probing: proxies (6), transcription, alignment, scenes, embeddings and semantic search (8). §7 builds the stage framework they plug into.
- **QA gate, critic, review markers, share page** (sub-project 10). §8 builds the review items they will produce and consume.
- **Hosted cloud:** accounts, OAuth/SSO, orgs, the Connect relay, hosted share links (`finisher-cloud`). Only the open extension points are defined.
- **LAN discovery (mDNS/Bonjour)**: pairing uses a link, QR code or typed code with an address.
- **Remote `fin`.** `fin` talks to a local socket only. Agents run next to the server (arch §6.7, "agents and jobs run next to the media").

### 1.3 Success, for every batch

- Every capability is driven and tested through `fin` → `finisherd`, or through the new `finisherd` console commands (§4.6) for human-only ones.
- `cargo test --workspace`, `pnpm test`, `pnpm gates` and `pnpm e2e` pass on macOS, Windows 11 and the Linux VM.
- No UI work. No host names, IPs, user names or key names in tracked files, fixtures included.

### 1.4 Batches and order

| Batch | Delivers | Depends on | Unblocks |
|---|---|---|---|
| **5A** Principals, local credentials, scopes | `control.db` and the audit log; principals; local and console credentials with a server proof (fixes pipe squatting and the readable WebSocket token); one-time WebSocket tickets; a scope on every RPC; human-only enforcement; `finisherd` console commands; event sources from credentials; protocol 2; the sub-project-5 follow-ups in `TASKS.md` | D1–D7 | everything below; sub-project 7 |
| **5C** Jobs | `fin-jobs` protocol types, the `fin-workers` binary, a persisted scheduler with resource pools, hard kill of process trees, retries with bisection, progress streams, `fin job …`, `fin wait`, `job.done` events | 5A | 5D; sub-project 6 (proxies), 8 (analysis, generation) |
| **5D** Library indexer | libraries per server, catalog databases, scans for local/network/sync roots, placeholder safety, fingerprint and probe stages through jobs, the stage framework, offline libraries, metadata search, `fin library …`, `library.scan` events | 5C | sub-projects 6, 8, 11 (clip finder) |
| **5E** Review workflow | review items, change lists from merge dry runs, live refresh, per-transaction decisions, `history.merge` exclusions, conflicts as review items, protected branches, `fin review …`, `review.verdict` events | 5A | sub-projects 7 (draft mode), 10 |
| **5B** Pairing, devices, remote access, environments | device keys, pairing offers with owner approval, DPoP-bound tokens with server nonces, WebSocket tickets for devices, TLS listeners with SPKI pinning, key rotation, environment descriptors and routes, revocation, capability URLs, server filesystem reach | 5A | sub-project 9 |

**Order:** 5A → 5C → 5D → 5E → 5B. Sub-project 6, the next in arch §24, needs jobs most. 5B is needed only by the frontend, so it comes last, but it touches only transport and auth files: it may run in parallel with 5C–5E on its own stacked branch. (5B keeps its letter because it completes 5A's auth work, although it is built last.)

### 1.5 What exists today (the baseline)

- **Transports** (`apps/finisherd/src/transport.ts`): Effect RPC over NDJSON on the Unix socket `<server-data>/fin.sock` (0600) or the named pipe `\\.\pipe\finisher-<username>[-<hash>]`, and a WebSocket on `127.0.0.1:<random>` that requires `Authorization: Bearer <token>`. The token is random per start and written to `<server-data>/finisherd.json` (0600) with the pid, port, socket and protocol version.
- **Trust:** anything that can open the socket is fully trusted. Anything that can read `finisherd.json` can use the WebSocket. D4 maps the transport to an event source (local socket → `fin`, WebSocket → `ui`), and only `ui` may opt in to training content. The D4 plan lists the readable token as a Review Focus item.
- **Server data** (`config.ts` `ensureServerData`): created 0700, refused if a symlink or owned by another uid; nothing is checked on Windows.
- **Identity:** `<server-data>/server-id`, a lowercase ULID with clone detection (`fin-store::server_id`).
- **Settings:** `<server-data>/settings.json`, re-read while running; the D3/D4 merging writer changes only the keys it owns.
- **`fin`** (`crates/fin`): finds the socket through `FIN_SERVER` or `finisherd.json`, sends `headers: []` today (D6 adds actor headers), never calls `server.hello`, and exits 0/2/3/4/5/6.
- **`TASKS.md` "Follow-ups for sub-project 5":** Windows named-pipe DACL and pipe squatting, server-data ACL check, graceful stop without SIGTERM; decode failures and unknown tags as `FinError`; relative paths refused server-side; a client deadline and frame-size cap; test hooks out of release builds; `childEnv` provider variables (moved to sub-project 7).

---

## 2. Principles

1. **Trust comes from a credential, never from a transport.** Loopback is not local (an SSH tunnel ends on loopback; so does a browser tab after DNS rebinding). A Unix socket is not the user (an agent in a sandbox can still `connect()` to it: Landlock at the ABI spike S8 used (4) does not mediate `connect` on pathname sockets, and Windows' default named-pipe DACL lets every user open the pipe for reading). Every connection proves a credential; with none, it may only say hello.
2. **The client authenticates the server too.** A squatted pipe or a stale `finisherd.json` must not receive a request. Every handshake carries a server proof bound to the client's nonce.
3. **Human-only is a property of the principal, enforced in one place.** The server checks it before any handler runs. A request from `fin` (which is agent-only, D16) can never be human, whatever its headers claim.
4. **Least privilege by default, the status quo by explicit grant.** The `local` principal (anything that can read `<server-data>/auth/local.key`) keeps what `fin` can do today, minus the human-only operations. Sub-project 7's sandbox removes agents' read access to server data and gives them narrower `agent` credentials.
5. **No secret on disk where an agent or a sync service can read it.** Keys live in server data (0700, local disk) or the OS keychain; never in a project folder, never in the environment (arch §8.4), never in the journal.
6. **Control-plane state is local, versioned and rebuildable where it can be.** It lives in SQLite on the server's local disk (arch §6.7 rule), carries `formatVersion`/`minReaderVersion` like every persisted format (sub-project 4 §2.6), and anything that can be re-derived (library catalogs, job queues, change lists) is re-derived rather than migrated with heroics.
7. **The diagnostics journal stays metadata-first.** New kinds carry only `meta` fields from closed tables. The security audit log is a separate store with its own rules (§3.3).
8. **Every error is `{code, message, details, fix?}`** with a stable exit code in `fin`.

---

## 3. Shared foundations

### 3.1 The control store (`control.db`)

- `<server-data>/control.db`, SQLite through Node's built-in **`node:sqlite`** (`DatabaseSync`), WAL mode, `synchronous = NORMAL`, `foreign_keys = ON`, mode 0600. No new dependency and no native prebuild matrix (Node 26.12 ships it unflagged with FTS5; 5A Task 1 confirms both in the Electron runtime of sub-project 9's pinned version as soon as it exists).
- **One writer:** `finisherd`'s main thread. Library catalogs are separate databases with their own writer (§7.3).
- **Event loop:** `node:sqlite` is synchronous. Every statement is prepared once, indexed and small; the control-store cost test bounds p99 at **1 ms per statement** on the Linux VM with 10k jobs and 100k audit rows. Bulk work (catalog diffs, imports) never runs on the main thread.
- **Versioning:** a `meta` table holds `formatVersion`, `minReaderVersion`, `serverId` and `createdAt`. Migrations are frozen modules `m0001…` applied in one transaction at start, after a `VACUUM INTO control.db.pre-m<n>.bak` (the newest two are kept). A database whose `minReaderVersion` is above this code fails the start with `CONTROL_DB_TOO_NEW` (exit 1, stderr): there is no read-only control plane.
- **Damage:** `PRAGMA quick_check` at open. A failed check moves the file aside to `control.db.corrupt-<ulid>`, starts empty and writes an audit entry. Devices then re-pair (fail closed), library catalogs rescan, jobs are lost, review items are re-derived where possible (§8.1).
- **`serverId` mismatch** (server data copied to another machine and the id reminted by `fin-store::server_id`): §5.9.

Tables per batch: 5A `meta`, `audit`, `agent_sessions`; 5B `devices`, `pair_offers`, `pair_pending`, `rotations`; 5C `jobs`; 5D `libraries`; 5E `reviews`, `review_events`, `review_comments`, `branch_policy`.

### 3.2 Principals

```text
Principal {
  kind:     anonymous | local | console | device | agent | plugin
  id:       "local" | "console" | <deviceId ULID> | <agentSessionId ULID> | <pluginId>
  role:     owner | editor | viewer            (device only; console counts as owner)
  scopes:   Scope[]                            (§4.4: the kind's defaults, narrowed by role or grant)
  projects: "*" | ProjectId[]                  (device and agent; "*" otherwise)
  human:    console, or device                 (derived)
  owner:    console, or device with role owner (derived)
  source:   Source                             (§3.4: derived, never sent)
  author:   string                             (the default Tx.author for its writes)
}
```

| Kind | Who | Proof | Batch |
|---|---|---|---|
| `anonymous` | any connection before a proof | none | 5A |
| `local` | `fin` and scripts run by the user; until sub-project 7, also agent harnesses the user starts outside Finisher | HMAC with `<server-data>/auth/local.key` (§4.2) | 5A |
| `console` | a person at the server: `finisherd <command>` (§4.6), and the desktop shell's main process | HMAC with `<server-data>/auth/console.key` | 5A |
| `device` | a paired client: the desktop renderer, a browser, later a phone | DPoP-bound token → WebSocket ticket (§5) | 5B |
| `agent` | one agent session (sub-project 7) | HMAC with the session key in the session's credential file (§4.2) | 5A defines, 7 mints |
| `plugin` | one plugin process (sub-project 8) | reserved | 8 |

- **Single owner** (arch §5 `AuthProvider` local default): the server has one owner, the person at the console. Paired devices carry a role. `editor` and `viewer` devices exist so that the owner's own phone can be a viewer and so that an informal collaborator can be paired without owner powers; accounts, orgs and real multi-user roles are `finisher-cloud`.
- The principal is attached to the **connection**, not to each request (§4.3), and is re-checked on every request (revocation is immediate).

### 3.3 The audit log

- **What:** a security record for the owner (arch §16 "audit log"). Rows in `control.db` `audit {id ULID, at, kind, principalKind, principalId, outcome, code?, remote?, details JSON}`.
- **Kinds:** `auth.fail` (bad proof, bad ticket, bad DPoP proof, rate-limited), `auth.denied` (a human-only or scope denial: RPC, scope, principal), `auth.ticket` (WebSocket tickets: issued and used, not every request), `pair.offer|redeem|approve|reject|expire`, `device.revoke|rename|role`, `agent.mint|revoke` (7), `consent.change`, `diagnostics.purge`, `settings.write`, `project.takeoverConfirmed`, `project.relocated`, `listener.start|stop` (with the SPKI pin), `tls.rotate`, `env.moved|adopted|reset`, `library.add|remove|set`, `review.merge` (on protected branches); reserved for 7/8: `grant`, `hook.block`, `hook.modify`, `sandbox.violation`, `plugin.install`.
- **Always on.** It is not diagnostics: consent does not gate it, `DO_NOT_TRACK` does not stop it, it is never uploaded, never part of `fin diagnostics export` and never read by the dataset builder.
- **What it may hold** that the journal may not: device names and the remote address of failed and pairing requests (Owner question 4). Never secrets, tokens, MACs, pairing codes, paths or request payloads.
- **Retention:** 180 days or 100k rows, oldest first, swept hourly. Readable and purgeable only by the owner (`auth.audit.list`, `auth.audit.purge`; `finisherd audit`).
- **Never blocks:** an audit write failure is logged to stderr and counted in `finisherd status`; the request it describes still gets its answer (denials still deny).

### 3.4 Diagnostics journal integration

- **Source from the principal** (replaces D4's per-transport `RpcSource`): `local` → `fin`, `agent` → `fin` (the emitting program is still `fin`), `device` → `ui`, `console` → **`console`** (a new `fin_core::events::Source` variant, 5A), `anonymous` → nothing is accepted. A WebSocket opened with a `local` ticket (the e2e subscriber) is `fin`, not `ui`.
- **Actor from the principal, narrowed by headers.** D6's `fin-actor-*` headers are kept, but the principal bounds them: `agent` principals always get `{kind: agent, session: <their session id>}`, whatever the headers say, and only `call` comes from the header; `device` principals get `{kind: user, session: <deviceId>}`; `console` gets `{kind: user}`; `local` keeps D6's unverified header actor (as today).
- **Consent and the other human-only diagnostics operations** move from "source is `ui`" to "principal is owner" (§4.5). This lands the "human-only once permissions exist" TASKS follow-up of the D-plan in 5A instead of sub-project 7.
- **The D4 rate limit** becomes per principal (`local`, each device, each agent session) instead of per source.
- **New kinds** (all `meta` only, all `source: finisherd`, names from new closed tables in `fin_core::events::names`):

| Kind | Batch | Payload |
|---|---|---|
| `auth.pair` | 5B | `outcome: offered\|redeemed\|approved\|rejected\|expired\|revoked`, `role`, `platform: Name(PLATFORMS)`, `route: loopback\|lan\|tailscale\|public\|connect\|other`, `micros?` (redeem → approve) |
| `job.done` | 5C | `kind: Name(JOB_KINDS)`, `outcome: succeeded\|failed\|cancelled\|interrupted`, `code?`, `attempts`, `priority`, `queuedMs`, `runMicros`, `items?`, `itemsFailed?`, `bisected` |
| `library.scan` | 5D | `storage: local\|network\|sync`, `full`, `outcome`, `code?`, `files` and `dirs` (log₂ buckets), `added`, `changed`, `missing`, `moved`, `placeholders`, `micros` |
| `review.verdict` | 5E | fills the reserved v0 kind as v1: `itemKind`, `verdict: accepted\|rejected\|merged\|dismissed\|changesRequested`, `txIds: IdRef[]` (≤ 64), `accepted`, `rejected`, `by: user\|agent\|critic`, `openS`; training only: `reason: Content<userText>` |

- **Denials** are not a kind: `rpc.stats` already counts error codes per method, so `SCOPE_DENIED`/`HUMAN_ONLY` show up there.
- **Never in the journal:** device names, environment names, routes, addresses, library names and paths, file names (they are `fileName` content and nothing in sub-project 5 records them), review titles and comments outside `Content`.

### 3.5 Exit code 7: denied

`fin` gains one exit class (`crates/fin/src/output.rs`): **7 = denied**, "this needs a different principal, usually a person". It covers `UNAUTHENTICATED`, `SERVER_UNTRUSTED`, `SCOPE_DENIED`, `HUMAN_ONLY`, `BRANCH_PROTECTED`, `PATH_NOT_ALLOWED`, `CREDENTIAL_REVOKED`. An agent must not retry these; the `fix` names who can act and how (for example `ask the user to run: finisherd diagnostics purge --all`).

### 3.6 Protocol 2

5A bumps `PROTOCOL_VERSION` to **2**: the local handshake (§4.2) and WebSocket tickets (§4.3) are not compatible with protocol-1 clients, and every client lives in this repository and ships version-locked (arch §21 "Updates"). Protocol 2 also takes the two changes sub-project 4 left for it: `tx.commit.generation` and `ops.subscribe.generation` become required, and the `project.recover` result change of 4B is recorded. The WebSocket upgrade refuses another version with **HTTP 426** and a JSON body `{protocol: {min, max}}` (arch §16 "Handshake").

---

## 4. 5A — Principals, local credentials and scopes

### 4.1 Data model

- `Principal` (§3.2), `Scope` (§4.4), `Connection {clientId, transport: socket|ws, principal, serverNonce, openedAt}`.
- Key files in `<server-data>/auth/` (0700): `local.key`, `console.key` (32 random bytes, base64url, 0600), **rewritten at every start** before the socket listens. A process that read a key keeps nothing after a restart.
- `agent_sessions {id ULID, label, scopes JSON, projects JSON, branches JSON?, createdBy, createdAt, expiresAt?, revokedAt?}` (5A creates the table and the verifier; sub-project 7 the minting). An agent's key is never stored: it is `HMAC(agentMaster, "fin/agent-key/v1" ‖ id)` with `agentMaster` in `<server-data>/auth/agent.master` (0600, kept across restarts, so sessions survive one). Revocation is the row's `revokedAt`.
- **The agent credential file** (written by sub-project 7 inside the session sandbox, 0600): `{"v":1,"kind":"agent","id":"<ulid>","key":"<base64url>","env":"<envId>"}`. `fin` finds it through **`FIN_CREDENTIAL`**, a path. The variable names a file, not a secret, so it may be in the agent environment (arch §8.4 forbids only the token itself).

### 4.2 The local handshake

```text
fin                                                    finisherd
 │ server.hello {clientNonce, credential: {kind: local|console} | {kind: agent, id}}
 │ ─────────────────────────────────────────────────────▶
 │   {protocolVersion, serverVersion, capabilities, envId, serverNonce,
 │    proof = HMAC(K, "finisherd/hello/v1" ‖ clientNonce ‖ serverNonce)}
 │ ◀─────────────────────────────────────────────────────
 │ verify proof (constant time) — else SERVER_UNTRUSTED, send nothing more
 │ auth.prove {mac = HMAC(K, "fin/prove/v1" ‖ serverNonce ‖ clientNonce)}
 │ ─────────────────────────────────────────────────────▶
 │   {principal: {kind, id, scopes, human, owner}}      (connection now carries the principal)
 │ ◀─────────────────────────────────────────────────────
```

- `K` is the key of the credential named in the hello. Nonces are 32 random bytes; HMAC-SHA-256 (`node:crypto` on the server, the `hmac`/`sha2` crates in `fin`); every label is domain-separated.
- **Why a server proof:** on Windows a pipe name can be squatted by another user before `finisherd` starts, and a stale `finisherd.json` can name it. Only a process that can read the key file can produce the proof, and only the user can read server data (0700 on unix; an owner-only DACL on Windows, checked at start, §4.11). The proof closes squatting without native pipe-security code in Node (Node exposes neither the pipe handle nor `GetNamedPipeClientProcessId`, nor `SO_PEERCRED` on sockets).
- **Unknown or revoked agent id, wrong MAC, no hello:** `UNAUTHENTICATED` (exit 7), audited as `auth.fail`. The connection may retry the hello; after 5 failures it is closed.
- **Before the proof** a connection may call only `server.hello` and `auth.prove`.
- **Cost:** one round trip and four HMACs per `fin` run; the budget is ≤ 1 ms added to `fin`'s p50 on the Linux VM.
- **`fin` without a readable key** (a sandboxed agent before it has a credential file, a test with only `FIN_SERVER`) still connects and gets `UNAUTHENTICATED` for anything but the hello.

### 4.3 Connections, the WebSocket and tickets

- **Connection-aware RPC protocol.** `finisherd` replaces `RpcServer.layerProtocolSocketServer` with its own `RpcServer.Protocol` (`protocol.ts`), modelled on Effect's socket protocol: it owns framing for both transports, gives each connection a `clientId`, keeps its `Connection`, and enforces the frame-size cap (§4.10). Handlers find the principal by `clientId`. Client-sent headers can never set the principal: the protocol strips any header with the reserved `x-fin-` prefix.
- **Authorisation wraps every handler** in one place (`authorize.ts`, where `guard` wraps them today): it looks up the RPC's scope (§4.4), evaluates payload escalations, checks the principal, audits a denial and only then runs the handler.
- **WebSocket tickets** replace the bearer token. `auth.ticket {ttlS ≤ 60}` (any authenticated principal) returns a 256-bit one-time ticket bound to the caller's principal. `ws://127.0.0.1:<port>/rpc?v=2&ticket=<t>` upgrades once; the WebSocket connection then carries that principal. Devices get tickets through DPoP in 5B; 5A's minters are `local` and `console` (the e2e subscriber, a future bare web client started from a script).
- **`finisherd.json` no longer holds a token.** It keeps `pid`, `wsPort`, `socket`, `protocolVersion` and adds `envId` and `diagnostics.enabled` (D4).
- **Origin and Host on the loopback WebSocket:** a request without `Origin` (not a browser) is accepted. A browser `Origin` must be one of the server's own origins (`http://127.0.0.1:<port>`, `http://localhost:<port>`, `http://[::1]:<port>`), the desktop shell's origin (sub-project 9) or a configured one; `Host` must be one of the server's own names. Otherwise HTTP 403 before the upgrade (DNS-rebinding protection).

### 4.4 Scopes

A closed vocabulary in `@finisher/contracts` (`scopes.ts`). **Every RPC declares exactly one scope**, plus optional payload escalations; a test fails when an RPC has none.

| Scope | Covers | Human-only |
|---|---|---|
| `public` | `server.hello`, `auth.prove` | — |
| `server.read` | `auth.ticket`, `env.describe`, `fonts.catalog`, `fonts.status`, `server.status` | — |
| `project.read` | `snapshot.get`, `ops.subscribe`, `project.events`, `project.status`, `project.verify`, `tx.get`, `branch.list`, `eval.*`, `fonts.plan`; later `review.list/get/watch` (5E) and `capability.url` (5B) | — |
| `project.write` | `project.create`, `project.open`, `asset.probe`, `tx.commit`, `branch.create`, `branch.set`, `history.*`, `fonts.fetch`, `project.acknowledge*` | — |
| `project.admin` | `project.rebuild`, `project.compact`, `project.recover`, `project.takeover` | — |
| `project.confirm` | escalation: `project.takeover {confirm: true}`, `project.open {relocate: true}` | **human** |
| `branch.protected` | escalation: any write whose target branch is protected (§8.5) | — |
| `review.write` | `review.request`, `review.comment`, `review.refresh` | — |
| `review.decide` | `review.decide`, `review.merge`, `review.close` | — |
| `jobs.read`, `jobs.submit`, `jobs.control` | §6.6 | — |
| `library.read`, `library.scan` | §7.6 | — |
| `library.admin` | `library.add`, `library.remove`, `library.set` | **owner** |
| `diagnostics.read` | `diagnostics.status`, `diagnostics.export` (content redacted, below), `diagnostics.project {collect: never}`, `diagnostics.record` | — |
| `diagnostics.admin` | `diagnostics.consent`, `diagnostics.purge`, `diagnostics.project {collect: default}`, escalation: `diagnostics.export` of content | **owner** |
| `settings.admin` | `settings.write` (5B: remote listeners, origins, project roots) | **owner** |
| `auth.admin` | pairing, devices, agent sessions, `auth.audit.*`, `server.stop`, `tls.*`, `env.adopt/reset` | **owner** |
| `plugin.install` | reserved (sub-project 8, arch §8.3) | **owner** by default |

**Default scopes per principal:**

| Principal | Scopes |
|---|---|
| `anonymous` | `public` |
| `console` | all |
| `device` owner | all |
| `device` editor | all but the human-only rows and `auth.admin`; `project.confirm` yes (a person) |
| `device` viewer | `public`, `server.read`, `project.read`, `review.write`, `jobs.read`, `library.read`, `diagnostics.read` |
| `local` | all but the human-only rows. It keeps `branch.protected` and `review.decide`, so `fin merge` into `main` keeps working (Owner question 5) |
| `agent` | as minted by sub-project 7, never a human-only scope; by default no `branch.protected`, no `review.decide`, no `jobs.control`, no `project.admin` |

- **Payload escalations** are declared beside the scope (`escalate(payload) → Scope | null`) and checked by the same wrapper, so a handler never re-implements a check.
- **`diagnostics.export` for non-owners is redacted, not refused:** content fields and blob references are removed, as the upload view does for diagnostics-only, and the result says `redacted: true`. An agent can still read what the server records about it, without the user's content.
- **Project reach:** a principal with `projects: [ids]` gets `PROJECT_NOT_FOUND` (not `SCOPE_DENIED`, so ids don't leak) for any other project, on every project-scoped RPC and stream.

### 4.5 Human-only operations

These are refused with **`HUMAN_ONLY`** (exit 7) for every principal that is not human (owner where the table says owner), before the handler runs. The `fix` names the console command.

| Operation | Why | Who | Console |
|---|---|---|---|
| any consent change (`diagnostics.consent`, both purposes, any value) | arch §21.1: a person's choice | owner | `finisherd diagnostics consent …` |
| export of recorded content | it is the user's content | owner | `finisherd diagnostics export --content` |
| `diagnostics.purge` (every selector) | destroys data | owner | `finisherd diagnostics purge …` |
| `diagnostics.project {collect: default}` (setting `never` stays open) | clearing a never-collect flag binds collaborators | owner | `finisherd diagnostics project default <id>` |
| `project.takeover {confirm: true}` | arch §6.7: "the user confirms" | human | `finisherd project takeover <dir> --confirm` |
| `project.open {relocate: true}` | "a person's decision; never set it automatically" (4A) | human | `finisherd project open <dir> --relocate` |
| `library.add`, `library.remove`, `library.set` of hydration, bandwidth or cloud embeddings | grants read access to a directory tree; downloads; money and uploads (D29) | owner | `finisherd library …` (5D) |
| pairing offers and approvals, device revoke/rename/role, agent session mint/revoke outside sub-project 7's runtime, audit read and purge, `server.stop`, TLS and environment commands | account takeover vectors | owner | `finisherd pair`, `devices`, `revoke`, `audit`, `stop`, `tls`, `env` |
| settings writes through RPC (remote listeners, origins, project roots) | opens the server to the network | owner | `finisherd remote …` (5B) |
| plugin install (sub-project 8 default) | arch §8.3 | owner | (8) |

- **Before sub-project 7's sandbox this protects against agents that use `fin`, not against arbitrary code running as the user:** an unsandboxed process can read `console.key` exactly as it can hand-edit `settings.json`. The mechanism is complete in 5A; sub-project 7 makes it hold by denying agents read access to server data (a TASKS follow-up that already exists).
- **`fin`'s own D-plan commands** follow: `fin diagnostics consent`, `purge` and `project default` reach the server and come back `HUMAN_ONLY`; `fin diagnostics consent training on` stays a local usage error (exit 2, D14b).

### 4.6 The console (`finisherd <command>`)

- `finisherd` alone (or `finisherd serve`) serves, as today. Any other first word is a console command that connects to the running server over the local socket with `console.key` and prints text for a person (`--json` for scripts). English, like `fin` (arch §17.1 makes CLIs English; codes are stable).
- 5A commands: `status` (version, listeners, connected principals by kind, journal state, audit failures), `stop` (`server.stop`: the graceful stop Windows lacks without SIGTERM), `diagnostics consent|purge|project|export`, `project takeover <dir|id> --confirm`, `project open <dir> --relocate`, `audit [--since] [--kind] [--limit]`, `agent mint|ls|revoke` (for tests and hand-run agents until sub-project 7).
- 5B adds `pair`, `devices`, `revoke`, `remote enable|disable|status`, `tls show|rotate`, `env show|name|adopt|reset`; 5D adds `library add|rm|set`.
- **Errors** print `finisherd: <CODE>: <message>` plus the `fix`, and exit with `fin`'s exit codes.

### 4.7 RPCs and `fin` (5A)

| RPC | Scope | Payload → result |
|---|---|---|
| `server.hello` (changed) | `public` | `{clientNonce?, credential?}` → adds `envId`, `serverNonce`, `proof?` |
| `auth.prove` | `public` | `{mac}` → `{principal}` |
| `auth.ticket` | `server.read` | `{ttlS?}` → `{ticket, url, expiresAt}` |
| `auth.whoami` | `server.read` | `{}` → `{principal}` |
| `auth.agent.mint` | `auth.admin` | `{label, scopes, projects?, branches?, expiresAt?}` → `{session, credential}` (the file contents; shown once) |
| `auth.agent.list` / `auth.agent.revoke` | `auth.admin` | — |
| `auth.audit.list` / `auth.audit.purge` | `auth.admin` | `{since?, kind?, cursor?, limit ≤ 1000}` → `{entries, next?}` |
| `server.status` | `server.read` | `{}` → `{version, protocol, envId, listeners, connections: {kind: count}, journal, auditFailures}` |
| `server.stop` | `auth.admin` | `{}` → `{}` then a graceful shutdown |

`fin` changes: the handshake in a new `crates/fin/src/auth.rs` (key discovery: `FIN_CREDENTIAL`, else `<server-data>/auth/local.key`); exit class 7; `fin whoami` → `{principal}`.

### 4.8 Storage

`<server-data>/auth/{local.key, console.key, agent.master}`; `control.db` tables `meta`, `audit`, `agent_sessions`. Nothing in project folders.

### 4.9 Security and threat model (5A)

Arch §16 priority: injected agent > malicious plugin > stolen device > network attacker.

| Threat | Mitigation | Residual |
|---|---|---|
| Injected agent runs `fin diagnostics consent training on`, `purge --all`, `takeover --confirm` | `HUMAN_ONLY` for every non-human principal, checked before the handler | before sub-project 7, the agent can read `console.key` with a shell (documented, §4.5) |
| Injected agent reads `finisherd.json` and connects to the WebSocket as `ui` | no token in the file; WebSocket needs a ticket; a `local` ticket gives `local`, never `ui` | — |
| Another OS user squats the pipe name, or opens the pipe for reading | server proof (client refuses with `SERVER_UNTRUSTED`); the squatter cannot authenticate to the real server either; Windows server data has an owner-only DACL | a squatter can deny service (fin reports it) |
| A sandboxed agent `connect()`s to the socket directly | no proof without a readable key → `UNAUTHENTICATED`; its own credential gives only its scopes | — |
| Replay of a captured handshake | fresh nonces on both sides; one-time tickets | — |
| DNS rebinding of a browser tab to the loopback WebSocket | Host and Origin checks; a ticket is required anyway | — |
| A client claims another actor in headers | the principal bounds the actor (§3.4); `x-fin-` headers stripped | `local` can still claim `agent` or `user` (unverifiable, as in D6) |
| Audit flood from a misbehaving client | 5 failed proofs close a connection; `auth.fail` aggregated per minute per remote/connection | — |

### 4.10 Error codes (5A)

| Exit | Codes |
|---|---|
| 2 | `FRAME_TOO_LARGE` (a frame over 16 MiB on either transport; `details.max`) |
| 3 | `INVALID_REQUEST` (an RPC frame that does not decode, an unknown tag, a payload that fails its schema: replaces today's `Die` defect), `INVALID_PATH` (a relative or non-normalised path, server-side) |
| 5 | `CONTROL_DB_BUSY` (a statement waited past the busy timeout) |
| 7 | `UNAUTHENTICATED`, `SERVER_UNTRUSTED`, `SCOPE_DENIED {rpc, scope, principal: {kind}}`, `HUMAN_ONLY {rpc, scope, console}`, `CREDENTIAL_REVOKED` |
| start (stderr, exit 1) | `CONTROL_DB_TOO_NEW`, `SERVER_DATA_UNSAFE` (now also on Windows) |

### 4.11 Cross-platform (5A)

- **Windows server data:** `ensureServerData` calls a new `fin-node` `checkPrivateDir(path)` (Rust, `windows-sys` `GetNamedSecurityInfoW`): the owner is the current user's SID, the DACL grants nothing to `Everyone`, `Users`, `Authenticated Users` or another user, inherited or not (SYSTEM and Administrators may stay). A directory created by `finisherd` gets a protected owner-only DACL; an existing one that fails is `SERVER_DATA_UNSAFE` with a `fix` (a later sub-project-7 step adds per-project AppContainer SIDs for the pipe, not for server data).
- **Key files** are written temp-then-rename; on Windows the temp file is created inside the already private directory, so it inherits the DACL; rename-over retries sharing violations (`fsio::is_transient`-style).
- **Graceful stop:** `finisherd stop` replaces SIGTERM on Windows; it also works elsewhere.
- **Unix:** server data 0700 and key files 0600, as today; the socket stays 0600.
- **Clocks:** nothing in 5A compares clocks; ticket TTLs are measured on the server's monotonic clock.

### 4.12 Journal (5A)

`console` source; principal-derived source and actor; owner-only diagnostics operations; per-principal rate limit; new `RPC_METHODS` entries for every new RPC (the D4 drift test).

### 4.13 Testing (5A)

- **Unit (vitest):** handshake vectors (fixed keys and nonces → exact MACs, shared with a Rust test so both sides agree); scope table covers every RPC (`rpc_scopes_cover_every_rpc`); every human-only scope is refused for `local` and `agent` and allowed for `console` (`human_only_matrix`); escalations; `x-fin-` header stripping; ticket one-time use and TTL; Origin/Host matrix; control-store migrations, `CONTROL_DB_TOO_NEW`, corruption move-aside; audit retention; the 1 ms statement budget.
- **Rust (`fin`):** `fin` refuses a server without a proof (`SERVER_UNTRUSTED`, a fake server in the test that holds no key); `FIN_CREDENTIAL` discovery; exit 7 mapping; `fin whoami`.
- **e2e `auth` scenario:** an unauthenticated connection can only say hello; `fin` works as `local`; `fin diagnostics purge --all` → exit 7 naming the console; `finisherd diagnostics purge --all` works; a ticketed WebSocket subscriber streams ops; a reused ticket is refused; a restart invalidates the old keys; a minted agent credential with `project.read` only cannot commit (`SCOPE_DENIED`) and its journal entries carry its session as actor.
- **Windows:** a second user (or a test that creates the pipe first from an unprivileged token) cannot make `fin` send a request; `SERVER_DATA_UNSAFE` on a directory with an `Everyone` ACE; `finisherd stop`.

---

## 5. 5B — Pairing, devices, remote access and environments

### 5.1 Data model

- `devices {id ULID, name (user text), platform (Name: macos|windows|linux|ios|android|browser|other), role, jwk (public, P-256), jkt (RFC 7638 thumbprint), projects JSON, createdAt, approvedBy, lastSeenAt, lastRoute, revokedAt?, revokedBy?}`.
- `pair_offers {id ULID, secretHash (SHA-256), codeHash?, role, label?, createdBy, createdAt, expiresAt (≤ 10 min), attempts, state: open|redeemed|expired|burned}`.
- `pair_pending {id ULID, offerId, deviceName, platform, jwk, jkt, safetyCode, redeemedAt, remote, expiresAt, state: pending|approved|rejected|expired}`.
- **Environment descriptor** `{envId (= server-id), name (owner-set text), spki (base64url SHA-256 of the identity key's SubjectPublicKeyInfo), protocol {min, max}, routes: [{kind: loopback|lan|tailscale|public|connect, url}], capabilities}`.
- **Identity key:** one P-256 key per environment. It is the TLS key, so clients pin the key (SPKI), not a certificate.
- Tokens and tickets are **opaque random strings held in memory only**, mapped to `{deviceId, jkt, expiresAt}`. A restart forgets them; devices mint new ones with their key. No JWT is issued, so there is nothing to verify offline and nothing to forge.

### 5.2 Pairing

1. **Offer.** An owner (console, or an owner device) calls `auth.pair.offer {role, label?, ttlS ≤ 600}` → `{offerId, link, qr, code, expiresAt}`.
   - **Link:** `finisher://pair?v=1&env=<envId>&spki=<pin>&r=<base64url(routes)>#<offerId>.<secret>`; the secret (256 bits) is in the fragment (arch §16). For browsers, `https://<route>/pair#<offerId>.<secret>` on a route with a trusted certificate (§5.5).
   - **Code** for typing: 12 Crockford base32 characters, `XXXX-XXXX-XXXX` (an offer index and a 40-bit secret), typed together with an address. An offer whose code fails 5 times is burned; redeem attempts are limited to 10 per minute per remote address and 30 per minute per server.
   - The console's `finisherd pair` prints the link, a QR code (in the terminal) and the code, then waits for a redeem (below).
   - The UI warns that the link is a bearer credential (arch §16); `fin` can never create one (`auth.admin` is owner-only).
2. **Redeem.** The new device makes a P-256 key pair (non-extractable where the platform allows: WebCrypto in a browser, the keychain on the desktop, sub-project 9), then `POST /pair/redeem {offerId, secret | code, device: {name, platform, jwk}}` over the pinned route. The offer is consumed at the first valid redeem, even if the owner later rejects it. The answer is `{pendingId, safetyCode}`; the safety code is 6 digits, `HMAC(secret, jkt) mod 10⁶`.
3. **Approve.** Owner principals receive `auth.pair.pending` items (a stream); the console's `finisherd pair` shows them in place. The owner compares the device name and the safety code with the device's screen and approves (`auth.pair.approve {pendingId, role ≤ offer role, name?}`) or rejects. A pending request expires 10 minutes after the redeem.
   - **One exception:** the desktop shell pairs its own renderer with a console-made offer flagged `self`, redeemed over loopback only, approved at redeem. The shell is the console (arch §16 lets the owner approve at the server console).
4. **Finish.** The device polls `POST /pair/status` with a DPoP proof (below) made with the key it redeemed with, and gets `{deviceId, role, env}` once approved.

### 5.3 Tokens, DPoP and WebSocket tickets

- `POST /auth/token` with a DPoP proof (RFC 9449 JWS: `typ dpop+jwt`, `alg ES256`, `jwk`; claims `jti`, `htm`, `htu`, `iat`, `nonce`) signed by a known, non-revoked device key → `{accessToken, tokenType: "DPoP", expiresIn: 900}`.
- `POST /auth/ws-ticket` with `Authorization: DPoP <token>` and a proof carrying `ath` → `{ticket, expiresIn: 30}`; then the WebSocket upgrade of §4.3 (`/rpc?v=2&ticket=`), now over TLS for remote routes. The WebSocket keeps its principal until it closes or the device is revoked.
- **Freshness comes from server nonces, not client clocks.** Every proof must carry a current `DPoP-Nonce` (rotated every 5 minutes; the previous one stays valid). `iat` is only sanity-checked (±7 days), because the 4A spike shows client clocks off by many seconds for days. `jti` is remembered for the nonce lifetime. A missing or old nonce answers `use_dpop_nonce` with a new one (RFC 9449 §8).
- **Capability URLs** for binary channels that a browser loads without headers (`<video src>`, sub-projects 3 and 6): `capability.url {hash, ttlS ≤ 3600}` → `/b/<hash>?c=<cap>`, where `cap = exp ‖ HMAC(capKey, deviceId ‖ hash ‖ exp)`; checked against the device's revocation on every request. 5B ships the mint and the check; 6 the routes that serve bytes.
- **JOSE:** verification uses `jose` 6 (MIT, no dependencies) for JWS and JWK thumbprints rather than hand-written ECDSA parsing.

### 5.4 Devices

- `auth.device.list`, `auth.device.rename`, `auth.device.role`, `auth.device.revoke` (owner). Revocation sets `revokedAt`, forgets the device's tokens and tickets, **closes its live connections at once** (subscriptions included) and invalidates its capability URLs. A device can revoke itself.
- `auth.devices.watch` streams changes to owner devices.
- A device with role `viewer` or `editor` and `projects: [ids]` sees only those projects (§4.4 "Project reach").

### 5.5 Listeners and TLS

- **Local by default (D6).** The loopback WebSocket of §4.3 is always on. Remote listening is off until the owner enables it: `finisherd remote enable [--listen <host:port>]` writes `settings.json` `server.remote {enabled, listen: [..], tls: {mode: selfSigned|files, certFile?, keyFile?}, publicUrl?, origins?: [..]}`; `FIN_LISTEN=<host:port>` does the same for Docker (env wins, `basis` reported in status).
- **A non-loopback listener is TLS only.** Plain HTTP on a non-loopback address is refused at start (`LISTEN_NEEDS_TLS`).
- **Self-signed mode:** the certificate is made by `rcgen` (in `fin-node`, `ring` provider) from the identity key, with the current LAN and Tailscale addresses and any configured names as SANs. It is remade with the same key when the address set changes, so the pin never changes.
- **Pinning:** the desktop shell and native clients pin the SPKI from the pairing link. **Browsers cannot pin.** A browser reaches a remote server only over a route with a certificate it trusts: certificate files the owner supplies (for example from `tailscale cert`, or a reverse proxy in front), or loopback through an SSH tunnel. In self-signed mode, the web client is offered over loopback only (Owner question 3).
- **Key storage** (`SecretStore`, in `fin-node`): the macOS Keychain, the Windows Credential Manager (DPAPI) or the Linux Secret Service through `keyring` 3 (MIT OR Apache-2.0; `apple-native`, `windows-native`, the pure-Rust `zbus` Secret Service backend). On a headless server or NAS without a secret service: `<server-data>/secrets/identity.pem` (0600), optionally wrapped with AES-256-GCM under a key read from `FIN_SECRETS_KEY_FILE` (a Docker secret). The arch §16 "encrypted file" is that wrapping; without the key file it is a plain 0600 file, and `finisherd status` says so.
- **Rotation** (arch §16): `finisherd tls rotate` makes a new identity key and a statement `{env, oldSpki, newSpki, at}` signed by the old key, served at `/.well-known/finisher/rotation`. A paired client that meets an unknown SPKI fetches the statement over that connection and re-pins if the signature verifies against its old pin. The old key is kept 90 days to sign statements, then deleted. A lost key means re-pairing.
- **Handshake** (arch §16): `GET /.well-known/finisher` (anonymous) → `{envId, protocol {min, max}, spki}` only; names and routes need a principal.

### 5.6 Environments and routes

- **The environment id is the server id** (`<server-data>/server-id`), so the lease holder named in `project.status` (`holder.serverId`) *is* an environment id: a client connected to its local server that finds a project leased by the NAS server can offer "open on <environment name>" when it has paired with that environment. `project.status` gains `holder.envId` (same value, explicit name).
- **Routes** are discovered at start and on address change: `loopback`; `lan` from non-internal interface addresses; `tailscale` from addresses in 100.64.0.0/10 and `fd7a:115c:a1e0::/48`; `public` from `server.remote.publicUrl`; `connect` from the cloud's `ConnectRelay` (open extension point, no local implementation). SSH routes are the desktop shell's (sub-project 9): through a tunnel, a server looks like loopback, which is why loopback grants nothing (principle 1).
- `env.describe` (`server.read`) → the descriptor; `env.name` (owner) sets the name.
- **"Sync" of environments** in arch §16 is read here as: the client keeps its list of environments (sub-project 9); a project moves between environments by folder sync and the lease (4A). Nothing syncs control-plane state between servers in v1 (Owner question 1 decides whether review state should).

### 5.7 Server filesystem reach

- A `device` (and later `agent`) principal may name server paths only inside **allowed roots**: registered library roots (5D), the project roots in `settings.json` `server.projectRoots` (default: `<home>/Finisher`), and the folders of projects already open. `project.create {dir}`, `project.open {dir}`, `asset.probe {path}` and `library.add {root}` outside them → **`PATH_NOT_ALLOWED`** (exit 7). `local` and `console` keep full reach (they run as the user anyway).
- Paths are normalised and canonicalised on the server (symlinks resolved) before the check; `..` and relative paths are `INVALID_PATH` (5A).

### 5.8 RPCs, HTTP and console (5B)

| Surface | Scope | Notes |
|---|---|---|
| `GET /.well-known/finisher`, `…/rotation` | anonymous | §5.5 |
| `POST /pair/redeem`, `POST /pair/status` | anonymous, rate-limited / DPoP | §5.2 |
| `POST /auth/token`, `POST /auth/ws-ticket` | DPoP | §5.3 |
| `GET /rpc?v=&ticket=` | ticket | WebSocket upgrade; 426 on a protocol mismatch |
| `auth.pair.offer`, `auth.pair.pending` (stream), `auth.pair.approve`, `auth.pair.reject` | `auth.admin` | |
| `auth.device.list/rename/role/revoke`, `auth.devices.watch` (stream) | `auth.admin` (a device may revoke itself) | |
| `capability.url` | `project.read` | §5.3 |
| `env.describe`, `env.name` | `server.read` / `auth.admin` | |
| `settings.write {path, value}` | `settings.admin` | `server.remote`, `server.projectRoots`, `server.origins` only, through the merging writer |
| console | — | `pair [--role] [--label]`, `devices`, `revoke <id>`, `remote enable|disable|status`, `tls show|rotate`, `env show|name|adopt|reset` |

### 5.9 Copied or moved server data

When `fin-store::server_id` detects that server data was cloned or moved (the `server-id.origin` check):
- **Reminted id** (a clone): the environment is new. Remote listeners stay off, every device credential is refused (`ENV_MOVED`), and the console offers `finisherd env reset` (new identity key, devices dropped) as the only way on. Two servers holding one identity key would let one impersonate the other.
- **Same id, new place** (a move): remote listeners stay off and devices are refused until the owner runs `finisherd env adopt` (keep the key and devices) or `env reset`.

### 5.10 Security and threat model (5B)

| Threat | Mitigation |
|---|---|
| Leaked pairing link | one-time, ≤ 10 min, consumed at redeem; owner approval with name and safety code; the legitimate device's redeem fails visibly (`PAIRING_USED`) |
| Brute-forced typed code | 40 bits, burn after 5 failures, per-address and per-server rate limits, 10-minute life |
| Stolen device (threat 3) | revoke from any owner device or the console; live connections closed; keys non-extractable where possible; tokens 15 min |
| Stolen access token | DPoP-bound: useless without the device key; tickets one-time and 30 s |
| Network attacker on LAN | TLS only off loopback; SPKI pin from the link; no trust-on-first-use without a link |
| Clock skew | server nonces for freshness; TTLs on the server clock only |
| CSRF / DNS rebinding against the HTTP endpoints | Origin and Host checks; no cookies anywhere; every state change needs a DPoP proof or a principal |
| Remote device reads arbitrary server files | allowed roots (§5.7) |
| Impersonation after a server-data copy | §5.9 |
| Injected agent pairs an attacker's device | offers and approvals are owner-only; `fin` has no pairing command |

### 5.11 Error codes (5B)

| Exit / status | Codes |
|---|---|
| HTTP 400/401/403/409/410/429 (pairing, tokens; mirrored as `FinError` bodies) | `PAIRING_EXPIRED`, `PAIRING_USED`, `PAIRING_REJECTED`, `PAIRING_RATE_LIMITED`, `PAIRING_CODE_BURNED`, `DPOP_INVALID`, `DPOP_NONCE` (with `DPoP-Nonce`), `TOKEN_EXPIRED`, `TICKET_INVALID`, `ORIGIN_REFUSED` |
| 7 | `DEVICE_REVOKED` (`CREDENTIAL_REVOKED` for devices), `PATH_NOT_ALLOWED`, `ENV_MOVED` |
| start | `LISTEN_NEEDS_TLS`, `TLS_FILES_INVALID`, `SECRET_STORE_UNAVAILABLE` (only when no file fallback is possible) |
| client side (sub-project 9) | `TLS_PIN_MISMATCH`, `ROTATION_INVALID` |

### 5.12 Cross-platform (5B)

- **Firewalls:** binding a non-loopback address triggers the macOS application firewall prompt and the Windows Defender Firewall prompt on first use; that is the moment the owner enabled remote access, so it is expected. The Windows installer (sub-project 9) may add a scoped rule.
- **Keychain:** macOS prompts unsigned development builds on every access; development uses the file store with a warning (`FIN_SECRETS=file`). Windows Credential Manager blobs are ≤ 2,560 bytes (a P-256 PEM fits). Linux desktops without a running Secret Service and every headless install use the file store.
- **Docker:** `FIN_LISTEN=0.0.0.0:7420`, the identity key in the volume, optionally `FIN_SECRETS_KEY_FILE`; the container's addresses are not the host's, so `publicUrl` (or Tailscale inside the container) supplies routes.
- **Addresses:** IPv6 link-local addresses are excluded from routes; LAN address changes (DHCP) regenerate the certificate without changing the pin.

### 5.13 Journal and testing (5B)

- `auth.pair` events (§3.4). Pairing and device rows hold names; the journal never does.
- **Tests:** pairing state machine (offer → redeem → approve/reject/expire; reuse; burn); DPoP verifier against RFC 9449 vectors, nonce rotation, `jti` replay, `ath`; revocation closes a live subscription within 100 ms; capability URL expiry and revocation; TLS listener with a self-signed certificate, a client pinning the SPKI, a mismatched pin refused, rotation accepted with a valid statement and refused with a forged one; `LISTEN_NEEDS_TLS`; allowed roots; `ENV_MOVED` after a server-data copy; an e2e `remote` scenario on the Linux VM with a client on another host (no host names in the repo: addresses come from the environment of the test run).

---

## 6. 5C — Jobs

### 6.1 Data model

```text
Job {
  id: ULID, kind: Name (JOB_KINDS), kindVersion, params: JSON (validated by the kind),
  owner: {kind, id}, project?: ProjectId, library?: LibraryId,
  priority: interactive | normal | background,
  state: queued | running | paused | succeeded | failed | cancelled | interrupted,
  attempts, maxAttempts, idemKey? (per owner), dedupeKey? (per kind),
  progress: {done, total?, unit: bytes|items|frames|seconds, phase?: Name},
  checkpoint?: JSON (resumable kinds), result?: JSON, error?: FinError,
  externalRef?: JSON (a provider's job id, persisted before waiting; sub-project 8),
  parent?: JobId, createdAt, startedAt?, finishedAt?, nextRunAt?
}
```

- **Kinds** are declared in `finisherd` (`jobs/kinds/*.ts`): `{kind, version, scope, params (Rust-generated schema for worker kinds), resources, runner: worker:<name> | inline, timeoutS, maxAttempts, resumable, dedupe(params), defaultPriority}`. Sub-project 5 ships `library.scan`, `library.probe`, `library.hash` and, behind `test-hooks`, `test.sleep|echo|crash|hang|spawn`. Sub-projects 6 and 8 add `proxy`, `render`, `transcribe`, `align`, `scenes`, `embed`, `gen`.
- **Worker protocol types** (`fin-jobs` crate, Rust → JSON Schema → generated TS, like `EngineEvent`): host → worker `start {job: {id, kind, params, checkpoint?}, limits}`, `cancel`; worker → host `progress`, `checkpoint {data}`, `item {value}` (streamed partial results, e.g. scan batches), `alive`, `result {value}`, `error {error, itemErrors?}`.

### 6.2 Execution

- **`fin-workers`** (the crate arch §7 names) is one binary with a subcommand per worker family (`fin-workers library`, later `media`, `analysis`). It speaks JSONL over stdio, runs one job at a time, and exits when the job ends. `finisherd` spawns it with `childEnv` (arch §27.7) and finds it beside the `fin-node` binary (as `FIN_NODE_BINARY_DIR` does for tests).
- **A job is coarse:** one worker process per job, never per file. Small work is batched (a `library.probe` job takes up to 256 files), so 60k files need ~240 processes, not 60k.
- **Hard kill of the whole tree** (arch §4 rule 3): Unix workers start in their own process group (`detached`), and the host kills `-pgid` with SIGKILL; on Windows the worker puts itself in a Job Object with kill-on-close at start, so killing the worker kills its children. `cancel` first sends `{"t":"cancel"}` and waits 2 s.
- **Liveness:** a worker that sends nothing for 30 s (progress or `alive` every 5 s) is killed: `WORKER_HUNG`. A worker stuck in uninterruptible I/O (an NFS hard mount) may not die; the host abandons it, counts it, and runs at most 2 abandoned workers per resource before it stops dispatching there (`RESOURCE_STUCK` in status).
- **Background priority:** background jobs run at `nice 10` (Linux, macOS) or `BELOW_NORMAL_PRIORITY_CLASS` (Windows), and on macOS with the background QoS clamp.
- **Bandwidth:** a per-root byte budget (`library.bandwidthMiBs`) is enforced in the worker by a token bucket on reads.

### 6.3 Scheduling

- **Resource pools:** `cpu` (cores − 1 slots, at least 1), `gpu` (1; sub-project 6), `io:<rootKey>` per library root (network and sync roots: 1 scan + 2 probe jobs; local: 4), `net` (4; sub-project 8). A kind declares what it needs; a job runs when every pool has a slot.
- **Order:** priority (`interactive` > `normal` > `background`), then FIFO, with aging (a job waiting 10 minutes is treated one level higher). The scheduler is a pure function `schedule(queue, pools, now) → dispatches`, property-tested.
- **Pause:** a job, a library, a project or everything (`jobs.pause {scope}`). Pausing stops dispatch; running resumable jobs checkpoint and stop; non-resumable ones finish.
- **Limits:** at most 50,000 queued jobs (`JOB_QUEUE_FULL`); finished jobs are kept 7 days or 10,000 rows.
- **Idempotent submit:** the same `idemKey` from the same owner returns the existing job; the same `dedupeKey` while a job is queued or running returns that job (identical work coalesces).

### 6.4 Persistence, retries, restart

- Jobs live in `control.db` `jobs`; state transitions are written at once, progress only in memory (with the checkpoint written at most every 10 s).
- **Restart:** `running` jobs become `interrupted`; resumable ones with attempts left are re-queued from their checkpoint; others fail `JOB_INTERRUPTED`. A job with an `externalRef` is never re-submitted (arch §14.3): its runner resumes polling.
- **Retries:** transient failures (`WORKER_CRASHED`, `WORKER_HUNG`, transient I/O) are retried with backoff 5 s × 2ⁿ (≤ 5 min), up to `maxAttempts` (default 3).
- **Bisection:** a batch job whose worker crashes is split in half and both halves re-run; a single item that crashes twice fails alone (`itemErrors`, `POISON_ITEM`) and the rest succeeds. One corrupt file never stops a library.

### 6.5 Events and hooks

An in-process `JobDone` bus (sub-project 7 hooks attach to it; arch §10 `JobDone`); `jobs.watch` streams; `job.done` journal events (§3.4).

### 6.6 RPCs and `fin` (5C)

| RPC | Scope | Notes |
|---|---|---|
| `jobs.submit {kind, params, priority?, idemKey?, project?}` | the kind's scope (`jobs.submit` and e.g. `library.scan`) | → `Job` |
| `jobs.get {id}`, `jobs.list {filter, cursor?, limit ≤ 500}` | `jobs.read` | owners see their own jobs; `jobs.control` sees all |
| `jobs.cancel {id}` | `jobs.submit` for own jobs, `jobs.control` for others' | |
| `jobs.pause {scope}`, `jobs.resume {scope}` | `jobs.control` (own job: `jobs.submit`) | scope: `{job}` \| `{library}` \| `{project}` \| `{all}` |
| `jobs.wait {ids, any?, timeoutMs ≤ 600000}` | `jobs.read` | → `{jobs, timedOut}` |
| `jobs.watch {filter}` (stream) | `jobs.read` | snapshot, then changes, ≤ 4 Hz per job |

`fin job ls [--state] [--kind]`, `fin job show <id>`, `fin job cancel <id>`, `fin job pause|resume <id|--library <l>|--all>`, `fin wait <id>… [--any] [--timeout <s>]` (loops `jobs.wait`; exit 0 when all succeeded, 4 with `JOB_FAILED` details otherwise, 5 `WAIT_TIMEOUT`). Commands that start jobs (`fin library scan`, later `render`, `transcribe`) print `{job}` and return at once (arch §9.9).

### 6.7 Security, codes, cross-platform, tests (5C)

- **Security:** workers get the cleared environment and no credential; they cannot call `finisherd` (they speak only to their parent's stdio). Parameters are validated before spawn; a worker never receives a path outside the job's library root or project. Sub-project 7 adds the OS sandbox to workers too (they parse untrusted media).
- **Codes:** 3 `JOB_KIND_UNKNOWN`, `JOB_PARAMS_INVALID`; 4 `JOB_FAILED {job, error}`, `JOB_CANCELLED`, `JOB_INTERRUPTED`; 5 `JOB_QUEUE_FULL`, `WORKER_UNAVAILABLE` (binary missing), `WAIT_TIMEOUT`, `RESOURCE_STUCK`; `JOB_NOT_FOUND` (3); in results and item errors: `WORKER_CRASHED`, `WORKER_HUNG`, `JOB_TIMEOUT`, `POISON_ITEM`.
- **Cross-platform:** process groups vs Job Objects; Windows `TerminateProcess` on a worker that holds an open file on SMB (sharing violations for the next attempt are transient); signals do not exist on Windows (cancel is the stdio message, then the kill).
- **Tests:** scheduler properties (no pool over-commit, priority order with aging, pause/resume, dedupe); a `test.spawn` worker whose grandchild must be gone after cancel on all three OSes; `test.crash` bisection finds the one poison item in 256; restart turns `running` into `interrupted` and resumes from the checkpoint; `fin wait` exit codes; `job.done` entries.

---

## 7. 5D — Library indexer

### 7.1 Data model

- `libraries` (in `control.db`): `{id ULID, name (user text), root (absolute server path), rootKey (canonical, case-folded where the volume is case-insensitive), storage: local|network|sync (detected by `fin-store::storage_kind`, with override), state: online|offline|scanning|paused, include?, exclude (defaults below), kinds (media, image, audio), scan {pollS, full: daily|weekly|manual}, hydrate: false, bandwidthMiBs?, stages {probe: true; later proxy, transcribe, scenes, embed}, embeddings: local|<cloud provider> (8), createdBy, createdAt}`.
- **Libraries are per server** (arch §20: "library roots are mapped per machine, stored per server, not in the project"). Libraries are read-only to Finisher (arch §6.7): nothing is ever written under a root.
- **Catalog** `<server-data>/libraries/<id>/catalog.db` (`node:sqlite`, WAL), one per library, so a removed library is one deleted directory and a 60k-file catalog never bloats `control.db`:
  - `dirs {id, rel, mtimeNs, lastScan}`;
  - `files {id, dirId, name (raw bytes as a BLOB) and nameNfc (display), relKey, size, mtimeNs, fileId (dev:ino, or the NTFS/SMB file id), placeholder, fp (fp1, the asset fingerprint of `fin-probe`), fullHash?, kind, probe (JSON), state: present|missing|cloudOnly|unreadable|probeFailed|skipped, code?, firstSeen, lastSeen, lastChange}`;
  - `stages {fp, stage, version, state: pending|running|done|failed|skipped, code?, at}`: keyed by fingerprint, so the same file in two libraries is analysed once;
  - `names` (FTS5 over `nameNfc` and directory names), `scans {id, full, startedAt, finishedAt, counts}`.
- **Default excludes:** dot-files and dot-directories, OS junk (`Thumbs.db`, `.DS_Store`, `@eaDir`, `#recycle`, `$RECYCLE.BIN`, `System Volume Information`), sync conflict copies by the patterns `fin-store` already knows, and inside any Finisher project found under a root, everything but `sources/`.
- **Overlap rules:** a root must be an existing absolute directory, not inside server data, not equal to, inside or containing another library's root (`LIBRARY_OVERLAP`).

### 7.2 Scanning

- **Full scan** (`library.scan {full: true}` job; on add, on mount coming back, on the schedule, on demand): the worker walks the root with parallel `stat`s (32 on network roots, as measured in spike S10; 64 locally), never follows symlinks or Windows reparse points (recorded as `skipped`), and streams batches of `{rel, size, mtimeNs, fileId, placeholder}`.
- **The indexer thread** (a `worker_thread` in `finisherd` that owns every catalog connection) diffs each batch against the catalog: new and changed files (size, mtime or file id differ) are queued for `library.probe`; a missing file id that reappears elsewhere with the same size and mtime is a **rename** (its fingerprint and analysis carry over); a new file whose fingerprint matches a missing one is a **move**; files not seen by a completed full scan become `missing`.
- **Incremental scans:**
  - *network roots:* every `pollS` (default 60 s, spike S10) the worker re-stats known directories and rescans those whose mtime changed. A directory's mtime does not change when a file inside is rewritten in place, so in-place rewrites wait for the next full scan (daily by default). On macOS network shares, it refreshes listings first as `fin-store` does (the 4A smbfs 60 s directory cache);
  - *local roots:* `fs.watch` (recursive) is a **hint** that queues a targeted rescan of the touched directory after 2 s of quiet. Watch errors (Linux inotify limits, Windows buffer overflow) fall back to polling every 5 minutes and say so in status. Correctness comes from scans, never from events alone;
  - *sync roots:* polled every 300 s, plus the daily full scan.
- **An offline root never marks files missing.** If the root cannot be stat'ed (or a stat takes more than 10 s), or a full scan finds the root empty while the catalog holds files, the library goes `offline` (`LIBRARY_OFFLINE` / `ROOT_EMPTY_SUSPECT` in status), keeps its catalog and analysis (arch §6.7 "offline libraries"), and is re-checked every 60 s. An unmounted NAS must not turn 60k files into `missing`.

### 7.3 Fingerprint, probe and stages

- `library.probe` jobs compute `fp1` and run `fin-probe` on each file in one pass (both read the head; the probe parses untrusted bytes, so it runs in the worker with `fin-probe`'s `catch_unwind`, and bisection isolates poison files).
- **The fingerprint stays `fp1`** (size, mtime and sampled head/middle/tail BLAKE3, as `fin-probe` computes it for assets). A library file and the asset made from it share the fingerprint, so `fin asset add <path in a library>` reuses the catalog's probe. A copy with another mtime gets another fingerprint; relinking across machines then falls back to the full hash (arch §20), computed by `library.hash` on demand, never in a sweep.
- **Placeholders** (cloud-only files in sync folders) are catalogued from metadata only and **never read**: reading one makes the provider download it (iCloud, OneDrive, Dropbox and Google Drive File Provider). macOS: `SF_DATALESS` in `st_flags`; Windows: `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`, `RECALL_ON_OPEN` or `OFFLINE`; the legacy `.<name>.icloud` stubs. Such files are `cloudOnly`. Setting `hydrate: true` (owner-only: it downloads) lets probe jobs read them, within the bandwidth budget.
- **Stages** are a framework: a stage `{name, version, appliesTo(file), dependsOn, jobKind, batch}` registered by a later sub-project; when a file's fingerprint has no `done` row for a stage at its current version, the indexer queues the stage's job. Sub-project 5 registers only `probe`. A new stage version re-queues only that stage.
- **Analysis artifacts** are keyed by fingerprint in `<server-data>/cache/` (arch §6.1); nothing in sub-project 5 writes there.

### 7.4 Search and lookup

- `library.search {query?, libraries?, kind?, duration?, width?, height?, since?, cursor?, limit ≤ 200}`: FTS5 over names (prefix and phrase), plus metadata filters from the probe. Sub-project 8 adds `mode: semantic` and transcript matches to the same RPC.
- `library.lookup {fingerprints | hashes}` → matching files across this server's libraries (relinking, arch §20).

### 7.5 Security and privacy

- **Registering a root grants read access to a tree** through search and listings, so `library.add` is owner-only (§4.5). Agents can list, search and rescan registered libraries.
- Library and file names are user content. They stay in the catalog on local disk; they are never journaled (`library.scan` carries counts only), never logged at info level.
- `library.files`/`library.file` take relative paths inside the root; `..`, absolute paths and paths that canonicalise outside the root are `INVALID_PATH`.
- Untrusted media is parsed only in workers.

### 7.6 RPCs and `fin` (5D)

| RPC | Scope | Notes |
|---|---|---|
| `library.add {name, root, options?}`, `library.remove {id}`, `library.set {id, options}` | `library.admin` (owner) | `set` of name, excludes and poll intervals only needs `library.scan` |
| `library.list`, `library.status {id}` | `library.read` | status: state, counts by state, stage progress, last scan, warnings |
| `library.scan {id, full?}` | `library.scan` | → `{job}` |
| `library.pause`, `library.resume` | `library.scan` | pauses its jobs (§6.3) |
| `library.files {id, dir?, state?, cursor?, limit}`, `library.file {id, rel \| fileId}` | `library.read` | file: probe, stage states, fingerprint |
| `library.search`, `library.lookup` | `library.read` | §7.4 |
| `library.watch {id?}` (stream) | `library.read` | status and scan progress |

`fin library ls|status|scan|files|show|search` and `fin search <query>` (the same search; arch §9.11). The console adds `finisherd library add <name> <root>|rm <id>|set <id> …`.

### 7.7 Codes, cross-platform, tests (5D)

- **Codes:** 3 `LIBRARY_ROOT_INVALID`, `LIBRARY_OVERLAP`, `INVALID_PATH`; 3 `LIBRARY_NOT_FOUND`, `FILE_NOT_FOUND`; 4 `FILE_CLOUD_ONLY` (asked to probe a placeholder without `hydrate`); 5 `LIBRARY_OFFLINE`, `LIBRARY_ROOT_UNREADABLE`; status warnings `ROOT_EMPTY_SUSPECT`, `WATCH_DEGRADED`, `RESOURCE_STUCK`.
- **Cross-platform:**
  - names: macOS stores NFD; matching uses NFC and, on case-insensitive volumes (APFS and NTFS by default, most SMB shares), a case-folded `relKey`; the raw bytes are kept for opening (Linux names need not be UTF-8);
  - case sensitivity is detected per root without writing (stat the root path with its case flipped);
  - Windows long paths (`\\?\`), reparse points and junctions skipped, file ids from `FileIdInfo` (the 4A machine-id work);
  - SMB/NFS semantics of §7.2; inotify limits; FSEvents coalescing;
  - placeholders per OS (§7.3).
- **Tests:** a generated tree of 10k files (local) with renames, moves, in-place rewrites, deletions and placeholder flags (faked through a test hook); an offline root marks nothing missing; a poison file is isolated; overlap rules; NFD/NFC and case-folding fixtures; `fin asset add` reuses the catalog probe; FTS queries; on the Linux VM and macOS, a network-root scan against the SMB share used by 4A Task 19 (paths from the environment, never in the repo), with poll latency measured.

---

## 8. 5E — Review workflow

### 8.1 Data model

- `reviews {id ULID, projectId, kind: branchReview | mergeConflict (later: qaFinding, criticSummary, pluginSuggestion), state, source (branch id), target (branch id), sourceHead, targetHead (seqs at the last refresh), title (user or agent text), summary?, requestedBy {principal, actor}, createdAt, updatedAt, closedAt?, closedBy?, mergeId?}`.
- **States:** `open` → `changesRequested` ↔ `open` → `merged` | `rejected` | `dismissed` | `superseded` (the source was merged or archived some other way).
- **Change list** (derived, cached in the row): one entry per merge candidate of the source path, as `history.merge {dryRun: true}` plans it: `{txId (origin root), record kind, author, actor?, ops: {kind: count}, at, decision: pending|accept|reject, decidedBy?, reason?, status: willMerge|alreadyPresent|excluded|dependsOnSkipped|conflict}` plus `conflicts: Conflict[]` (fin-core's `Conflict`, already generated).
- `review_events {itemId, at, kind: opened|refreshed|decided|commented|merged|closed, principal, details}` and `review_comments {id, itemId, principal, actor, body (user or agent text), at?: rational timeline time, txId?, createdAt}`.
- **Where review state lives:** in `control.db` of the server that holds the project (Owner question 1). What the op log already holds (branches, merges, archives) is the durable truth; a `branchReview` for an active, unmerged branch can be re-derived after a move, while decisions not yet merged and comments stay on the server that recorded them.
- `branch_policy {projectId, protected: [branchId], requireReview: bool}`, default `protected: [main]`, `requireReview: false`.

### 8.2 Workflow

1. **Request.** `review.request {projectId, source, into?: main, title, summary?}` (an agent at the end of a draft, a person, or sub-project 7's `BranchReady` hook) opens a `branchReview`. Opening runs a merge dry run, which works on followers too (4B Decision 26).
2. **Live refresh.** When the source or target head moves (the `finisherd` hubs already see every commit), the item is re-planned after 1 s of quiet. Decisions are keyed by origin txId, so they survive refreshes; a decided entry that vanished (undone and dropped) is reported, not silently lost.
3. **Decide.** `review.decide {itemId, decisions: [{txId, decision, reason?}], expect: {sourceHead, targetHead}}` marks entries. `expect` is optimistic concurrency: if either head moved since the reviewer's view, the call is `REVIEW_STALE` with the fresh change list, so nobody accepts work they did not see.
4. **Merge.** `review.merge {itemId, expect, idemKey}` merges with `history.merge {from, into, exclude: [rejected txIds], skipConflicting: false}` as the reviewer's principal. It is refused with `REVIEW_HAS_CONFLICTS` while the plan has conflicts (resolve by rejecting the conflicting entries, or by fixing the source branch: updating it from the target and editing), and with `REVIEW_UNDECIDED` when `requireReview` is on and entries are pending (otherwise pending means accept). The merge is audited when the target is protected. After it: state `merged`, `mergeId` stored, the source archived when it was an agent branch (`archiveSource`, default on for agent-created branches).
5. **Reject, request changes, dismiss:** `review.close {itemId, state: rejected|dismissed}`; `review.decide` with `changesRequested` (and a comment) hands the branch back to its agent (sub-project 7 delivers it as a turn).

### 8.3 Merge exclusions (a `fin-store` change, not a primitive op)

- `MergeRequest` gains **`exclude: Vec<TxId>`** (origin txIds). An excluded candidate is not copied and is reported in `skipped` with reason **`EXCLUDED`**. Everything that depends on it is handled by the existing rules: a conflict blamed on it and an undo, redo or revert whose unit holds it are `DEPENDS_ON_SKIPPED` (sub-project 4 §4.4).
- An exclusion is a planning input only: the merge records written are ordinary copies, so followers, rebuilds and `derived::compute` need nothing new. The source stays `active` (`left_to_merge` still sees the excluded records), which is right: they were not merged. `fin merge` gains `--exclude <txId>…`.

### 8.4 Conflicts become review items

Whenever any `history.merge` (from `fin`, a review or the UI) answers `CONFLICT`, `finisherd` opens or updates a **`mergeConflict`** item for `(source, target)` with the conflict list and who tried. It becomes `superseded` when a later merge of the pair succeeds or the source is archived, and can be dismissed. Rebase conflicts on `tx.commit` are answered to the writer (an agent re-plans) and are not review items: they never left the writer's hands.

### 8.5 Protected branches

- A write whose target is protected (`tx.commit`, `history.undo|redo|checkpoint|cherryPick|revert` on it, `history.merge` into it, `branch.set` of it) needs `branch.protected` (§4.4): `BRANCH_PROTECTED` (exit 7) otherwise, with the fix `fin review request --branch <b>`.
- `review.merge` into a protected branch needs `review.decide`, not `branch.protected`: reviewing is the sanctioned way in.
- Policy writes (`review.policy {projectId, protected, requireReview}`) are owner-only.

### 8.6 RPCs and `fin` (5E)

| RPC | Scope |
|---|---|
| `review.request`, `review.comment {itemId, body, at?, txId?}`, `review.refresh {itemId}` | `review.write` |
| `review.list {projectId, state?, kind?}`, `review.get {itemId}`, `review.watch {projectId}` (stream) | `project.read` |
| `review.decide`, `review.merge`, `review.close` | `review.decide` |
| `review.policy`, `review.policy.get` | `auth.admin` / `project.read` |

`fin review request [--into <b>] --title <t> [--summary <s>]`, `fin review ls`, `fin review show <id>`, `fin review comment <id> <text>`, `fin review decide <id> --accept <tx>… --reject <tx>…`, `fin review merge <id>` (the last two need `review.decide`, which `local` has and `agent` principals lack by default).

### 8.7 Security, codes, cross-platform, tests (5E)

- **Security:** an agent can open a review and comment, but not decide or merge into protected branches (sub-project 7 defaults); review text is content and shown as-is (arch §17.1); the change list is computed by the server, never taken from the requester.
- **Codes:** 3 `REVIEW_NOT_FOUND`; 4 `REVIEW_STALE {fresh}`, `REVIEW_HAS_CONFLICTS {conflicts}`, `REVIEW_CLOSED`; 3 `REVIEW_UNDECIDED {pending}`; 7 `BRANCH_PROTECTED`; merge reason `EXCLUDED`.
- **Cross-platform:** nothing beyond the store.
- **Tests:** `exclude` property tests in `fin-store` (excluding any subset never copies an excluded record or anything depending on it, and equals cherry-picking the complement in order when there are no undos); refresh after source and target commits; `REVIEW_STALE`; conflicts open and supersede items; protected `main` refuses an `agent` commit and accepts `review.merge`; `review.verdict` entries; an e2e `review` scenario driven by `fin` with two credentials (an agent and the local principal as reviewer).

---

## 9. Server and RPC surfaces for the frontend (sub-project 9)

The frontend needs nothing it cannot get from these, all over the ticketed WebSocket as a `device` principal:
- **Connect:** `/.well-known/finisher`, pairing (§5.2), `/auth/token`, `/auth/ws-ticket`, `server.hello`, `auth.whoami`, `env.describe`; HTTP 426 on a protocol mismatch.
- **Live state streams:** `ops.subscribe`, `project.events` (existing), `jobs.watch`, `library.watch`, `review.watch`, `auth.pair.pending`, `auth.devices.watch`; each starts with a snapshot item and is closed on revocation.
- **Settings and consent screens:** `diagnostics.status/consent` (owner device), `settings.write` (owner), `server.status`.
- **Libraries UI** (arch §17): `library.list/status/files/search`, progress through `library.watch`, offline state; `library.add` from an owner device.
- **Review mode** (arch §17): `review.*`, change lists with per-transaction decisions, A/B compare through `eval.frame` on both branches.
- **Media:** `capability.url` for `<video src>` and thumbnails (routes in sub-project 6).
- **Errors** are codes with parameters (arch §17.1); every code above is in `@finisher/contracts` for the catalogue.

---

## 10. Dependencies and licences

| Need | Choice | Licence | Batch |
|---|---|---|---|
| Control store, catalogs, FTS | `node:sqlite` (Node built-in) | MIT (Node), public domain (SQLite) | 5A, 5D |
| HMAC, ECDSA P-256, AES-GCM, random | `node:crypto`; Rust `hmac` 0.12 + `sha2` 0.10 in `fin` | MIT OR Apache-2.0 | 5A |
| WebSocket server under our protocol | `ws` 8 (already under `@effect/platform-node`; pinned directly) | MIT | 5A |
| DPoP / JWS / JWK thumbprints | `jose` 6 | MIT | 5B |
| Self-signed certificates | `rcgen` 0.14 with the `ring` provider, in `fin-node` | MIT OR Apache-2.0 (`ring`: Apache-2.0 AND ISC, already allowed) | 5B |
| Keychain | `keyring` 3 (`apple-native`, `windows-native`, Secret Service over `zbus`) | MIT OR Apache-2.0 (`zbus` MIT) | 5B |
| Windows ACLs, Job Objects, attributes | `windows-sys` 0.61 (already in the tree) | MIT OR Apache-2.0 | 5A, 5C, 5D |
| NFC keys | `unicode-normalization` 0.1 | MIT OR Apache-2.0 | 5D |
| Fingerprint, probe | `fin-probe` (ours), `blake3` | Apache-2.0 / CC0 (already allowed) | 5D |

**Rejected:** `better-sqlite3` (MIT, but a native module with its own Electron ABI prebuild matrix, which arch §7 already counts as a risk); `node-forge` (BSD-3-Clause OR GPL-2.0: usable, but the licence gate should not have to reason about dual copyleft); `keytar` (archived); `dbus`-crate backends of `keyring` (link `libdbus`, AFL-2.1 OR GPL-2.0+); mDNS libraries (not needed in v1); any JWT library (no JWTs are issued). Each batch's plan runs `cargo deny` and the JS licence gate over what it adds.

---

## 11. Decisions taken here

| # | Decision | Section |
|---|---|---|
| 1 | Trust comes from credentials, never from transports; loopback grants nothing | §2, §4.2 |
| 2 | Local credentials are key files in server data with a mutual HMAC handshake; this replaces peer-credential checks (arch §8.4), which Node cannot do | §4.2 |
| 3 | Keys rotate at every start; agent keys derive from a master, so they survive restarts and need no storage | §4.1 |
| 4 | One custom RPC protocol owns framing and connection state; one wrapper authorises every handler | §4.3 |
| 5 | WebSocket bearer token removed; one-time tickets bound to the minting principal | §4.3 |
| 6 | A closed scope vocabulary, one scope per RPC plus payload escalations, gate-tested | §4.4 |
| 7 | Human-only operations land in 5A (not 7), with `finisherd` console commands as the human interface until sub-project 9 | §4.5, §4.6 |
| 8 | `console` becomes an event `Source`; sources and actors derive from principals | §3.4 |
| 9 | `fin` exit code 7 = denied | §3.5 |
| 10 | Protocol 2 at 5A, carrying sub-project 4's pending protocol-2 changes | §3.6 |
| 11 | Control-plane state in `node:sqlite`, versioned like every format, with a per-statement budget | §3.1 |
| 12 | The audit log is a separate, always-on, local store; never uploaded | §3.3 |
| 13 | Pin the identity key (SPKI), not certificates; the TLS key is the identity key | §5.5 |
| 14 | Opaque in-memory tokens; DPoP freshness from server nonces, not client clocks | §5.3 |
| 15 | Environment id = server id, so lease holders map to environments | §5.6 |
| 16 | Non-loopback listeners are TLS only; remote is off by default | §5.5 |
| 17 | Remote principals reach server paths only inside allowed roots | §5.7 |
| 18 | One worker process per coarse job, batches with crash bisection, process-tree kill | §6.2, §6.4 |
| 19 | One catalog database per library, written by one indexer thread | §7.1 |
| 20 | The library fingerprint stays `fp1`; full hashes only on demand | §7.3 |
| 21 | Placeholders are never read unless the owner turns on hydration | §7.3 |
| 22 | An offline or suspiciously empty root marks nothing missing | §7.2 |
| 23 | Review change lists are merge dry runs; rejections are `history.merge` exclusions in `fin-store` | §8.2, §8.3 |
| 24 | `review.verdict` is filled in 5E (not 10); sub-project 10 adds marker verdicts | §3.4 |
| 25 | Protected branches per server, default `main`; `local` keeps the scope, agents do not | §8.5 |

---

## 12. Owner questions

1. **Where does review state live?** *Recommended:* in `control.db` of the server that holds the project (§8.1); branches, merges and archives (the durable facts) are already in the synced op log, and open branch reviews are re-derived after a move. *Trade-off:* pending per-transaction decisions and review comments do not follow a project to another server. The alternative, write-once review event files in `.finisher/review/` (sync-safe, any server can append, comments from read-only followers possible), costs a second synced format with its own conflict, compaction and `collect: never` rules.
2. **Make the human-only operations human-only now (5A), with `finisherd` console commands, rather than in sub-project 7?** *Recommended:* yes. *Trade-off:* until sub-project 9's UI, consent changes, purges, `project default`, confirmed takeovers and relocations need a terminal on the server (`docker exec` on a NAS). Agents (and the e2e and Rust tests) lose `fin diagnostics consent|purge|project default`, `fin project takeover --confirm` and `--relocate`. Waiting for sub-project 7 leaves these open to every agent for two more sub-projects.
3. **How do browsers reach a remote server?** *Recommended:* only over a route whose certificate the browser trusts (owner-supplied files such as `tailscale cert`, or a reverse proxy), or over loopback through an SSH tunnel; self-signed SPKI pinning is for the desktop shell and native clients. *Trade-off:* the v1 web client (§1 item 7) on a LAN needs one setup step. Alternatives: ship an ACME client (needs public DNS and port 80/443, wrong for a NAS behind NAT), or let users click through a certificate warning and compare fingerprints (trains users to accept warnings).
4. **May the local audit log keep device names and remote addresses?** *Recommended:* yes, for pairing, revocation and failed authentication only; local, owner-only, never uploaded, 180-day retention. *Trade-off:* it is the one store holding IP addresses (D34 forbids them in the journal); without them, a stolen-link or brute-force incident cannot be traced.
5. **Should the `local` principal (anything that can read `<server-data>/auth/local.key`: `fin` run by the user and, before sub-project 7, any agent harness the user starts) keep `branch.protected` and `review.decide`, so that `fin` can still commit and merge into `main`?** *Recommended:* yes, keep today's behaviour for `local`; protect `main` for `agent` principals (sub-project 7) and remote non-owner devices. *Trade-off:* an agent run outside Finisher's runtime can still write `main` without review until it runs under sub-project 7. Revoking it now forces every `fin` flow (and the existing tests) onto branches and reviews before the runtime that creates those branches exists.

---

## 13. Risks

| Risk | Mitigation |
|---|---|
| Effect 4.0.6's RPC server hides the connection from handlers | 5A Task 3 starts with a spike; the custom protocol owns framing either way, and can inject a server-only connection header after stripping client ones |
| `node:sqlite` blocks the event loop under load | per-statement budget test; bulk work on threads; fallback: the same schema through `rusqlite` in `fin-node` (async napi) |
| Windows pipe squatting and ACLs differ on non-elevated or domain users | server proof works without pipe ACLs; validation on a non-elevated account (already outstanding for S8) |
| Keychain prompts and missing Secret Service | file store with optional wrapping; `FIN_SECRETS=file` |
| Browsers and self-signed TLS | Owner question 3 |
| NAS scans hang in uninterruptible I/O | worker processes, abandonment limits, offline state |
| Sync providers hydrate on read | placeholder detection on every OS; never read by default; tests with faked flags plus a real iCloud/OneDrive check in validation |
| Review refresh cost on busy branches | debounce; dry runs off the commit lock where possible (they plan in a rolled-back IMMEDIATE transaction today, 4B Decision 26) — measured in 5E |

---

## 14. Findings in the architecture spec

Contradictions and gaps found while designing this; each batch's "as built" pass records the resolution in arch §27.

1. **§21.1 decides trust by transport** (local socket → `fin`, WebSocket → `ui`, and only `ui` may opt in), but the WebSocket token sits in a file any same-user process can read, and loopback is reachable through SSH tunnels. Resolved by §3.4 and §4.3.
2. **§8.4 "peer-credential checks on the local socket"** is not possible from Node (no `SO_PEERCRED`, no pipe client process id), Landlock (ABI 4, as used in spike S8) does not mediate `connect()` on pathname sockets, and Windows' default pipe DACL lets every user open the pipe for reading. Resolved by key-file proofs (§4.2).
3. **§16 pins a self-signed certificate "at pairing"** for every client, but browsers cannot pin, and the web client is a v1 deliverable (§1 item 7). Owner question 3.
4. **§16 DPoP-bound tokens** assume usable client clocks (`iat`), while §27.9 records clients off by ~10 s for a day. Resolved by server nonces (§5.3).
5. **§21.1 phasing puts human-only operations in sub-project 7**, although principals and scopes are sub-project 5, and arch §24 puts `review.*` verdicts in sub-project 10 while §11.3's per-transaction accept/reject is the review workflow of sub-project 5. Owner question 2; Decision 24.
6. **§11.3 review markers `open/review/resolved`** do not exist in the 4C model (`Marker {id, at, duration, label, color}` has no status). Sub-project 10 needs a model change (an op), not only a control-plane feature.
7. **§6.4 "conflicts become review items"** presumes something can be done per transaction, but 4B's merge is all-or-nothing with no exclusion input. Resolved by §8.3 (a store change, not a primitive op).
8. **§5 says the local `AuthProvider` has a "single owner"**, while §16 has the owner approve "device name and role": roles imply more than one person. Resolved as one owner plus device roles (§3.2).
9. **§16 "environments … sync"** is one line; nothing defines what syncs between environments. Read as §5.6; Owner question 1 decides the one candidate (review state).
10. **§6.7 fingerprints include mtime**, so a copy on another machine never matches by fingerprint and relinking (§20) always falls back to a full hash, which reads whole files over the network. Accepted and made on-demand (§7.3), but the cost is not in the spec.
11. **§6.7 library change detection** relies on "native file-system events on local disks" without the inotify watch limits, and says nothing about cloud placeholders, where any read (including a fingerprint's samples) downloads the file. Resolved by §7.2 and §7.3.
12. **§7 gives TS "its own SQLite"** but also a lint against synchronous calls on the event loop; every Node SQLite binding without a native module is synchronous. Resolved by the budget in §3.1.
13. **§16 audit log vs D34 "never stored raw: host names, IPs"**: the spec does not say whether the rule covers the audit log. Owner question 4.
14. **4A/4B expose "a person's decision"** (`project.takeover {confirm}`, `project.open {relocate}`) to `fin`, which D16 makes agent-only; the D-plan likewise uses `fin` for consent. Resolved by §4.5.
15. **Tracked files hold personal data, against the public-repo rule:** arch §27.8 names the Apple Developer team's owner and team id, and D3 holds an absolute local path. Worth scrubbing before the repository is public (the team id is visible in any signed build anyway; the personal name and path are not needed).
