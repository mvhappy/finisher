# Sub-project 5: Control plane breadth — Design

- **Status:** revision 3 (2026-11-07). Revision 2 answered an independent review of revision 1 (`45039ab`; §15 maps every finding). Revision 3 applies the cross-design reconciliation with sub-projects 6, 7 and 8 (§16): 5C owns the shared job system, adds warm workers and helpers, calls sub-project 7's `fin-sandbox`, and is split so its engine can start after 5A Tasks 1–2. Nothing here is built yet.
- **Parent:** `docs/specs/2026-11-01-finisher-architecture-design.md` (cited below as *arch §n*). This document refines arch §4 rule 6, §5 (`AuthProvider`, `ConnectRelay`), §6.4 ("conflicts become review items"), §6.7 (media libraries and indexing jobs), §8.3 (allow/ask/deny), §8.4 (token delivery, the env allow-list), §9.9 (jobs), §11.3 (per-transaction accept/reject), §16 (pairing, auth, security), §20 (library root mapping), §21.1 (sources and human-only operations) and §27.12.5 (the local transport and its WebSocket token). Where the two disagree, this document wins for sub-project 5, and arch §27 gets a pointer when each batch merges.
- **Builds on:** sub-projects 2 and 4 (4A, 4B, 4C, on stacked branches; 4C is `sp4c-edit-breadth`) and the diagnostics hooks D1–D7 on the 4C branch. **D1 and D2 are done; D3–D7 are not.** 5A's Tasks 1, 2 and 10 can start now; the rest of 5A needs D4 (sources per transport) and D6 (actor headers), which it changes (§3.4).
- **House style:** as `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md`. Each batch gets its own plan; each batch section is rewritten "as built" when it lands.

---

## 1. Goal, scope and batches

### 1.1 Goal

Sub-project 5 turns `finisherd` from a single-user local server into the control plane that the agent runtime (7), plugins (8), the frontend (9) and the reviewer (10) consume:
- **every caller is a principal** with a credential and scopes, on every transport; every RPC declares the scopes it needs, and one middleware chain checks them;
- **human-only operations** are enforced by the server, not by convention;
- devices **pair** with a server and reach it **remotely** over pinned TLS; a server is an **environment** with a stable id, bound to its identity key, and ordered routes;
- long work runs as **jobs**: persisted, scheduled by resource, cancellable with a hard kill of the whole process tree, resumable, observable;
- registered **media libraries** are catalogued, fingerprinted, probed and kept current on local disks, NAS shares and sync folders, through jobs, without downloading cloud placeholders or keeping NAS disks awake;
- agent branches go through a **review workflow**: a change list per transaction, merges limited to what was reviewed, rejections recorded durably in the op log, merge conflicts as review items, protected branches.

### 1.2 Non-goals and v1 cuts

**Not in sub-project 5:**
- **The frontend** (sub-project 9). §9 defines the server and RPC surfaces it needs. The desktop shell's keychain storage of its device key, SSH/WSL launch and forwarding are also sub-project 9 (arch §16).
- **Agent sessions, modes, roles, the OS sandbox and command-pattern rules** (sub-project 7). Sandboxing is sub-project 7's `fin-sandbox` crate with the profiles `agent`, `hook`, `worker` and `plugin`; 5C calls it when it spawns (§6.2). Sub-project 5 defines the `agent` and `mcp` credentials, their file format, their verifier and the middleware stage that sub-project 7's rules and hooks plug into (§4.6). It also states what each sandbox must allow so that `fin` can reach the server (§4.11).
- **Plugin credentials and permissions** (sub-project 8). The `plugin` principal kind is reserved.
- **Analysis stages** beyond probing: proxies (6), transcription, alignment, scenes, embeddings and semantic search (8). §7 builds the stage framework they plug into.
- **QA gate, critic, review markers, share page** (sub-project 10). §8 builds the review items they will produce and consume.
- **Hosted cloud:** accounts, OAuth/SSO, orgs, the Connect relay, hosted share links. Only the open extension points are defined.
- **LAN discovery (mDNS/Bonjour)** and **remote `fin`** (`fin` talks to a local socket only; agents run next to the server, arch §6.7).

**Cut from v1 (accepted after review):**
- **Device roles and per-device project lists.** The local server has a single owner (arch §5), and every paired device *is* the owner. `editor`/`viewer` roles and project lists come with accounts in `finisher-cloud`.
- **Typed pairing codes.** Pairing is by link or QR code only.
- **Capability URLs** move to sub-project 6, which serves the bytes (and must give them project reach, §15 H4).
- **Signed TLS rotation statements.** Rotating the identity key means re-pairing (§5.5).
- **Live re-planning of reviews.** Change lists are planned when an item is opened, refreshed on demand and re-checked at decide and merge time (§8.2).
- **Pausing jobs by library or project.** A job or everything can be paused (§6.3).
- **`externalRef`** on jobs (a provider's job id for paid calls) comes with generation providers in sub-project 8.

### 1.3 Success, for every batch

- Every capability is driven and tested through `fin` → `finisherd`, or through the `finisherd` console (§4.7) for human-only ones.
- `cargo test --workspace`, `pnpm test`, `pnpm gates` and `pnpm e2e` pass on macOS, Windows 11 and the Linux VM, **at every commit**.
- No UI work. No host names, IPs, user names or key names in tracked files, fixtures included.

### 1.4 Batches and order

| Batch | Delivers | Depends on | Unblocks |
|---|---|---|---|
| **5A** Principals, local credentials, scopes | `control.db` and the audit log; the environment id; principals; key files with a mutual handshake bound to the environment and the endpoint; the socket in `<server-data>/run/` and a native Windows pipe listener with its own security descriptor; one-time WebSocket tickets; the authorisation middleware chain with store-resolved references, reach and an `ask` outcome; human-only enforcement; the TTY-gated `finisherd` console; `agent` and `mcp` credentials; event sources from credentials; protocol 2; the sub-project-5 follow-ups in `TASKS.md` | D1–D2 (Tasks 1, 2, 10); D4, D6 (the rest) | everything below; sub-project 7 |
| **5C-1** Job engine | the shared job system (§6.0): `fin-jobs` (protocol types, envelope), the one `fin-workers <family>` binary with the `test` family, spawning in `fin-node` through a sandbox hook, a host-owned kill tree on every OS, the persisted scheduler with pools and warm workers, helpers and mapped files, retries, poison handling and isolation, stall detection, `job.done` and `worker.exit` | 5A Tasks 1–2 | 5D; sub-project 6 (6A-1 runs beside it), 8 |
| **5C-2** Job RPCs | `jobs.*` with scopes, ownership and reach, `jobs.watch`, `fin job …`, `fin wait` | 5C-1; 5A Tasks 3–6 | sub-projects 6, 8 (their `fin` commands that return job ids) |
| **5D** Library indexer | the `library` worker family; libraries per server, catalog databases, scans for local/network/sync roots that never open placeholders, mount and volume identity, demand-driven NAS polling, fingerprint and probe stages through jobs, the stage framework, offline libraries, metadata search, `fin library …`, `library.scan` events | 5C-1 (5C-2 for `fin library scan`'s job id) | sub-projects 6, 8, 11 |
| **5E** Review workflow | review items, change lists from merge dry runs, merges limited by `include`/`sourceUpTo`, durable `drop` records in the op log, conflicts as review items, protected branches checked inside the store's write transaction, `fin review …`, `review.verdict` events | 5A | sub-projects 7, 10 |
| **5B** Pairing, devices, remote access, environments | device keys, pairing links with owner approval, DPoP-bound tokens with server nonces, WebSocket tickets for devices, TLS listeners with SPKI pinning, environment descriptors and routes, revocation, server filesystem reach for remote principals, the secret store | 5A | sub-project 9 |

**Order:**

```text
5A Tasks 1–2 ──┬──▶ 5A Tasks 3–14 ──▶ 5C-2 ──▶ 5D ──▶ 5E ──▶ 5B
               └──▶ 5C-1 ───────────┘ (beside sub-project 6's 6A-1)
```

- **5C-1 starts right after 5A Tasks 1 and 2** and runs beside the rest of 5A and beside sub-project 6's 6A-1: it needs only `control.db` and the server-data layout (§6.8). 5A Tasks 1, 2 and 10 themselves can start now.
- **5C-2** waits for 5C-1 and for 5A's principals, handshake, chain and protocol 2 (5A Tasks 3–6).
- **5D** needs 5C-1; its `fin library scan` command (which returns a job id) needs 5C-2.
- **5B** is needed only by the frontend, so it comes last, but it touches only transport and auth files: it may run in parallel with 5C–5E on its own stacked branch. (5B keeps its letter because it completes 5A's auth work.)

### 1.5 What exists today (the baseline)

- **Transports** (`apps/finisherd/src/transport.ts`): Effect RPC over NDJSON on the Unix socket `<server-data>/fin.sock` (0600) or the named pipe `\\.\pipe\finisher-<username>[-<hash>]` created by libuv with the default DACL, and a WebSocket on `127.0.0.1:<random>` that requires `Authorization: Bearer <token>`. The token is random per start and written to `<server-data>/finisherd.json` (0600).
- **Trust:** anything that can open the socket is fully trusted. Anything that can read `finisherd.json` can use the WebSocket. D4 (not yet built) maps the transport to an event source (local socket → `fin`, WebSocket → `ui`), and only `ui` may opt in to training content.
- **Server data** (`config.ts` `ensureServerData`): created 0700, refused if a symlink or owned by another uid; nothing is checked on Windows.
- **Identity:** `<server-data>/server-id`, a lowercase ULID with clone detection (`fin-store::server_id`); it is reminted when server data turns up somewhere else.
- **`fin`** finds the socket through `FIN_SERVER` or `finisherd.json`, sends `headers: []` today, never calls `server.hello`, and exits 0/2/3/4/5/6.
- **Electron's Node is not the repository's Node** (Electron 44/45 embed Node 24.22.0; the repository pins 26.12.1): see §1.6 for sub-project 9.
- **`TASKS.md` "Follow-ups for sub-project 5":** Windows named-pipe DACL and pipe squatting, server-data ACL check, graceful stop without SIGTERM; decode failures and unknown tags as `FinError`; relative paths refused server-side; a client deadline and frame-size cap; test hooks out of release builds; `childEnv` provider variables (sub-project 7).

### 1.6 Cross-cutting notes for other sub-projects

- **Sub-project 6 (media):** the job system is 5C's (§6.0). Sub-project 6 contributes the `media` family inside `fin-workers`, its job specs, the mapped-file bulk channel (its own shared-memory crate) and `job.done` `kv` 2 with media fields; `fin look` uses a warm `interactive:media` worker (§6.3). It can build 6A-1 beside 5C-1 against the `JobEngine` API and the `test` family, and needs 5C-2 only for `fin` commands that return job ids.
- **Sub-project 7 (agent runtime):** owns `fin-sandbox` (profiles `agent`, `hook`, `worker`, `plugin`), which 5C's `Spawner` trait calls at every spawn; owns approvals behind `APPROVAL_REQUIRED` and the rules and hooks of the chain's policy and after stages (§4.4); mints `agent` credentials (§4.1); applies the sandbox reachability rules of §4.11.
- **Sub-project 8 (plugins):** contributes plugin job adapters, started by 5C as helpers or as a job's only process under the `plugin` profile (§6.4); `worker.exit` records them with a keyed plugin id. Generation providers add `externalRef` to jobs (cut from 5C, §1.2).
- **Sub-project 9 (frontend and Electron):** **Electron 44 and 45 embed Node 24.22.0, while the repository pins Node 26.12.1.** A `finisherd` started by the desktop shell as an `ELECTRON_RUN_AS_NODE` child therefore runs on Node 24, a Node the server is not tested on today. Checked on 2026-11-07 on Linux x64 with Electron 44.9.3 and 45.0.0: `node:sqlite` loads without an experimental warning, with SQLite 3.53.4, FTS5 and WAL. Sub-project 9 must either ship its own Node 26 for `finisherd` or add Node 24 to the test matrix; sub-project 5 uses only APIs present in both (§3.1). The same applies to `fin-node`'s napi build (it must load in both runtimes; arch §27.1 measured the Electron loads). Sub-project 9 also owns SSH/WSL routes, the device key in the keychain, and the onboarding question that is the only RPC route to training content (§3.4).

---

## 2. Principles

1. **Trust comes from a credential, never from a transport.** Loopback is not local (an SSH tunnel ends on loopback; so does a browser tab after DNS rebinding). A Unix socket is not the user (a sandboxed agent can still `connect()` to it). Every connection proves a credential; with none, it may only say hello.
2. **The client authenticates the server too,** and both sides bind the proof to the environment and the endpoint. A squatted pipe, a stale `finisherd.json` or a relay to another server never receives a request.
3. **Human-only is a property of the principal, enforced in one place,** before any handler runs. A request from `fin` (agent-only, D16) can never be human, whatever its headers claim. **Until sub-project 7's sandbox, this is advisory against code running in a raw shell as the user** (it can read key files); Owner question 6.
4. **No opt-in to training content from any command line.** The only routes stay D14b's: the onboarding question (sub-project 9), a hand edit of `privacy.training` in `settings.json`, and `FIN_TRAINING` in `finisherd`'s own environment.
5. **Error fixes address the user and never hand an agent a ready-to-run command** for a human-only operation.
6. **Least privilege by default, the status quo by explicit grant.** The `local` principal keeps what `fin` can do today, minus the human-only operations (Owner question 5).
7. **Checks happen against resolved, current state.** Branch names, directories and paths are resolved by the store before authorisation, and state-dependent rules (protected branches, an agent's branches) are checked again inside the store's write transaction.
8. **No secret where an agent or a sync service can read it.** Keys live in `<server-data>/auth/` (0700, local disk) or the OS keychain; the socket lives apart from them in `<server-data>/run/`, the only server-data path a sandbox may reach. Never in a project folder, never in the environment (arch §8.4), never in the journal.
9. **Decisions that change what a merge does are op-log facts.** A rejection is a record in the log (`drop`, §8.3), so it syncs, survives moves and rebuilds, and binds every later merge.
10. **Control-plane state is local, versioned and rebuildable where it can be** (arch §6.7, sub-project 4 §2.6).
11. **The diagnostics journal stays metadata-first.** New kinds carry only `meta` fields from closed tables. The security audit log is a separate store (§3.3).
12. **Every error is `{code, message, details, fix?}`** with a stable exit code in `fin`.

---

## 3. Shared foundations

### 3.1 The control store (`control.db`)

- `<server-data>/control.db`, SQLite through Node's built-in **`node:sqlite`** (`DatabaseSync`), mode 0600. `journal_mode = WAL`, `synchronous = NORMAL`, `foreign_keys = ON`, `busy_timeout = 250`, and **`wal_autocheckpoint = 0` on the main connection**: no checkpoint ever runs on the event loop.
- **Checkpoints** run on a `worker_thread` with its own connection: `PASSIVE` every 30 s or when the WAL passes 4 MiB, `TRUNCATE` after 60 s idle.
- **One writer:** `finisherd`'s main thread. Library catalogs are separate databases with their own writer (§7.1).
- **Event loop:** every statement is prepared once, indexed and small, with a budget of **p99 ≤ 1 ms** per statement (10k jobs, 100k audit rows, Linux VM). Bulk deletes (retention, purges) run in batches of ≤ 500 rows per statement, one batch per tick. Bulk inserts (catalog diffs) never run on the main thread.
- **Node versions:** only APIs present in both Node 24.22 (Electron 44/45) and Node 26.12 (§1.5).
- **Versioning:** `meta` holds `envId` (§3.7) and `createdAt`; `migrations {chain, n, appliedAt}` records what ran. **Migrations come in independent chains, one per area** (`core`, `auth`, `jobs`, `libraries`, `reviews`, `devices`), each a frozen ordered sequence (`control/migrations/<chain>/0001.ts`) with its own `formatVersion`/`minReaderVersion` in `meta`, and no foreign keys across chains. Batches that run in parallel (5A and 5C-1) therefore never fight over migration numbers. Pending migrations run in one transaction at start, after a `VACUUM INTO control.db.pre-migrate.bak` (the newest two kept). A chain whose `minReaderVersion` is above this code fails the start with `CONTROL_DB_TOO_NEW`.
- **Damage:** `PRAGMA quick_check` at open. A failure moves the files aside to `control.db.corrupt-<ulid>` and starts empty with an audit entry: the environment id and devices are lost (devices re-pair: fail closed), catalogs rescan, jobs are lost, open reviews are re-derived (§8.1). Rejections are not lost: they are in the op log (§8.3).

Tables per chain: `core` (5A Task 1) `meta`, `migrations`, `server_ids`, `audit`; `auth` (5A Task 9) `credentials`; `jobs` (5C-1) `jobs`; `devices` (5B) `devices`, `pair_offers`, `pair_pending`; `libraries` (5D) `libraries`; `reviews` (5E) `reviews`, `review_events`, `review_comments`, `branch_policy`.

### 3.2 Principals

```text
Principal {
  kind:     anonymous | local | console | device | agent | mcp | plugin
  id:       "local" | "console" | <deviceId> | <credential id> | <pluginId>   (ULIDs)
  scopes:   Scope[]                        (§4.5: the kind's defaults, or the credential's)
  projects: "*" | ProjectId[]              (agent, mcp; "*" otherwise)
  branches: "*" | BranchRule[]             (agent, mcp: branch ids or "own" = branches it created)
  human:    console | device               (derived; every device is the owner in v1)
  source:   Source                         (§3.4: derived, never sent)
  author:   string                         (the default Tx.author for its writes)
}
```

| Kind | Who | Proof | Batch |
|---|---|---|---|
| `anonymous` | any connection before a proof | none | 5A |
| `local` | `fin` and scripts run by the user; until sub-project 7, also agent harnesses the user starts in a raw shell | HMAC with `<server-data>/auth/local.key` (§4.2) | 5A |
| `console` | a person at the server: `finisherd <command>` (§4.7), and the desktop shell's main process | HMAC with `<server-data>/auth/console.key` | 5A |
| `device` | a paired client: the desktop renderer, a browser, later a phone; always the owner in v1 | DPoP-bound token → WebSocket ticket (§5) | 5B |
| `agent` | one agent session (sub-project 7, or minted from the console for a hand-run harness) | HMAC with the session key from its credential file | 5A |
| `mcp` | the MCP adapter (D16: a thin adapter over the command registry), one credential per adapter install | HMAC with the key from its credential file | 5A |
| `plugin` | one plugin process (sub-project 8) | reserved | 8 |

- The principal is attached to the **connection** and re-checked on every request, so revocation is immediate.
- `agent` and `mcp` are the non-human, credentialed kinds: they never hold a human-only scope, and the server, not a header, decides their actor (§3.4).

### 3.3 The audit log

- A security record for the owner (arch §16): rows in `control.db` `audit {id, at, kind, principalKind, principalId, credentialKind, outcome, code?, remote?, count, details JSON}`.
- **Kinds:** `auth.fail` (bad proof, bad ticket, bad DPoP proof, rate-limited), `auth.denied` (a human-only, scope, reach or policy denial: RPC, scope, principal kind), `auth.ticket`, `credential.mint|revoke|expire`, `pair.offer|redeem|approve|reject|expire`, `device.revoke|rename`, `consent.change`, `diagnostics.purge`, `settings.write`, `project.takeoverConfirmed`, `project.relocated`, `listener.start|stop` (with the SPKI pin), `env.moved|adopted|reset`, `library.add|remove|set`, `history.drop`, `review.merge` (protected targets), `console.refused` (a console command refused for lack of a TTY or a confirmation); reserved for 7/8: `grant`, `approval`, `hook.block`, `hook.modify`, `sandbox.violation`, `plugin.install`.
- **Aggregation:** `auth.fail` rows are aggregated **per credential kind (and remote address, in 5B) per minute**, with `count`, so a flood of connections cannot flood the log.
- **Always on.** Consent does not gate it, `DO_NOT_TRACK` does not stop it; it is never uploaded, never part of `fin diagnostics export`, never read by the dataset builder.
- **What it may hold** that the journal may not: device names and, for pairing and failed authentication, remote addresses (Owner question 4). Never secrets, tokens, MACs, nonces, links, paths or request payloads.
- **Retention:** 180 days or 100k rows, swept hourly in batches. Readable and purgeable only by the owner (`auth.audit.list`, `auth.audit.purge`).
- **Never blocks:** a failed audit write is logged to stderr and counted in `server.status`; the request still gets its answer (denials still deny).

### 3.4 Diagnostics journal integration

- **Source from the principal** (replaces D4's per-transport `RpcSource`): `local` and `agent` → `fin`; `mcp` → **`mcp`**; `device` → `ui`; `console` → **`console`**; `anonymous` → nothing is accepted. `mcp` and `console` are new `fin_core::events::Source` variants (5A). A WebSocket opened with a `local` ticket (the e2e subscriber) is `fin`, never `ui`.
- **Actor from the principal, narrowed by headers.** D6's `fin-actor-*` headers are kept, but bounded: `agent` and `mcp` principals always get `{kind: agent, session: <their credential id>}`, and only `call` comes from the header; `device` gets `{kind: user, session: <deviceId>}`; `console` gets `{kind: user}`; `local` keeps D6's unverified header actor.
- **Consent.** Every consent change (`diagnostics.consent`, any purpose, any value) needs a human principal, and **turning training on is refused for every principal but a `device`** (the onboarding question, sub-project 9): the console answers `CONSENT_UI_ONLY` (exit 2) like `fin` does, with D14b's fix (the app, a hand edit of `settings.json`, or `FIN_TRAINING`, described in words). The other human-only diagnostics operations move from "source is `ui`" to "principal is human" (§4.6).
- **The D4 rate limit** becomes per principal (`local`, `console`, each device, each credential).
- **New kinds** (all `meta` only, all `source: finisherd`, names from new closed tables in `fin_core::events::names`):

| Kind | Batch | Payload |
|---|---|---|
| `auth.pair` | 5B | `outcome: offered\|redeemed\|approved\|rejected\|expired\|revoked`, `platform: Name(PLATFORMS)`, `route: loopback\|lan\|tailscale\|public\|connect\|other`, `micros?` (redeem → approve) |
| `job.done` | 5C-1 | `kv` 1: `kind: Name(JOB_KINDS)`, `family: Name(WORKER_FAMILIES)`, `outcome: succeeded\|failed\|cancelled\|interrupted`, `code?`, `attempts`, `priority`, `warm` (ran on a warm worker), `queuedMs`, `runMicros`, `items?`, `itemsFailed?`, `isolated`, `helpers` (count). Sub-project 6 adds its media fields as `kv` 2 |
| `worker.exit` | 5C-1 | one per process 5C started: `role: worker\|helper`, `family: Name(WORKER_FAMILIES)`, `helperKind?: codec\|plugin`, `plugin?: Hashed` (the plugin id, keyed), `reason: done\|idleExpired\|recycled\|cancelled\|killed\|crashed\|hung\|stalled\|abandoned\|spawnFailed`, `exitCode?`, `signal?: Name(SIGNALS)`, `jobsServed`, `lifetimeS`, `sandboxed: bool` |
| `library.scan` | 5D | `storage: local\|network\|sync`, `full`, `trigger: add\|schedule\|demand\|watch\|mount`, `outcome`, `code?`, `files` and `dirs` (log₂ buckets), `added`, `changed`, `missing`, `moved`, `placeholders`, `micros` |
| `review.verdict` | 5E | fills the reserved v0 kind as v1: `itemKind`, `verdict: accepted\|rejected\|merged\|dismissed\|changesRequested`, `txIds: IdRef[]` (≤ 64), `accepted`, `rejected`, `by: user\|agent\|critic`, `openS`; training only: `reason: Content<userText>` |

- Denials are not a kind: `rpc.stats` already counts error codes per method.
- **Never in the journal:** device and environment names, routes, addresses, library names and paths, file names, review titles and comments outside `Content`.

### 3.5 Exit code 7: denied; fix texts

- `fin` gains one exit class (`crates/fin/src/output.rs`): **7 = denied**, "this needs a different principal, usually a person". It covers `UNAUTHENTICATED`, `SERVER_UNTRUSTED`, `SCOPE_DENIED`, `HUMAN_ONLY`, `BRANCH_PROTECTED`, `BRANCH_NOT_ALLOWED`, `PATH_NOT_ALLOWED`, `CREDENTIAL_REVOKED`, `APPROVAL_REQUIRED`. An agent must not retry these.
- **Fix texts address the user, and a human-only fix never contains a runnable command line.** For example `HUMAN_ONLY` on a purge: "Only you, the owner of this Finisher server, can delete recorded diagnostics: use the Finisher app or the server's console." `details.where` is `app | console | settingsFile | env`. A test checks every human-only code's `fix` against the pattern of a command line (`finisherd <word>`, `fin <word>`, backticks).

### 3.6 Protocol 2

5A bumps `PROTOCOL_VERSION` to **2**: the handshake (§4.2), tickets (§4.3) and the new `asset.probe {projectId}` (§4.5) are not compatible with protocol-1 clients, and every client lives in this repository and ships version-locked (arch §21). Protocol 2 also takes sub-project 4's pending changes: `tx.commit.generation` and `ops.subscribe.generation` become required, and the `project.recover` result change of 4B is recorded. The WebSocket upgrade refuses another version with **HTTP 426** and `{protocol: {min, max}}` (arch §16).

### 3.7 The environment id

- **A separate `envId`** (a ULID) is minted at the first start and stored in `control.db` `meta`. It is **not** the server id: the server id is reminted by `fin-store::server_id` when server data moves, and moving server data must not change the environment.
- **Bound to the identity key** (5B): the pairing link pins `(envId, spki)`, and `env.describe` returns both. Before 5B there is no identity key, and the id is bound to `control.db` alone.
- **`server_ids {serverId, envId, since}`** maps every server id this server data has had to the environment, so a lease holder named by server id (`project.status` `holder.serverId`) maps to an environment: `project.status` gains `holder.envId` when the holder is this environment, and `env.describe` lists `serverIds` so a client can map another environment's lease.
- **Copies and moves** (§5.8): a moved server data keeps its `envId`; a copy that runs beside its original must get a new one (`env reset`), because two servers holding one identity key could impersonate each other.

---

## 4. 5A — Principals, local credentials and scopes

### 4.1 Data model and files

- **Server data layout:**
  - `<server-data>/auth/` (0700; owner-only DACL on Windows): `local.key`, `console.key` (32 random bytes, base64url, 0600), **rewritten at every start** before any listener opens; `credential.master` (created once, kept).
  - `<server-data>/run/` (0700): **only** the Unix socket `fin.sock`. A sandbox may be allowed to reach this directory and nothing else of server data (§4.11). Windows uses the named pipe and needs no `run/` entry.
  - `finisherd.json` (0600) in server data, as today, without a token (§4.3).
- **`credentials`** (agent and MCP sessions): `{id ULID, kind: agent|mcp, label, scopes JSON, projects JSON, branches JSON, createdBy, createdAt, lastUsedAt, idleExpiry, absoluteExpiry, revokedAt?}`. A credential's key is never stored: it is `HMAC(credential.master, "fin/credential-key/v1" ‖ kind ‖ id)`, recomputed to verify. Revocation is `revokedAt`.
- **Expiry defaults: 24 hours idle, 7 days absolute.** *Why:* an agent cooking a library overnight is idle between turns for minutes, not a day, so 24 h idle never cuts a working session, and it ends credentials a harness forgot; 7 days caps how long a copied credential file works. Sub-project 7 mints a fresh credential per session, so neither limit is felt there. `mcp` credentials (one per adapter install, long-lived by nature) default to 30 days idle and no absolute limit; the owner revokes them. `lastUsedAt` is written at most once a minute.
- **The credential file** (written by sub-project 7 inside the session sandbox, or by `finisherd agent mint` for a hand-run harness, 0600): `{"v":1,"kind":"agent"|"mcp","id":"<ulid>","key":"<base64url>","env":"<envId>"}`. `fin` and the MCP adapter find it through **`FIN_CREDENTIAL`**, a path. The variable names a file, not a secret, so arch §8.4's env allow-list gains it (amended with this revision).

### 4.2 The handshake

```text
fin                                                    finisherd
 │ server.hello {clientNonce, env?, endpoint,
 │               credential: {kind: local|console} | {kind: agent|mcp, id}}
 │ ─────────────────────────────────────────────────────▶
 │   {protocolVersion, serverVersion, capabilities, envId, endpoint (canonical), serverNonce,
 │    proof = HMAC(K, "finisherd/hello/v2" ‖ envId ‖ endpoint ‖ clientNonce ‖ serverNonce)}
 │ ◀─────────────────────────────────────────────────────
 │ check envId and endpoint against what it connected to; verify proof (constant time)
 │ auth.prove {mac = HMAC(K, "fin/prove/v2" ‖ envId ‖ endpoint ‖ serverNonce ‖ clientNonce)}
 │ ─────────────────────────────────────────────────────▶
 │   {principal}                                         (the connection now carries it)
 │ ◀─────────────────────────────────────────────────────
```

- `K` is the key of the credential named in the hello. Nonces are 32 random bytes; HMAC-SHA-256; every label is domain-separated and length-prefixed fields (`u16` big-endian length before each variable-length field) keep concatenations unambiguous.
- **Binding:** `envId` (from `finisherd.json`, or the credential file's `env`) and the canonical `endpoint` (the socket's real path, or the pipe name lower-cased) go into both MACs, so a proof made for one server or one socket is useless on another, and a relay between two servers fails.
- **Why a server proof:** a pipe name can be squatted, and a stale `finisherd.json` can name a dead socket that someone else re-created. Only a process that can read the key can produce the proof.
- **A restart is not an attack.** On a wrong proof (`badProof`) or an environment mismatch, `fin` re-reads `finisherd.json` and the key file **once**, reconnects and retries; a restart between reading the key and connecting is then invisible. Only a second failure is **`SERVER_UNTRUSTED`** (exit 7) with `details.reason` `noProof | badProof | envMismatch | endpointMismatch`; the first failure is recorded as `keyStale` in the retry's details if the retry succeeds through a changed key.
- **Unknown, expired or revoked credential, wrong MAC:** `UNAUTHENTICATED` (exit 7), audited. The hello answers with a `serverNonce` and no proof for an unknown credential, so ids cannot be probed. Five failures on one connection close it.
- **Before the proof** a connection may call only `server.hello` and `auth.prove`.
- **Cost:** one round trip and four HMACs per `fin` run; budget ≤ 1 ms added to `fin`'s p50 on the Linux VM.

### 4.3 Transports, connections and tickets

- **Connection identity comes from Effect RPC.** The server's RPC protocol reports a `clientId` per connection and passes it to every handler in its options; `connections.ts` maps it to a `Connection {clientId, transport, principal, hello?, failures, peer?}`. Payload decoding stays at the protocol level, where Effect already does it; `finisherd` maps decode failures and unknown tags to `INVALID_REQUEST` there, so no handler sees an undecoded payload and no defect frame is sent.
- **Unix socket:** `<server-data>/run/fin.sock`, created under umask 0177 and chmod 0600 as today.
- **Windows: a native named-pipe listener in `fin-node`** (decided now). `finisherd` no longer lets libuv create the pipe. `fin-node`'s `PipeServer` creates every instance with its own security descriptor (`D:P(A;;GA;;;<user SID>)(A;;GA;;;SY)`; sub-project 7 adds per-project AppContainer SIDs with read/write), `FILE_FLAG_FIRST_PIPE_INSTANCE` on the first instance (so a squatter makes the start fail, `SERVER_RUNNING`/`PIPE_SQUATTED`), and `PIPE_REJECT_REMOTE_CLIENTS`. It accepts on its own thread and feeds each connection to `protocol.ts` as a duplex stream, with the client's process id and token user SID (`GetNamedPipeClientProcessId`, `OpenProcessToken`) recorded as `peer` for the audit log. A client whose user SID is neither ours nor an allowed AppContainer SID is closed before its first frame. The handshake still runs: the DACL and the proof are two layers.
- **WebSocket:** a `ws` `WebSocketServer` in `noServer` mode behind a `node:http` server on `127.0.0.1:0`, so the upgrade request is ours.
  - **Tickets** replace the bearer token. `auth.ticket {ttlS ≤ 60}` (any authenticated principal) returns a 256-bit one-time ticket bound to the caller's principal. The client sends it in **`Sec-WebSocket-Protocol: finisher.v2, finisher.ticket.<ticket>`**, not in the URL (query strings end up in proxy and server logs); the server echoes `finisher.v2`. The WebSocket then carries the ticket's principal. Devices get tickets through DPoP in 5B; in 5A any authenticated local principal may mint them (the e2e subscriber).
  - **Host allow-list:** `Host` must be one of `127.0.0.1:<port>`, `localhost:<port>`, `[::1]:<port>` (5B adds the route names and the `publicUrl` host); otherwise 403. **Origin:** absent (not a browser), or one of the server's own origins, the desktop shell's origin (sub-project 9) or a configured one; otherwise 403. Both checks run before the ticket is consumed.
- **`finisherd.json`** keeps `pid`, `wsPort`, `socket`, `protocolVersion` and adds `envId` and `diagnostics.enabled` (D4). The `token` key is gone.
- **Frames** over 16 MiB on either transport end the connection after an `Exit` with `FRAME_TOO_LARGE`. Any client request header with the reserved `x-fin-` prefix is dropped.

### 4.4 The middleware chain

Every request passes the same stages, in order, in one place (`chain.ts`), for unary and stream RPCs alike:

1. **Decode** (protocol): `INVALID_REQUEST` on failure.
2. **Authenticate:** the connection's principal; anything but `public` RPCs from `anonymous` is `UNAUTHENTICATED`.
3. **Resolve:** each RPC declares a resolver that turns references into current facts *before* any check:
   - **branch references** (`branch`, `from`, `into`, a name, an id or `name#id`) become branch ids **through the store** (`engine.resolveBranch`), so a name, an alias or a rename can never slip past a rule written for an id;
   - **directories** (`project.create.dir`, `project.open.dir`) become `{projectId?, canonical dir, parent}` by reading `finisher.project.json` (open) or canonicalising the parent (create);
   - **paths** (`asset.probe.path`, `project.recover.importDir`) become canonical absolute paths (symlinks resolved; `INVALID_PATH` for relative or non-normalised input);
   - the RPC's **project** (from its payload or from the resolved directory).
4. **Authorise:** every scope the RPC requires (**all of them**, §4.5), the human-only rules (§4.6), and **reach**: the project must be in `principal.projects`, and every resolved path must be inside an allowed root for principals that have roots (`agent`, `mcp`, `device`: the project folders they may use, registered library roots, the session workspace). Failures: `SCOPE_DENIED`, `HUMAN_ONLY`, `PROJECT_NOT_FOUND` (not `SCOPE_DENIED`, so ids don't leak), `PATH_NOT_ALLOWED`.
5. **Policy:** rules that see the resolved request and answer `allow | deny | ask`. 5A ships the stage with one rule, **agent branch limits** (a credential's `branches`: a write to a branch outside them is `BRANCH_NOT_ALLOWED`); 5E adds protected branches; sub-project 7 adds command-pattern rules (arch §8.3) and `PreCommand` hooks here. **`ask`** answers `APPROVAL_REQUIRED {approvalId}` (exit 7); sub-project 7 owns approvals and the approval cards, and 5A tests the outcome with a test rule.
6. **Handler,** with the resolved values, never the raw ones. **State-dependent rules are checked again inside the store's write transaction:** every write call into `fin-node` carries a `guard {denyBranches: [branchId], allowBranches?: [branchId]}`, and `fin-store` refuses (`BRANCH_PROTECTED` / `BRANCH_NOT_ALLOWED`) if the resolved target is denied at the moment of writing. A rename, a branch made in between or a racing policy change cannot open a window.
7. **After** (sub-project 7): `PostCommand` hooks.

A denial at stages 2–6 is audited as `auth.denied` with the RPC tag, the scope or rule and the principal kind, never the payload.

### 4.5 Scopes

A closed vocabulary in `@finisher/contracts` (`scopes.ts`). **Every RPC declares the scopes it needs (one or more, all required)** and its resolver; a test fails when an RPC has none.

| Scope | Covers | Human-only |
|---|---|---|
| `public` | `server.hello`, `auth.prove` | — |
| `server.read` | `auth.ticket`, `auth.whoami`, `auth.revokeSelf`, `env.describe`, `fonts.catalog`, `fonts.status`, `server.status` | — |
| `project.read` | `snapshot.get`, `ops.subscribe`, `project.events`, `project.status`, `project.verify`, `tx.get`, `branch.list`, `eval.*`, `fonts.plan`, **`project.open` of a project already open here** (attaching); 5E `review.list/get` | — |
| `project.create` | `project.create` | — |
| `project.write` | **`project.open` that loads a project** (it may take the lease), `asset.probe {projectId, path}` (a protocol-2 field: the probe is for that project), `tx.commit`, `branch.create`, `branch.set`, `history.*`, `project.acknowledge*` | — |
| `project.admin` | `project.rebuild`, `project.compact`, `project.recover`, `project.takeover` | — |
| `project.confirm` | escalation: `project.takeover {confirm: true}`, `project.open {relocate: true}` | **human** |
| `fonts.fetch` | `fonts.fetch` (network downloads from the pinned catalog) | — |
| `branch.protected` | escalation (5E): a write whose resolved target is protected | — |
| `review.write`, `review.decide` | 5E (§8.6) | — |
| `jobs.read`, `jobs.submit`, `jobs.control` | 5C-2 (§6.7) | — |
| `library.read`, `library.scan` | 5D (§7.6) | — |
| `library.admin` | `library.add`, `library.remove`, `library.set` of hydration or bandwidth | **human** |
| `diagnostics.read` | `diagnostics.status`, `diagnostics.export` (content redacted, below), `diagnostics.project {collect: never}` | — |
| `diagnostics.record` | `diagnostics.record` (it writes the journal) | — |
| `diagnostics.admin` | `diagnostics.consent`, `diagnostics.purge`, `diagnostics.project {collect: default}`, escalation: `diagnostics.export {content: true}` | **human** (training on: `device` only, §3.4) |
| `settings.admin` | `settings.write` (5B) | **human** |
| `auth.admin` | credentials (mint, list, revoke), pairing and devices (5B), `auth.audit.*`, `server.stop`, `env.*` writes | **human** |
| `plugin.install` | reserved (sub-project 8, arch §8.3) | **human** by default |

**Default scopes per principal:**

| Principal | Scopes |
|---|---|
| `anonymous` | `public` |
| `console`, `device` | all (training on still `device` only) |
| `local` | all but the human-only rows; it keeps `branch.protected` and `review.decide` (Owner question 5) |
| `agent`, `mcp` | exactly the credential's scopes, never a human-only one (a mint that asks for one is refused). The console's mint defaults to `server.read`, `project.read`, `project.write`, `review.write`, `jobs.read`, `jobs.submit`, `library.read`, `library.scan`, `diagnostics.read`, `diagnostics.record`; never `branch.protected`, `review.decide`, `jobs.control`, `project.admin`, `project.create` or `fonts.fetch` unless asked for |

- **Two scopes where an RPC does two things:** `jobs.submit` of a `library.scan` job needs `jobs.submit` **and** `library.scan` (the kind's scope, found by the resolver); `review.merge` into a protected branch needs `review.decide` only (reviewing is the sanctioned way in, §8.5); `history.drop` into a protected branch needs `review.decide`.
- **Non-human `diagnostics.export` is redacted, not refused:** content fields and blob references are removed, as the upload view does for diagnostics-only, and the result says `redacted: true`.
- **`auth.revokeSelf {}`** revokes the caller's own credential (an agent, MCP or device credential) and closes its connections; it takes no id, so it can never name another principal.

### 4.6 Human-only operations

These are refused with **`HUMAN_ONLY`** (exit 7) for every non-human principal, at stage 4, with a fix that addresses the user (§3.5).

| Operation | Why |
|---|---|
| any consent change; **training on only from a `device`** (onboarding) | arch §21.1, D14b |
| export of recorded content | it is the user's content |
| `diagnostics.purge` (every selector) | destroys data |
| `diagnostics.project {collect: default}` (setting `never` stays open) | clearing a never-collect flag binds collaborators |
| `project.takeover {confirm: true}` | arch §6.7: "the user confirms" |
| `project.open {relocate: true}` | "a person's decision; never set it automatically" (4A) |
| `library.add`, `library.remove`, `library.set` of hydration or bandwidth | grants read access to a tree; downloads |
| credential mint and revoke (beyond `revokeSelf`), pairing, devices, audit read and purge, `server.stop`, environment commands | account takeover vectors |
| settings writes through RPC (5B) | opens the server to the network |
| plugin install (sub-project 8 default) | arch §8.3 |

**Before sub-project 7's sandbox, this binds agents that use `fin` or MCP, not code in a raw shell** that can read `console.key` exactly as it can hand-edit `settings.json` (Owner question 6). The mechanism is complete in 5A; sub-project 7 makes it hold by denying agents read access to `<server-data>/auth/`.

### 4.7 The console (`finisherd <command>`)

- `finisherd` alone (or `finisherd serve`) serves, as today. Any other first word is a console command that connects to the running server with `console.key`, runs the handshake as `console` and prints text for a person (`--json` for read-only commands).
- **Read-only and service commands run anywhere:** `status`, `audit` (listing), `stop` (`server.stop`, the graceful stop Windows lacks; service managers need it non-interactively, and a same-user process can kill the server anyway).
- **Every mutating human-only command needs an interactive terminal and a typed confirmation.** stdin and stdout must be TTYs (`CONSOLE_NEEDS_TTY`, exit 7, otherwise); the command shows what it will do, asks for `console.challenge` (a fresh 6-character code the server issues to this connection, valid 60 s), and the person types the code back. The server refuses the mutating RPC from a `console` principal without a valid challenge answer (`CONFIRMATION_REQUIRED`). This keeps `yes |` pipes and pasted scripts out; it is not a boundary against same-user code driving a pseudo-terminal before sub-project 7 (Owner question 6).
- **Training cannot be turned on from the console** (§3.4): `finisherd diagnostics consent training on` answers `CONSENT_UI_ONLY` (exit 2).
- **5A commands:** `status`, `stop`, `audit [--since] [--kind] [--limit]`, `audit purge`, `diagnostics consent <diagnostics|training> <off|unset>` and `diagnostics on|off|unset`, `diagnostics purge`, `diagnostics project default <id>`, `diagnostics export --content`, `project takeover <dir|id> --confirm`, `project open <dir> --relocate`, `agent mint|ls|revoke`, `mcp setup` (mints an `mcp` credential and writes it to the path the person names). 5B adds `pair`, `devices`, `revoke`, `remote …`, `tls show|rotate`, `env show|name|adopt|reset`; 5D adds `library add|rm|set`.
- **Output** is English, like `fin` (arch §17.1); errors print `finisherd: <CODE>: <message>` and the fix, with `fin`'s exit codes.

### 4.8 RPCs and `fin` (5A)

| RPC | Scopes | Payload → result |
|---|---|---|
| `server.hello` (changed) | `public` | `{clientNonce?, env?, endpoint?, credential?}` → adds `envId`, `endpoint`, `serverNonce`, `proof?` |
| `auth.prove` | `public` | `{mac}` → `{principal}` |
| `auth.ticket` | `server.read` | `{ttlS?}` → `{ticket, expiresAt}` |
| `auth.whoami`, `auth.revokeSelf` | `server.read` | → `{principal}` / `{}` |
| `auth.credential.mint` | `auth.admin` | `{kind: agent\|mcp, label, scopes?, projects?, branches?, idleS?, absoluteS?}` → `{credential, file}` (the file contents; shown once) |
| `auth.credential.list`, `auth.credential.revoke` | `auth.admin` | — |
| `auth.audit.list`, `auth.audit.purge` | `auth.admin` | `{since?, kind?, cursor?, limit ≤ 1000}` → `{entries, next?}` |
| `console.challenge` | `auth.admin` | `{}` → `{code, expiresAt}` (console principals only) |
| `server.status` | `server.read` | → `{version, protocol, envId, listeners, connections: {kind: count}, journal, auditFailures}` |
| `server.stop` | `auth.admin` | → `{}`, then a graceful shutdown |

`fin` changes: the handshake in a new `crates/fin/src/auth.rs` (key discovery: `FIN_CREDENTIAL`, else `<server-data>/auth/local.key`; the retry of §4.2); exit class 7; `fin whoami`; `fin asset add` and `fin asset probe` send `projectId`.

### 4.9 Storage

`<server-data>/auth/{local.key, console.key, credential.master}`, `<server-data>/run/fin.sock`; `control.db` tables `meta`, `server_ids`, `audit`, `credentials`. Nothing in project folders.

### 4.10 Security and threat model (5A)

Arch §16 priority: injected agent > malicious plugin > stolen device > network attacker.

| Threat | Mitigation | Residual |
|---|---|---|
| Injected agent runs `fin diagnostics consent training on`, `purge --all`, `takeover --confirm` | `HUMAN_ONLY` for every non-human principal at stage 4; training on is device-only | a raw-shell agent before sub-project 7 can read `console.key` (Owner question 6) |
| Injected agent is handed a fix text and runs it | fixes never contain a runnable command (§3.5); console mutations need a TTY and a typed challenge | a PTY-driving agent before sub-project 7 |
| Injected agent opts the user in to training through the console | refused: `CONSENT_UI_ONLY` from every principal but a device | — |
| Injected agent reads `finisherd.json` to become `ui` | no token in the file; a `local` ticket gives `local` | — |
| Another OS user squats the pipe, or opens it | native listener: first-instance flag, explicit DACL, remote clients rejected, peer SID checked; the server proof | denial of service only |
| Proof relayed to another server or socket | envId and canonical endpoint inside both MACs | — |
| Sandboxed agent `connect()`s to the socket | no proof without a key → `UNAUTHENTICATED`; its credential gives only its scopes | — |
| A rule written for a branch id bypassed by a name, alias or rename | references resolved by the store before checks; guards re-checked in the write transaction | — |
| A client claims another actor | the principal bounds the actor (§3.4); `x-fin-` headers stripped | `local` can still claim `agent` or `user` (as in D6) |
| Audit flood | aggregation per credential kind per minute; 5 failed proofs close a connection | — |

### 4.11 Cross-platform (5A)

- **Windows server data:** `ensureServerData` calls `fin-node` `checkPrivateDir(path)`: the owner is the current user, and the DACL allows nothing to `Everyone`, `Users`, `Authenticated Users` or another user (SYSTEM and Administrators may stay). A directory `finisherd` creates gets a protected owner-only DACL; an existing one that fails is `SERVER_DATA_UNSAFE`.
- **Windows pipe:** the native listener above; graceful stop through `finisherd stop`.
- **Key files** are written temp-then-rename inside the private directory (the temp file inherits its DACL); rename-over retries sharing violations.
- **Sandbox reachability** (sub-project 7 applies these; 5A fixes the layout they depend on):
  - **macOS Seatbelt:** `(allow network-outbound (remote unix-socket (path-literal "<server-data>/run/fin.sock")))` plus `file-read-metadata` on the path's ancestors; everything else under server data denied;
  - **Linux:** Landlock at the ABI spike S8 used (4) does not mediate `connect()` on pathname sockets, so the socket is reachable as long as the path resolves; under the bwrap fallback, `--bind <server-data>/run <server-data>/run` and nothing else of server data;
  - **Windows AppContainer:** the pipe's DACL grants the project's AppContainer SID (`finisher-<project-uuid>`) read/write through `PipeServer.allow(sid)`; `auth/` stays owner-only, which an AppContainer cannot read;
  - in every sandbox, `fin` proves with `FIN_CREDENTIAL` (a file inside the sandbox), never with `local.key`.
- **Clocks:** nothing in 5A compares clocks; TTLs use the server's monotonic clock.

### 4.12 Journal (5A)

`console` and `mcp` sources; principal-derived source and actor; human-only diagnostics operations; device-only training opt-in; per-principal rate limit; new `RPC_METHODS` entries for every new RPC.

### 4.13 Error codes (5A)

| Exit | Codes |
|---|---|
| 2 | `FRAME_TOO_LARGE`, `CONSENT_UI_ONLY` (now also from the console) |
| 3 | `INVALID_REQUEST`, `INVALID_PATH` |
| 5 | `CONTROL_DB_BUSY` |
| 7 | `UNAUTHENTICATED`, `SERVER_UNTRUSTED`, `SCOPE_DENIED`, `HUMAN_ONLY`, `PATH_NOT_ALLOWED`, `BRANCH_NOT_ALLOWED`, `CREDENTIAL_REVOKED`, `APPROVAL_REQUIRED`, `CONSOLE_NEEDS_TTY`, `CONFIRMATION_REQUIRED` |
| start (stderr, exit 1) | `CONTROL_DB_TOO_NEW`, `SERVER_DATA_UNSAFE` (now also on Windows), `PIPE_SQUATTED` |

### 4.14 Testing (5A)

- **Shared vectors:** `tests/fixtures/auth/handshake.json` (fake keys, nonces, envId, endpoint → proofs and MACs), read by a vitest and a Rust test.
- **Unit (vitest):** every RPC has scopes and a resolver; the human-only matrix over every principal kind and escalation; training on refused for `console`, `local`, `agent`, `mcp`; the chain's order; resolvers (branch names, renames between resolve and write, directories, symlinked paths); `ask`; ticket one-time use, TTL, `Sec-WebSocket-Protocol`; Host and Origin; fix texts carry no command; control store (migrations, `CONTROL_DB_TOO_NEW`, corruption, no autocheckpoint on the main connection, batched deletes, the 1 ms budget); audit aggregation.
- **Rust (`fin`, `fin-node`, `fin-store`):** `SERVER_UNTRUSTED` against a fake server; the restart retry (`keyStale`); `FIN_CREDENTIAL`; exit 7; the write guard refuses inside the transaction; the Windows pipe listener (first instance, remote rejection, peer SID, DACL).
- **e2e `auth` scenario** (Task 12 of the plan).
- **Windows:** a second user cannot reach the pipe or squat it unnoticed; `SERVER_DATA_UNSAFE`; `finisherd stop`; all as a non-elevated user.

---

## 5. 5B — Pairing, devices, remote access and environments

### 5.1 Data model

- `devices {id ULID, name (user text), platform (Name), jwk (public, P-256), jkt (RFC 7638 thumbprint), createdAt, approvedBy, lastSeenAt, lastRoute, revokedAt?}`. Every device is the owner (§1.2).
- `pair_offers {id ULID, secretHash (SHA-256), label?, createdBy, createdAt, expiresAt (≤ 10 min), state: open|redeemed|expired}`; `pair_pending {id, offerId, deviceName, platform, jwk, jkt, safetyCode, redeemedAt, remote, expiresAt, state}`.
- **Environment descriptor** `{envId, name, spki, serverIds, protocol {min, max}, routes: [{kind, url}], capabilities}`.
- **Identity key:** one P-256 key per environment, stored with the `envId` it is bound to. It is the TLS key, so clients pin the key (SPKI).
- Tokens and tickets are **opaque random strings held in memory**, mapped to `{deviceId, jkt, expiresAt}`. No JWT is issued.

### 5.2 Pairing

1. **Offer** (owner): `auth.pair.offer {label?, ttlS ≤ 600}` → `{offerId, link, qr, expiresAt}`. Link: `finisher://pair?v=1&env=<envId>&spki=<pin>&r=<base64url(routes)>#<offerId>.<secret>` (256-bit secret in the fragment, arch §16); for browsers, `https://<route>/pair#…` on a route with a trusted certificate (§5.5). The console's `finisherd pair` prints the link and a terminal QR code and waits.
2. **Redeem:** the device makes a P-256 key pair (non-extractable where the platform allows), then `POST /pair/redeem {offerId, secret, device: {name, platform, jwk}}` over the pinned route. The offer is consumed at the first valid redeem. Answer: `{pendingId, safetyCode}` (6 digits, `HMAC(secret, jkt) mod 10⁶`). Redeems are limited to 10 per minute per remote address and 30 per minute per server.
3. **Approve:** the owner compares the device name and safety code and approves (`auth.pair.approve`) or rejects, on the console or an existing device; a pending request expires 10 minutes after the redeem. The one exception: the desktop shell pairs its own renderer with a console offer flagged `self`, redeemed over loopback only, approved at redeem.
4. **Finish:** the device polls `POST /pair/status` with a DPoP proof made with its key.

### 5.3 Tokens, DPoP and tickets

- `POST /auth/token` with a DPoP proof (RFC 9449: `typ dpop+jwt`, `alg ES256`, `jwk`; claims `jti`, `htm`, `htu`, `iat`, `nonce`) signed by a known device key → `{accessToken, tokenType: "DPoP", expiresIn: 900}`. `POST /auth/ws-ticket` with `Authorization: DPoP <token>` and a proof carrying `ath` → `{ticket, expiresIn: 30}`; then the upgrade of §4.3 over TLS.
- **`htu`** is compared with the URL the client used, which behind a reverse proxy is not the URL the server sees: the server accepts `htu` values whose origin is one of its route URLs or `server.remote.publicUrl`, with the request's path.
- **Freshness from server nonces** (`DPoP-Nonce`, rotated every 5 minutes, the previous one still valid); `iat` is only sanity-checked (±7 days), because client clocks are off by seconds for days (arch §27.9); `jti` remembered for the nonce lifetime.
- **JOSE:** `jose` 6 (MIT, no dependencies) for JWS and thumbprints.

### 5.4 Devices

`auth.device.list|rename|revoke`, `auth.devices.watch` (stream), and `auth.revokeSelf` (§4.5). Revocation forgets the device's tokens and tickets and closes its live connections at once.

### 5.5 Listeners and TLS

- **Local by default (D6).** Remote listening is off until the owner enables it on the console (`finisherd remote enable [--listen <host:port>]`, TTY-gated), which writes `settings.json` `server.remote {enabled, listen, tls: {mode: selfSigned|files, certFile?, keyFile?}, publicUrl?, origins?, hosts?}`; `FIN_LISTEN` does the same for Docker.
- **A non-loopback listener is TLS only** (`LISTEN_NEEDS_TLS` at start otherwise), with a **Host allow-list** of its route names, `publicUrl`'s host and `server.remote.hosts`.
- **Self-signed mode:** the certificate is made by `rcgen` (in `fin-node`, `ring` provider) from the identity key, with the current addresses and names as SANs, and remade with the same key when they change, so the pin never changes.
- **Pinning** is for the desktop shell and native clients. **Browsers cannot pin:** they reach a remote server only over a route whose certificate they trust (owner-supplied files, a reverse proxy) or over loopback through an SSH tunnel (Owner question 3).
- **Rotation is re-pairing** (signed rotation statements are cut, §1.2): `finisherd tls rotate` makes a new identity key (TTY-gated), keeps the `envId`, and every device pairs again.
- **Secret store** (`fin-node`): `keyring` 3 pinned exactly in `Cargo.toml` (`=3.x.y`, recorded in the 5B plan) with `default-features = false` and `apple-native`, `windows-native`, `async-secret-service`, `async-io`, `crypto-rust` (the pure-Rust `zbus` Secret Service, also pinned; never the `dbus`/libdbus backend). **When the Secret Service is absent, or present but locked** (no unlock prompt is possible for a server), the key goes to the file store and `server.status` says so; the server never prompts and never blocks its start. File store: `<server-data>/secrets/identity.pem` (0600), optionally wrapped with AES-256-GCM under a key from `FIN_SECRETS_KEY_FILE` (a Docker secret).
- **Handshake** (arch §16): `GET /.well-known/finisher` (anonymous) → `{envId, protocol, spki}` only.

### 5.6 Environments and routes

- **Routes** are discovered at start and on address change: `loopback`; `lan` (non-internal interfaces, IPv6 link-local excluded); `tailscale` (addresses in the Tailscale ranges); `public` (`publicUrl`); `connect` (the cloud's `ConnectRelay`, an open extension point). SSH routes are the desktop shell's: through a tunnel the server looks like loopback, which is why loopback grants nothing.
- `env.describe` (`server.read`) → the descriptor; `env.name` (owner) sets the name.
- **"Sync" of environments** (arch §16) is read as: the client keeps its list of environments (sub-project 9); projects move between environments by folder sync and the lease (4A); nothing syncs control-plane state between servers in v1.

### 5.7 Server filesystem reach for remote principals

A `device` (like `agent` and `mcp`, §4.4) may name server paths only inside **allowed roots**: registered library roots, `settings.json` `server.projectRoots` (default `<home>/Finisher`) and the folders of projects open here. `project.create`, `project.open`, `asset.probe` and `library.add` outside them → `PATH_NOT_ALLOWED`. `local` and `console` keep full reach.

### 5.8 Copied or moved server data

When `fin-store::server_id` remints the server id because server data turned up elsewhere, the new server id is added to `server_ids` under the **same `envId`**, remote listeners stay off and every device is refused (`ENV_MOVED`) until the owner runs, on the console, `finisherd env adopt` (it was a move: keep the envId, key and devices) or `finisherd env reset` (it is a copy: new envId and identity key, devices dropped).

### 5.9 RPCs, HTTP and console (5B)

| Surface | Scope / proof |
|---|---|
| `GET /.well-known/finisher` | anonymous |
| `POST /pair/redeem`, `POST /pair/status` | anonymous, rate-limited / DPoP |
| `POST /auth/token`, `POST /auth/ws-ticket` | DPoP |
| WebSocket upgrade (`Sec-WebSocket-Protocol` ticket) | ticket; 426 on a protocol mismatch |
| `auth.pair.offer|approve|reject`, `auth.pair.pending` (stream), `auth.device.list|rename|revoke`, `auth.devices.watch` (stream) | `auth.admin` |
| `env.describe`, `env.name` | `server.read` / `auth.admin` |
| `settings.write {path, value}` (`server.remote`, `server.projectRoots`) | `settings.admin` |
| console | `pair`, `devices`, `revoke <id>`, `remote enable|disable|status`, `tls show|rotate`, `env show|name|adopt|reset` |

### 5.10 Security and threat model (5B)

| Threat | Mitigation |
|---|---|
| Leaked pairing link | one-time, ≤ 10 min, consumed at redeem; owner approval with name and safety code; the legitimate redeem fails visibly (`PAIRING_USED`) |
| Stolen device | revoke from another device or the console; live connections closed; non-extractable keys; 15-minute tokens |
| Stolen access token | DPoP-bound; tickets one-time and 30 s |
| Network attacker | TLS only off loopback; SPKI pin from the link; no trust on first use |
| Clock skew | server nonces; server-side TTLs |
| CSRF / DNS rebinding | Host allow-list and Origin checks; no cookies; every change needs a proof |
| Remote device reads server files | allowed roots |
| Impersonation after a server-data copy | §5.8 |
| Injected agent pairs an attacker's device | offers and approvals are human-only and TTY-gated; `fin` has no pairing command |

### 5.11 Error codes, cross-platform, journal, tests (5B)

- **Codes:** HTTP-mirrored `FinError` bodies `PAIRING_EXPIRED`, `PAIRING_USED`, `PAIRING_REJECTED`, `PAIRING_RATE_LIMITED`, `DPOP_INVALID`, `DPOP_NONCE`, `TOKEN_EXPIRED`, `TICKET_INVALID`, `ORIGIN_REFUSED`, `HOST_REFUSED`; exit 7 `DEVICE_REVOKED`, `PATH_NOT_ALLOWED`, `ENV_MOVED`; start `LISTEN_NEEDS_TLS`, `TLS_FILES_INVALID`; client side (9) `TLS_PIN_MISMATCH`.
- **Cross-platform:** first non-loopback listen triggers the macOS and Windows firewall prompts (expected: that is when the owner enabled remote access); macOS prompts unsigned development builds on keychain access, so development uses `FIN_SECRETS=file`; Windows Credential Manager blobs are ≤ 2,560 bytes (a P-256 PEM fits); Linux without a running or unlocked Secret Service and every headless install use the file store; Docker containers need `publicUrl` or Tailscale inside the container for routes.
- **Journal:** `auth.pair` events (§3.4).
- **Tests:** the pairing state machine; the DPoP verifier against RFC 9449 vectors, nonces, `jti` replay, `ath`, `htu` behind a proxy; revocation closes a live subscription within 100 ms; TLS with a pinned SPKI and a mismatched pin; `LISTEN_NEEDS_TLS`; allowed roots; `ENV_MOVED` and `adopt` keeping the envId; a locked keyring falls back to the file store; an e2e `remote` scenario with a client on another host (addresses from the test run's environment, never in the repo).

---

## 6. 5C — Jobs

### 6.0 Ownership (cross-design reconciliation, 2026-11-07)

**5C owns the shared job system**, and no other sub-project builds a second one:
- the job model, the scheduler and its pools, persistence and restart;
- **one worker binary, `fin-workers <family>`**, and the **`fin-jobs`** crate with the protocol types and the envelope;
- spawning and the kill tree, retries, poison handling and isolation (bisection);
- warm workers for short interactive jobs (§6.3);
- helper processes started beside a job's worker (§6.4) and the lifecycle of the job's mapped files;
- the `jobs.*` RPCs, `fin job …` and `fin wait`;
- the diagnostics kinds **`job.done`** and **`worker.exit`** (every process 5C starts, plugin job processes included).

**Contributions:**
- **Sub-project 6** contributes only the **`media` family** (its job specs and the family's code inside `fin-workers`), the **mapped-file bulk channel** (a shared-memory crate that sub-project 6 owns; 5C only creates, sizes, hands over and deletes the files), and the **media fields of `job.done`** (as `kv` 2 of the kind, §3.4).
- **Sub-project 7** owns sandboxing (`fin-sandbox`, profiles `agent`, `hook`, `worker`, `plugin`); 5C calls it at every spawn (§6.2).
- **Sub-project 8** contributes **plugin job adapters**: a job kind whose work is done by a plugin process, started by 5C as a helper (§6.4) or as the job's process, under the `plugin` profile.
- **5D** contributes the `library` family (§7).

### 6.1 Data model

```text
Job {
  id: ULID, kind: Name (JOB_KINDS), kindVersion, family: Name (WORKER_FAMILIES),
  params: JSON (validated by the kind), owner: {kind, id}, project?: ProjectId, library?: LibraryId,
  priority: interactive | normal | background,
  state: queued | running | paused | succeeded | failed | cancelled | interrupted,
  attempts, maxAttempts, idemKey? (per owner), dedupeKey? (per kind),
  progress: {done, total?, unit: bytes|items|frames|seconds, phase?: Name, at},
  checkpoint?: JSON (resumable kinds), result?: JSON, error?: FinError, itemErrors?: [...],
  files?: [MappedFile], helpers?: [HelperSpec],
  parent?: JobId, createdAt, startedAt?, finishedAt?, nextRunAt?
}
MappedFile { name, bytes, mode: read | write | readWrite }          (created by 5C, format owned by the family)
HelperSpec { kind: codec | plugin, id, profile: worker | plugin, files: [name] }
```

- **Kinds** are declared in `finisherd` (`jobs/kinds/*.ts`): `{kind, version, family, scopes, params (Rust-generated schema), resources, warm?: bool, timeoutS, stallS, maxAttempts, resumable, batch?, dedupe(params), defaultPriority, qos, files?, helpers?}`. 5C ships the `test` family (behind `test-hooks`: `sleep|echo|crash|hang|stall|spawn|warm`); 5D the `library` family; sub-project 6 `media`; sub-project 8 plugin adapters.
- **Ownership:** a job belongs to its owner principal (`local` runs share the owner `local`; each credential and device is its own owner; in-process callers inside `finisherd` use the owner `system`). Owners see and control their own jobs; `jobs.control` sees and controls all; jobs on projects or libraries outside a principal's reach are invisible to it.

### 6.2 The worker binary, the envelope and spawning

- **`fin-workers <family>`** is the one worker binary (arch §7's `fin-workers`), one subcommand per family; it is found beside the `fin-node` binary and versioned with it. **Workers never start processes**: anything a job needs beside its worker (a codec backend, a plugin) is a helper that 5C starts (§6.4). `fin-sandbox`'s `worker` profile denies process creation once it exists; until then a test-only check and code review hold the rule.
- **The envelope** (`fin-jobs`, Rust → JSON Schema → generated TS): newline-delimited JSON on the worker's stdin/stdout, `{"v":1,"t":<type>,"seq":n,"job"?:<id>,…}`; stderr is a log stream. The worker opens with `hello {v, family, kinds, warm}`; a `v` the host does not speak is `WORKER_PROTOCOL_MISMATCH`.
  - host → worker: `start {job: {id, kind, params, checkpoint?}, limits, files: [{name, path}], helpers: [{id, files}]}`, `cancel {job}`, `shutdown`;
  - worker → host: `begin {job, item}`, `item {job, item, value}`, `progress`, `checkpoint {data}`, `alive`, `result {job, value}`, `error {job, error, itemErrors?}`, `ready` (a warm worker is free again, §6.3).
  - Bulk data never travels in the envelope: it goes through the job's mapped files, in the format of the family's channel (sub-project 6's crate for `media`).
- **Spawning lives in `fin-node`** (`jobhost`), not in `child_process`: it starts the process with an explicit environment (the `childEnv` allow-list computed by `finisherd`, arch §27.7; nothing inherited), frames the envelope, runs the liveness timers and owns the kill tree. The scheduler, persistence and RPCs stay in TypeScript (D24). *Why in Rust:* the process must enter its Job Object before it runs, and `fin-sandbox` (a Rust crate) must wrap the spawn.
- **Sandboxing:** every spawn goes through a `Spawner` trait in `jobhost`. Its implementation is **`fin-sandbox`** (sub-project 7) with the `worker` profile for workers and codec helpers and the `plugin` profile for plugin processes. **Until `fin-sandbox` exists, 5C spawns unsandboxed with the env allow-list only**, and `server.status` says so.
- **The kill tree, owned by the host, on every OS:**
  - **Windows:** one host-owned Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, created at start, and a nested Job per job; every process (worker and helpers) is created **suspended**, assigned to its job, then resumed, so nothing runs outside it. Cancel is `TerminateJobObject` on the job's Job; if `finisherd` dies, the host job's handle closes and everything dies;
  - **Linux:** each job's processes share a process group, and each sets `PR_SET_PDEATHSIG` = SIGKILL at start (exiting if `getppid()` already changed); where cgroup v2 is delegated to `finisherd` (a systemd user service, a container), each job gets a child cgroup, killed with `cgroup.kill`; otherwise the host kills `-pgid`;
  - **macOS:** process groups; the host kills `-pgid` with SIGKILL;
  - **everywhere:** a worker or helper exits when its stdin reaches EOF (the host is gone).
- **Cancel** sends `cancel`, waits 2 s (500 ms for interactive jobs), then kills the job's tree.
- **Liveness and progress are separate:** silence for 30 s (`alive` every 5 s) is `WORKER_HUNG`; no `progress` or `item` for the kind's `stallS` (default 10 minutes) is `JOB_STALLED`. A process stuck in uninterruptible I/O may not die; the host abandons it and, after 2 abandoned processes on a resource, stops dispatching there (`RESOURCE_STUCK`).
- **QoS:** library scans and probes at **utility** QoS on macOS and `nice 5` on Linux; CPU-heavy background kinds at background QoS, `nice 10`, `BELOW_NORMAL_PRIORITY_CLASS`. Interactive jobs run at normal priority.
- **Bandwidth:** a per-root byte budget is enforced in the worker by a token bucket on reads.

### 6.3 Scheduling and warm workers

- **Pools:** `cpu` (cores − 1, at least 1), `gpu` (1; sub-project 6), `io:<rootKey>` per library root (network and sync: 1 scan + 2 probe jobs; local: 4), `net` (4; sub-project 8), and **`interactive:<family>`** (warm workers, below). A job runs when every pool it needs has a slot.
- **Cold jobs** (the default): one worker process per job; small work is batched (a `library.probe` job takes up to 256 files).
- **Warm workers** (new requirement, for sub-project 6's `fin look` ≤ 300 ms): kinds marked `warm` with priority `interactive` run on a **kept-alive worker of their family** that serves short jobs back to back:
  - the `interactive:<family>` pool has its own slots (default 1 worker per family, at most 2), so a long cold job never delays an interactive one, and a warm worker uses a `cpu` slot only while it runs a job;
  - a warm worker announces `ready` after each job and gets the next `start`; dispatch to a ready worker is a write on its stdin (host-side budget ≤ 5 ms from submit to `start`; the 300 ms end-to-end budget is sub-project 6's);
  - **idle expiry:** a warm worker that has been idle for **60 s** (per-family setting) is shut down; it is also replaced after **500 jobs** or **30 minutes**, whichever comes first, to bound leaks;
  - **no state between jobs** except caches keyed by content hash (a family contract, tested by running the same job on a fresh and a reused worker and comparing results);
  - each job on a warm worker gets its own nested Job / cgroup for its helpers; the worker itself lives in a per-worker one;
  - a crash or kill during a job loses the worker: the job is retried once on a fresh worker (cold start), and the pool starts a replacement on the next interactive submit;
  - pre-warming: the pool starts a worker at the first interactive submit of a family and keeps it under the expiry rules; `jobs.warm {family}` (internal, used by sub-project 6 when a project opens) starts one ahead.
- **Order:** priority, then FIFO, with aging (10 minutes waiting counts one level higher). The scheduler is a pure function `schedule(queue, pools, workers, now) → dispatches`, property-tested.
- **Pause:** a job, or everything (`jobs.pause {job} | {all}`). Pausing stops dispatch; running resumable jobs checkpoint and stop; others finish; warm workers are kept until they expire.
- **Limits:** 50,000 queued jobs (`JOB_QUEUE_FULL`); finished jobs kept 7 days or 10,000 rows.
- **Idempotent submit:** the same `idemKey` from the same owner returns the existing job; the same `dedupeKey` while queued or running returns that job.

### 6.4 Helpers and mapped files

- **Mapped files:** a kind may declare files; 5C creates them in `<server-data>/jobs/<jobId>/` (0700 directory, 0600 files; local disk), sizes them, passes their paths to the worker and its helpers in `start`, and deletes the directory when the job ends (also after a crash, at the next start). Their layout is the family's (sub-project 6's shared-memory crate for `media`); 5C never reads them.
- **Helpers** (codec backends, plugins) are **siblings of the worker, started by 5C**, never by the worker:
  - started before the worker's `start`, inside the job's kill tree, each through `fin-sandbox` with its profile (`worker` for codecs, `plugin` for plugins);
  - connected to the worker **only through the job's mapped files**; each has its own envelope channel to the host (`hello`, `alive`, `error`), not to the worker;
  - a helper that exits early fails the job with `HELPER_FAILED {helper, code}`; all helpers are killed with the job;
  - warm workers keep no helpers between jobs: helpers are per job.
- Sub-project 8's plugin job adapters are helpers (or, for a job that is all plugin, the job's only process) under the `plugin` profile.

### 6.5 Persistence, retries and isolation

- Jobs live in `control.db` (the `jobs` migration chain, §3.1); state transitions are written at once, progress only in memory, checkpoints at most every 10 s. Warm-worker state is in memory only.
- **Restart:** `running` jobs become `interrupted`; resumable ones with attempts left are re-queued from their checkpoint; others fail `JOB_INTERRUPTED`. Leftover `jobs/<jobId>/` directories are deleted.
- **Retries** with backoff 5 s × 2ⁿ (≤ 5 min), up to `maxAttempts` (default 3); interactive jobs retry once, at once, on a fresh worker.
- **Poison handling and isolation of a crashing batch:**
  1. items the worker already reported (`item`) are never re-run;
  2. the item that was **in flight** (`begin` without `item`) is retried **alone** first; if it crashes again it fails as **`POISON_ITEM`** and the rest re-run without it;
  3. only when no item was in flight (a crash between items) is the remainder split in halves;
  4. **never bisected:** `WORKER_HUNG`, `JOB_STALLED`, `HELPER_FAILED`, I/O errors and an offline root. Those are resource problems: retry with backoff, or the library goes offline (§7.2).

### 6.6 Events and hooks

- An in-process `JobDone` bus (sub-project 7's hooks attach to it; arch §10) and `jobs.watch` streams.
- **`job.done`** for every job, and **`worker.exit`** for every process 5C started (§3.4). Sub-project 6 adds its media fields to `job.done` as the kind's `kv` 2.

### 6.7 RPCs and `fin` (5C-2)

| RPC | Scopes |
|---|---|
| `jobs.submit {kind, params, priority?, idemKey?, project?}` | `jobs.submit` **and** the kind's scopes (e.g. `library.scan`), plus reach over its project or library |
| `jobs.get`, `jobs.list {filter, cursor?, limit ≤ 500}`, `jobs.wait {ids, any?, timeoutMs ≤ 600000}`, `jobs.watch` (stream) | `jobs.read`; own jobs, or all with `jobs.control` |
| `jobs.cancel`, `jobs.pause {job}`, `jobs.resume {job}` | `jobs.submit` for own jobs, `jobs.control` for others' |
| `jobs.pause {all}`, `jobs.resume {all}`, `jobs.workers` (warm pools and live processes) | `jobs.control` |

`fin job ls|show|cancel|pause|resume`, `fin wait <id>… [--any] [--timeout <s>]` (exit 0 when all succeeded, 4 `JOB_FAILED`, 5 `WAIT_TIMEOUT`). Commands that start jobs print `{job}` and return at once (arch §9.9). Inside `finisherd`, other services (5D, sub-project 6's `look` handler) use the engine's TypeScript API (`JobEngine.submit(owner, spec)`) directly; their own RPCs are authorised by the chain (§4.4).

### 6.8 Split and what 5C needs from 5A

- **5C-1, the engine:** `fin-jobs`, `fin-workers` with the `test` family, `fin-node` `jobhost` (spawner trait, kill tree, envelope framing, timers), the TypeScript scheduler with pools and warm workers, persistence, restart, retries and isolation, mapped files and helpers, the `job.done`/`worker.exit` kinds in `fin-core::events`, and the `JobEngine` API. **It needs only 5A Task 1** (`control.db`, its migration chains and the audit log) **and Task 2** (`<server-data>` layout and private directories, for `jobs/`). It can therefore start right after them and run beside sub-project 6's 6A-1. Until diagnostics D4 wires the journal into `finisherd`, events go to a `JobEvents` sink that tests read; D4's journal becomes the sink when it lands. Owners are recorded (`system` for in-process callers) but not yet checked.
- **5C-2, the surface:** the `jobs.*` RPCs with their scopes, ownership and reach checks, `jobs.watch`, `fin job …` and `fin wait`. **It needs 5A Tasks 3–6** (principals, the `fin` handshake, the chain and scopes, protocol 2).
- What 5C never needs from 5A: credentials, the console, tickets (beyond what the chain gives), the native pipe listener.

### 6.9 Security, codes, cross-platform, tests (5C)

- **Security:** workers and helpers get the cleared environment and no credential, speak only to their host over their own stdio, never start processes, and never receive a path outside the job's project, library root or job directory. Sandboxing is `fin-sandbox`'s (sub-project 7), applied at every spawn once it exists.
- **Codes:** 3 `JOB_KIND_UNKNOWN`, `JOB_PARAMS_INVALID`, `JOB_NOT_FOUND`; 4 `JOB_FAILED`, `JOB_CANCELLED`, `JOB_INTERRUPTED`; 5 `JOB_QUEUE_FULL`, `WORKER_UNAVAILABLE`, `WORKER_PROTOCOL_MISMATCH`, `WAIT_TIMEOUT`, `RESOURCE_STUCK`; in results: `WORKER_CRASHED`, `WORKER_HUNG`, `JOB_STALLED`, `JOB_TIMEOUT`, `POISON_ITEM`, `HELPER_FAILED`.
- **Cross-platform:** the kill tree above; signals do not exist on Windows (cancel is the envelope message, then the job kill); a killed process's open file on SMB causes transient sharing violations for the next attempt; mapped files on local disk only.
- **Tests:**
  - scheduler properties (pools never over-committed, priority and aging, warm pool separation);
  - a `test.spawn` worker (test-only: it breaks the no-processes rule on purpose) whose double-forked grandchild is gone after cancel and after `finisherd` is killed with SIGKILL / `TerminateProcess`, on all three OSes; and a check that the shipped families never call a process-spawning API;
  - `test.crash` with a known poison item is isolated in two runs; `test.hang` and helper failures are never bisected; `test.stall` hits `JOB_STALLED`;
  - warm workers: 100 back-to-back `test.warm` jobs on one process with p99 dispatch ≤ 5 ms; idle expiry and the 500-job/30-minute replacement with an injected clock; a crash mid-job retries on a fresh worker; fresh-vs-reused results equal;
  - helpers start before the worker, die with the job, and only see their mapped files; mapped-file directories are gone after the job and after a crash;
  - restart resumes from the checkpoint; `fin wait` exit codes (5C-2); `job.done` and `worker.exit` entries.


### 6.10 Interface additions from the 5C-1 plan (revision 2, for sub-projects 7 and 8)

The 5C-1 plan (`docs/superpowers/plans/2026-11-07-sub-project-5c1-job-engine.md`, revision 2) adds these to §6.2–6.5; its Task 13 folds them into those sections as built:
- **Host calls:** worker → host `call {id, method, params}`, host → worker `reply {id, value | error}`, routed to a family's (or helper's) `onCall` handler with the job's owner, `via` and rights. Sub-project 8 serves its host API (`project.snapshot`, `media.pcm`, `net.fetch`, `job.submit`, ring `slot`/`release`) this way; a plugin process speaks only the envelope.
- **Versions:** bundled processes must match the envelope's schema hash exactly; plugin processes (`profile: plugin`) need only the major `v` and announce a `minor`; an unknown message type above the host's minor gets an error reply, not a kill.
- **Warm keys:** `FamilySpec.warmKey(job)`; a warm worker serves only jobs of its key (sub-project 8: plugin, project, invoker class).
- **Warmness comes from the engine:** a family declares one (cold) argv; the engine spawns a warm process by appending `--warm` to it (`fin-workers media --warm`; a plugin process's `<path> <args…> --warm`). A family's own code (`Family::warm()` in `fin-jobs`) only declares that it can serve warm jobs. Sub-project 6's `media` family and sub-project 8's plugin SDK must accept a trailing `--warm`.
- **Kill domains:** every process gets a leaf inside its job's group (Windows host → group → leaf Job; a leaf cgroup where delegated) carrying `PlatformCommand.job_limits` (active processes, UI limits) for sub-project 7's profiles. Memory limits apply per group on every OS.
- **Self-sandbox entry:** `fin_jobs::worker::run` and `helper::run` call a function registered by `fin-sandbox` before `hello` (seccomp, Seatbelt applied by the process itself after exec); a requested self-sandbox that cannot be applied fails closed.
- **`onSucceeded` is at least once:** the result and `phase: hooks` are persisted before the hook; after a crash the job directory is kept and the hook runs again, so hooks must be idempotent.
- **Scheduling, as built (5C-1 batch A):** a warm dispatch may take `cpu` beyond the pool's size by at most the sum of every warm family's slots; within one scheduling pass, dispatches come first and warm spawns and retirements after, and a ready worker whose key has a due job is never retired. **Multi-slot jobs can starve:** the scheduler reserves nothing, so a job that needs several slots at once (two `cpu`, or `gpu` + `hwenc`) waits while single-slot jobs keep taking the slots it needs as they free, aging notwithstanding (a blocked job is passed over, not waited for). Accepted for 5C-1; a reservation for the oldest blocked job is the fix if a kind needs it.
- **Deviation from §6.9:** without a delegated cgroup (macOS; Linux in a terminal), a grandchild that calls `setsid` escapes the process-group kill after a cancel and after host death; Windows Job Objects and delegated cgroups have no such gap.

---

## 7. 5D — Library indexer

### 7.1 Data model

- `libraries` (in `control.db`): `{id ULID, name (user text), root (absolute server path), rootKey (canonical; case-folded on case-insensitive volumes), volume {devId, fsId?, mounted: bool}, storage: local|network|sync (from `fin-store::storage_kind`, with override), state: online|offline|scanning|paused, include?, exclude, kinds, scan {poll: demand|<seconds>, full: daily|weekly|manual}, hydrate: false, bandwidthMiBs?, stages, createdBy, createdAt}`.
- **Volume identity:** at add, the library stores the root's device id and filesystem id (`st_dev` and, where available, the filesystem UUID: `statfs` `f_fsid` on macOS and Linux, the volume serial number on Windows) and whether the root is a **mount point** (its `st_dev` differs from its parent's). A root that was a mount point and whose `st_dev` now **equals its parent's** is an **unmounted share**: the empty mount-point directory on the local disk, not the library. It goes offline (§7.2) and is never scanned.
- **Libraries are per server** (arch §20) and read-only to Finisher: nothing is ever written under a root.
- **Catalog** `<server-data>/libraries/<id>/catalog.db` (`node:sqlite`, WAL, owned by one indexer `worker_thread`): `dirs {id, rel, mtimeNs, placeholder, lastScan}`; `files {id, dirId, name (raw bytes) and nameNfc, relKey, size, mtimeNs, fileId (dev:ino, or the 128-bit NTFS/ReFS/SMB file id), placeholder, fp (fp1), fullHash?, kind, probe JSON, state: present|missing|cloudOnly|unreadable|probeFailed|skipped, code?, firstSeen, lastSeen, lastChange}`; `stages {fp, stage, version, state, code?, at}`; `names` (FTS5); `scans`.
- **Default excludes:** dot-files and directories, OS junk (`Thumbs.db`, `.DS_Store`, `@eaDir`, `#recycle`, `$RECYCLE.BIN`, `System Volume Information`), sync conflict copies by the patterns `fin-store` knows, and inside any Finisher project under a root everything but `sources/`.
- **Overlap rules:** a root must be an existing absolute directory, not inside server data, not equal to, inside or containing another library's root (`LIBRARY_OVERLAP`).

### 7.2 Scanning

- **Full scan** (on add, when a mount comes back, on the schedule, on demand): the worker walks the root with parallel metadata reads (32 on network roots, as measured in spike S10; 64 locally) and **never opens a file to enumerate it**:
  - **Windows:** directories are read with `GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)` (`FILE_ID_EXTD_DIR_INFO`), which returns size, times, attributes and the 128-bit file id for every entry from the directory handle alone; reparse points and junctions are skipped;
  - **macOS:** `getattrlistbulk` with the common flags, so dataless files and directories are seen without touching them; the worker sets `setiopolicy_np(IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, IOPOL_SCOPE_PROCESS, IOPOL_MATERIALIZE_DATALESS_FILES_OFF)`, so any accidental access to a dataless item fails (`EDEADLK`) instead of downloading it;
  - **Linux:** `readdir` + `statx`; symlinks are never followed.
- **The indexer thread** diffs each batch against the catalog: new and changed files (size, mtime or file id) are queued for `library.probe`; a vanished file id that reappears elsewhere is a **rename**; a new file whose fingerprint matches a missing one is a **move**; files not seen by a completed full scan become `missing`.
- **Incremental scans:**
  - **network roots: demand-driven polling by default** (`poll: demand`). The worker re-stats known directories and rescans those whose mtime changed **every 60 s while the library is in use** (a project that references it is open, a `library.watch` stream is open, or it was searched, listed or scanned in the last 10 minutes), and **every 6 hours otherwise**. Before answering a search or listing of a library whose last poll is older than 5 minutes, the indexer polls the touched directories first (bounded to 5 s; the answer says `stale: true` if it ran out). *Why:* NAS hibernation (Synology, QNAP, TrueNAS) is defeated by any request that misses the NAS's caches, so a fixed 60-second poll keeps the disks spinning all day for libraries nobody is using, while an agent or person using the library still sees changes within a minute. The owner can set a fixed interval per library. In-place rewrites (a directory's mtime does not change) wait for the next full scan (daily). On macOS shares, listings are refreshed first as `fin-store` does (the smbfs 60 s directory cache);
  - **local roots:** `fs.watch` (recursive) is a **hint** that queues a targeted rescan after 2 s of quiet; watch errors (inotify limits, Windows buffer overflow) fall back to polling every 5 minutes and say so (`WATCH_DEGRADED`); correctness comes from scans;
  - **sync roots:** polled every 10 minutes and fully scanned daily.
- **An unavailable root never marks files missing.** A root that cannot be reached, an unmounted share (§7.1), or a full scan that finds the root empty while the catalog holds files puts the library `offline` (`LIBRARY_OFFLINE` / `ROOT_EMPTY_SUSPECT`) with its catalog and analysis intact (arch §6.7), re-checked every 60 s.
- **Spin-up-tolerant timeouts:** a sleeping NAS disk takes 10–30 s (sometimes more) to answer its first request. The first `stat` of a root waits up to **60 s** before the library counts as offline, and per-entry metadata reads up to 30 s; a scan that is slow but still progressing is never offline. The job's `stallS` for scans is 10 minutes.

### 7.3 Fingerprint, probe, placeholders and stages

- `library.probe` jobs compute `fp1` and run `fin-probe` in one pass, in the worker (untrusted bytes), with the isolation rules of §6.5.
- **The fingerprint stays `fp1`** (size, mtime and sampled BLAKE3, as `fin-probe` computes it for assets), so a library file and its asset share it; relinking across machines falls back to the full hash (arch §20), computed by `library.hash` on demand, never in a sweep.
- **Placeholders are catalogued from metadata and never opened or read** (opening or reading makes the provider download them):
  - macOS: `SF_DATALESS` on files **and directories** (a dataless directory is not descended into: its listing would be fetched; it is recorded `cloudOnly` as a whole); legacy `.<name>.icloud` stubs;
  - Windows: `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`, `FILE_ATTRIBUTE_RECALL_ON_OPEN`, `FILE_ATTRIBUTE_OFFLINE` and `FILE_ATTRIBUTE_PINNED`/`UNPINNED` as reported by `FILE_ID_EXTD_DIR_INFO`;
  - **Nextcloud and ownCloud virtual files** (suffix mode, on every OS): `<name>.nextcloud` and `<name>.owncloud` are placeholders for `<name>`; the stub itself is never probed;
  - Linux FUSE clients without markers: treated by the storage kind (`sync`), probed only within the bandwidth budget.

  Such files are `cloudOnly`; `hydrate: true` (human-only) lets probe jobs read them within the bandwidth budget.
- **Stages** are a framework (`{name, version, appliesTo, dependsOn, jobKind, batch}`); sub-project 5 registers only `probe`. A new stage version re-queues only that stage. Analysis artifacts are keyed by fingerprint in `<server-data>/cache/`.

### 7.4 Search and lookup

- `library.search {query?, libraries?, kind?, duration?, width?, height?, since?, cursor?, limit ≤ 200}`: FTS5 over names plus probe metadata filters (sub-project 8 adds semantic and transcript matches).
- `library.lookup {fingerprints | hashes}` → matches across this server's libraries (relinking).

### 7.5 Security and privacy

- `library.add` is human-only: a root grants read access to a tree through search and listings. Agents list, search and rescan registered libraries.
- Library and file names are user content, kept in the catalog on local disk; never journaled or logged at info level.
- `library.files`/`library.file` take relative paths inside the root; `..`, absolute paths and paths that canonicalise outside it are `INVALID_PATH`.

### 7.6 RPCs and `fin` (5D)

| RPC | Scopes |
|---|---|
| `library.add`, `library.remove`, `library.set` (hydration, bandwidth) | `library.admin` (human) |
| `library.set` (name, excludes, poll, schedule) | `library.scan` |
| `library.list`, `library.status`, `library.files`, `library.file`, `library.search`, `library.lookup`, `library.watch` (stream) | `library.read` |
| `library.scan {id, full?}` | `library.scan` (→ `{job}`, which also needs `jobs.submit`) |

`fin library ls|status|scan|files|show|search`, `fin search <query>`; the console adds `library add|rm|set`.

### 7.7 Codes, cross-platform, tests (5D)

- **Codes:** 3 `LIBRARY_ROOT_INVALID`, `LIBRARY_OVERLAP`, `LIBRARY_NOT_FOUND`, `FILE_NOT_FOUND`, `INVALID_PATH`; 4 `FILE_CLOUD_ONLY`; 5 `LIBRARY_OFFLINE`, `LIBRARY_ROOT_UNREADABLE`; status warnings `ROOT_EMPTY_SUSPECT`, `ROOT_UNMOUNTED`, `WATCH_DEGRADED`, `RESOURCE_STUCK`.
- **Cross-platform:** names (NFD on macOS → NFC keys; case-folded keys on case-insensitive volumes, detected per root without writing; raw bytes kept for opening); Windows long paths (`\\?\`); the enumeration APIs and placeholder rules above; SMB/NFS directory-mtime semantics; inotify limits; FSEvents coalescing.
- **Tests:** a generated 10k-file tree with renames, moves, in-place rewrites, deletions and placeholder markers (faked through a test hook on each OS, plus Nextcloud suffix files); an unmounted mount point (bind mount on Linux, a disk image on macOS) goes offline and marks nothing missing; a scan never opens a placeholder (an `open` counter in the test hook); a poison file isolated; demand polling cadence with an injected clock; a 45 s first-stat delay (injected) does not mark the library offline; on the Linux VM and macOS, a network-root scan against the 4A test share (paths from the environment).

---

## 8. 5E — Review workflow

### 8.1 Data model

- `reviews {id ULID, projectId, kind: branchReview | mergeConflict (later qaFinding, criticSummary, pluginSuggestion), state, source, target (branch ids), snapshot {sourceSeq, targetSeq, plannedAt}, title, summary?, requestedBy {principal, actor}, createdAt, updatedAt, closedAt?, closedBy?, mergeId?}`.
- **States:** `open` ↔ `changesRequested` → `merged` | `rejected` | `dismissed` | `superseded`.
- **Change list** (derived at the snapshot, cached): one entry per merge candidate, as `history.merge {dryRun: true}` plans it at `snapshot`: `{txId (origin root), sourceSeq, record kind, author, actor?, ops, at, decision: pending|accept|reject, decidedBy?, status: willMerge|alreadyPresent|dropped|excluded|dependsOnSkipped|conflict}`, plus `conflicts`.
- `review_events`, `review_comments {id, itemId, principal, actor, body, at?: rational time, txId?, createdAt}`.
- **Where review items live:** `control.db` of the server that holds the project (Owner question 1). **Rejections are not review state:** they are `drop` records in the op log (§8.3), so they are never lost. What a move loses (in the recommended answer) is open items' comments and their not-yet-merged accept marks; open `branchReview` items for active branches are re-derived.
- `branch_policy {projectId, protected: [branchId], requireReview}`, default `protected: [main]`, `requireReview: false`.

### 8.2 Workflow

1. **Request.** `review.request {projectId, source, into?: main, title, summary?}` opens a `branchReview` and plans its change list at the current heads (the snapshot). Dry runs work on followers (4B Decision 26).
2. **Refresh on demand** (live re-planning is cut, §1.2): `review.refresh` re-plans at the current heads and moves the snapshot; `review.get` reports `stale: true` when either head moved since. Decisions are keyed by origin txId and survive refreshes.
3. **Decide.** `review.decide {itemId, decisions: [{txId, decision, reason?}], expect: snapshot}`. `expect` must equal the item's snapshot (`REVIEW_STALE` otherwise, with the fresh change list). **A rejection writes a `drop` record at once** (§8.3), so it is durable whatever happens to the item.
4. **Merge.** `review.merge {itemId, expect: snapshot, acceptPending?: bool, idemKey}`:
   - the merge is limited to what was reviewed: `history.merge {from, into, include: [accepted origins], sourceUpTo: snapshot.sourceSeq}` (§8.4); with `acceptPending: true`, pending entries **of the snapshot** are added to `include`. Nothing outside the snapshot is ever included, so a commit that lands on the source after the review is never merged by it;
   - with `requireReview`, pending entries refuse the merge (`REVIEW_UNDECIDED`), whatever `acceptPending` says;
   - conflicts (`REVIEW_HAS_CONFLICTS`) are resolved by rejecting the conflicting entries or by fixing the source branch and refreshing;
   - afterwards: state `merged`, `mergeId` stored, and the source archived when it was an agent branch and nothing of it is left to merge.
5. **Reject all, request changes, dismiss:** `review.close {itemId, state: rejected|dismissed}` (rejecting all writes one `drop` for every pending entry); `review.decide` with `changesRequested` hands the branch back to its agent (sub-project 7 delivers it as a turn).

### 8.3 Durable rejections: the `drop` record

A **history record**, not a primitive op (it has no ops; it is planned and written by `fin-store`'s history layer like `checkpoint`).

- **Kind `drop`** on the **target** branch's log, with fields **`from`** (the source branch id) and **`drops`** and/or **`undrops`** (origin root txIds; at least one non-empty). The field-order list of sub-project 4 §4.2 is extended at its end: `…, recovers, drops, undrops`. `intent.ops == []`.
- **Meaning:** an origin root is **dropped for T** when the newest `drop` record on T's lineage naming it lists it in `drops` (not `undrops`). Branches forked from T after the record inherit it, as they inherit every record before their fork point.
- **Honoured by:**
  - **merge candidate selection** into T: a dropped origin is not a candidate; it is reported in `skipped` with reason **`DROPPED`**; anything that depends on it is `DEPENDS_ON_SKIPPED` under the existing rules (sub-project 4 §4.4). A later plain `fin merge` therefore never brings it back, on this server or any other;
  - **`derived::compute`** (the `merged` state): a dropped origin does not count as left to merge into T, so a branch whose remaining records are all merged or dropped becomes `merged`;
  - **not cherry-pick:** an explicit pick of a dropped origin is a deliberate act and is allowed (it does not undrop);
  - **not undo stacks:** like `checkpoint`, a `drop` is no unit on any stack; reversing it is an `undrops` record.
- **Format impact:**
  - `drop` records are written at **`RECORD_FORMAT = 3`, `MIN_READER = 3`**; every other kind keeps format 2 and stays byte-identical, and `RecordKind` (already `#[non_exhaustive]`) gains `Drop`;
  - segments are versioned by content (`fin_store::segment::version_for`): a segment holding a `drop` is format 3 / min reader 3; segments without one keep their versions. A 4C reader meeting a `drop` (an unknown kind) opens the project read-only with `NEWER_FORMAT`, which is the existing rule, so mixed-version collaborators degrade safely;
  - **no upcaster:** no stored record changes shape, and the op schema (`schemaVersion`) does not change (no op is added);
  - **DB migration** `m000N` adds a derived `drops` index `(branch, origin) → (seq, dropped)`, rebuilt from the log by rebuild and import like the other derived tables;
  - goldens: the encoding of a `drop` record; a 4C fixture project opens with identical state; a segment with a `drop` read by the 4C reader version is `NEWER_FORMAT`.
- **RPC:** `history.drop {projectId, txId, author, into, from, drops?, undrops?, dryRun?, argsHash?}` → `CommittedTx`; `fin drop <txId>… --from <b> [--into <b>]` and `fin drop --undo <txId>… …`. On a protected target it needs `review.decide`; elsewhere `project.write`. Audited as `history.drop`.

### 8.4 Merges limited to what was reviewed

- `MergeRequest` gains **`include?: [TxId]`** (origin roots) and **`sourceUpTo?: Seq`** (a seq on the source branch). Both are applied **inside the merge's write transaction**, to the candidates planned at the heads of that moment: a candidate is copied only if its origin root is in `include` (when given) and its source seq is ≤ `sourceUpTo` (when given). The rest stay on the source and are reported as **`notIncluded`**.
- A candidate that is included but depends on one that is not (or on a dropped one) is `DEPENDS_ON_SKIPPED`; for a review merge this is a conflict, and the merge writes nothing.
- `fin merge` gains `--include <txId>…` and `--up-to <seq>`.

### 8.5 Conflicts as review items; protected branches

- Whenever any `history.merge` answers `CONFLICT`, `finisherd` opens or updates a **`mergeConflict`** item for `(source, target)` with the conflict list; it becomes `superseded` when a later merge of the pair succeeds or the source is archived. Rebase conflicts on `tx.commit` stay with their writer.
- **Protected branches:** a write whose resolved target is protected (`tx.commit`, `history.undo|redo|checkpoint|cherryPick|revert|merge`, `branch.set` of it) needs `branch.protected` (stage 4) and is re-checked in the store's write transaction through the guard (§4.4 stage 6): `BRANCH_PROTECTED` (exit 7). `review.merge` and `history.drop` into a protected branch need `review.decide` instead. `review.policy` writes are human-only.

### 8.6 RPCs and `fin` (5E)

| RPC | Scopes |
|---|---|
| `review.request`, `review.comment`, `review.refresh` | `review.write` |
| `review.list`, `review.get`, `review.watch` (stream: item changes, not re-plans) | `project.read` |
| `review.decide`, `review.merge`, `review.close` | `review.decide` |
| `history.drop` | `project.write`, or `review.decide` on a protected target |
| `review.policy`, `review.policy.get` | `auth.admin` (human) / `project.read` |

`fin review request|ls|show|refresh|comment|decide|merge`, `fin drop`, `fin merge --include/--up-to`.

### 8.7 Security, codes, tests (5E)

- **Security:** an agent can open a review and comment, but not decide, drop into or merge into protected branches by default; review text is content; the change list is computed by the server.
- **Codes:** 3 `REVIEW_NOT_FOUND`, `REVIEW_UNDECIDED`; 4 `REVIEW_STALE`, `REVIEW_HAS_CONFLICTS`, `REVIEW_CLOSED`; 7 `BRANCH_PROTECTED`; merge reasons `DROPPED`, `notIncluded`.
- **Tests:** `fin-store` properties: a dropped origin is never copied by any merge into the target or its later descendants, whatever the order of drops, undrops, merges and imports; `include`/`sourceUpTo` never copy a record outside them, and a commit that lands between planning and the write transaction is not included; `derived::compute` agrees between a writer and a follower that imported the records in another order; the format goldens of §8.3. Server: `REVIEW_STALE`; pending never means accept outside the snapshot; conflicts open and supersede items; protected `main` refuses an `agent` commit (stage 4 and the in-transaction guard) and accepts `review.merge`; `review.verdict` entries; an e2e `review` scenario with an agent credential and the local principal as reviewer, including a project move that keeps the rejection.

---

## 9. Server and RPC surfaces for the frontend (sub-project 9)

All over the ticketed WebSocket as a `device` principal:
- **Connect:** `/.well-known/finisher`, pairing (§5.2), `/auth/token`, `/auth/ws-ticket`, `server.hello`, `auth.whoami`, `env.describe`; HTTP 426 on a protocol mismatch.
- **Live streams:** `ops.subscribe`, `project.events` (existing), `jobs.watch`, `library.watch`, `review.watch`, `auth.pair.pending`, `auth.devices.watch`; each starts with a snapshot item and is closed on revocation.
- **Settings and consent:** `diagnostics.status/consent` (the onboarding question is the only way training turns on through an RPC), `settings.write`, `server.status`.
- **Libraries UI:** `library.list/status/files/search`, `library.watch`, offline and stale states; `library.add` from the device.
- **Review mode:** `review.*`, `history.drop`, A/B compare through `eval.frame` on both branches.
- **Approvals:** `APPROVAL_REQUIRED` outcomes (the approval cards are sub-project 7's).
- **Errors** are codes with parameters (arch §17.1); every code above is in `@finisher/contracts`.

---

## 10. Dependencies and licences

| Need | Choice | Licence | Batch |
|---|---|---|---|
| Control store, catalogs, FTS | `node:sqlite` (Node built-in; checked under Electron 44/45's Node 24.22) | MIT (Node), public domain (SQLite) | 5A, 5D |
| HMAC, ECDSA P-256, AES-GCM, random | `node:crypto`; Rust `hmac` 0.12 + `sha2` 0.10 in `fin` | MIT OR Apache-2.0 | 5A |
| WebSocket server under our protocol | `ws` 8 (already under `@effect/platform-node`; pinned directly) | MIT | 5A |
| Windows pipes, ACLs, Job Objects, enumeration | `windows-sys` 0.61 (already in the tree) | MIT OR Apache-2.0 | 5A, 5C, 5D |
| Linux `prctl`, `statx`, cgroups | `libc` (already in the tree) | MIT OR Apache-2.0 | 5C, 5D |
| Job protocol, worker binary | `fin-jobs`, `fin-workers` (ours) | MIT | 5C-1 |
| Sandboxing at spawn | `fin-sandbox` (sub-project 7's crate, behind 5C's `Spawner` trait) | MIT | 5C-1 hook; 7 |
| Mapped-file bulk channel | sub-project 6's shared-memory crate (5C only creates, hands over and deletes the files) | MIT | 6 |
| DPoP / JWS / JWK thumbprints | `jose` 6 | MIT | 5B |
| Self-signed certificates | `rcgen` 0.14, `ring` provider, in `fin-node` | MIT OR Apache-2.0 (`ring`: Apache-2.0 AND ISC, allowed) | 5B |
| Keychain | `keyring` 3, exact pin, `async-secret-service` over `zbus` (pinned), no `dbus` | MIT OR Apache-2.0 (`zbus` MIT) | 5B |
| NFC keys | `unicode-normalization` 0.1 | MIT OR Apache-2.0 | 5D |
| Fingerprint, probe | `fin-probe` (ours), `blake3` | already allowed | 5D |

**Rejected:** `better-sqlite3` (a native module with its own Electron ABI matrix); `node-forge` (dual BSD-3-Clause OR GPL-2.0); `keytar` (archived); `keyring`'s `dbus` backends (libdbus, AFL-2.1 OR GPL-2.0+); mDNS libraries; JWT libraries (no JWTs are issued). Each batch's plan runs `cargo deny` and the JS licence gate over what it adds.

---

## 11. Decisions taken here

| # | Decision | Section |
|---|---|---|
| 1 | Trust comes from credentials, never from transports | §2, §4.2 |
| 2 | Key files with a mutual HMAC handshake bound to `envId` and the canonical endpoint; one retry after a restart | §4.2 |
| 3 | Keys rotate at every start; credential keys derive from a master | §4.1 |
| 4 | The socket lives in `<server-data>/run/`, apart from `auth/` | §4.1, §4.11 |
| 5 | A native Windows pipe listener in `fin-node` with its own security descriptor, first-instance and remote-rejection flags, and peer SIDs | §4.3 |
| 6 | Effect RPC's client id identifies connections; decoding stays at the protocol level | §4.3 |
| 7 | One-time tickets in `Sec-WebSocket-Protocol`; Host allow-list and Origin checks | §4.3 |
| 8 | One middleware chain: decode → authenticate → resolve (store-resolved branches, directories, paths) → authorise (all scopes, human-only, reach) → policy (`allow|deny|ask`) → handler (guards re-checked in the write transaction) → after | §4.4 |
| 9 | Every RPC declares its scopes (all required) and its resolver | §4.5 |
| 10 | Human-only lands in 5A; fixes address the user and contain no command | §3.5, §4.6 |
| 11 | No command line can opt in to training; the console refuses it | §3.4, §4.7 |
| 12 | Console mutations need a TTY and a server-issued typed challenge | §4.7 |
| 13 | `agent` and `mcp` credentials; expiry 24 h idle / 7 d absolute (MCP: 30 d idle) | §3.2, §4.1 |
| 14 | `console` and `mcp` event sources; sources and actors from principals | §3.4 |
| 15 | `fin` exit code 7 = denied | §3.5 |
| 16 | Protocol 2 (handshake, tickets, `asset.probe {projectId}`, required generations) | §3.6 |
| 17 | `control.db` in `node:sqlite`; no autocheckpoint on the event loop; batched deletes | §3.1 |
| 18 | The audit log is separate, always on, aggregated per credential kind per minute | §3.3 |
| 19 | A separate `envId` in `control.db`, bound to the identity key, mapped from every server id | §3.7, §5.8 |
| 20 | Every device is the owner in v1; no roles, no project lists, no typed codes, no capability URLs, no signed rotations | §1.2 |
| 21 | DPoP freshness from server nonces; `htu` checked against public URLs | §5.3 |
| 22 | A locked or missing keyring falls back to the file store without prompting | §5.5 |
| 23 | 5C owns the one job system and the one `fin-workers <family>` binary; spawning lives in `fin-node` behind a sandbox hook; processes enter their kill tree before they run; stdin-EOF exit everywhere | §6.0, §6.2 |
| 24 | Crash isolation targets the in-flight item; resource failures are never bisected; stall detection is separate from liveness | §6.2, §6.5 |
| 24a | Warm workers in an `interactive:<family>` pool serve short jobs back to back, with idle expiry (60 s) and recycling (500 jobs / 30 min) | §6.3 |
| 24b | Workers never start processes; helpers are siblings started by 5C, connected only through the job's mapped files | §6.4 |
| 24c | 5C is split: the engine needs only 5A Tasks 1–2; the RPCs need 5A Tasks 3–6 | §6.8 |
| 24d | `control.db` migrations run in independent per-area chains | §3.1 |
| 25 | Enumeration never opens a file; placeholders (files and directories, Nextcloud suffixes) are never read | §7.2, §7.3 |
| 26 | Volume identity per library; an unmounted share is offline; spin-up-tolerant timeouts | §7.1, §7.2 |
| 27 | Demand-driven NAS polling: 60 s while in use, 6 h otherwise, freshness pass before answering | §7.2 |
| 28 | Review merges take `include`/`sourceUpTo` checked in the write transaction; pending is never accept outside the snapshot | §8.2, §8.4 |
| 29 | Rejections are `drop` records in the op log (format 3 for those records only; no upcaster) | §8.3 |
| 30 | `review.verdict` is filled in 5E (not 10) | §3.4 |
| 31 | Protected branches per server, default `main`; `local` keeps the scope, agents do not | §8.5 |

---

## 12. Owner questions

1. **Where do review items live (now that rejections are durable `drop` records in the op log)?** *Recommended:* in `control.db` of the server that holds the project; open `branchReview` items are re-derived after a move. *Trade-off:* comments and not-yet-merged accept marks of open items stay on the server that recorded them. The alternative, write-once review event files in `.finisher/review/` (sync-safe, any server can append, comments from read-only followers), costs a second synced format with its own conflict, compaction and `collect: never` rules.
2. **Make the human-only operations human-only now (5A), with the TTY-gated `finisherd` console as the human interface until the UI, rather than in sub-project 7?** *Recommended:* yes. *Trade-off:* until sub-project 9, consent changes (except training on, which waits for onboarding or a hand edit), purges, `project default`, confirmed takeovers and relocations need an interactive terminal on the server (`docker exec -it` on a NAS). Agents and tests lose `fin diagnostics consent|purge|project default`, `fin project takeover --confirm` and `--relocate`. Waiting leaves them open to every agent for two more sub-projects.
3. **How do browsers reach a remote server?** *Recommended:* only over a route whose certificate the browser trusts (owner-supplied files such as `tailscale cert`, or a reverse proxy), or over loopback through an SSH tunnel; SPKI pinning is for the desktop shell and native clients. *Trade-off:* the v1 web client on a LAN needs one setup step. Alternatives: an ACME client (needs public DNS and ports 80/443, wrong for a NAS behind NAT), or clicking through certificate warnings (trains users to accept them).
4. **May the local audit log keep device names and remote addresses?** *Recommended:* yes, for pairing, revocation and failed authentication only; local, owner-only, never uploaded, 180 days. *Trade-off:* it is the one store holding IP addresses (D34 forbids them in the journal); without them a stolen-link or brute-force incident cannot be traced.
5. **Should the `local` principal keep `branch.protected` and `review.decide`, so that `fin` run by the user (and, before sub-project 7, any agent harness the user starts in a shell) can still commit and merge into `main`?** *Recommended:* yes for `local`; `agent` and `mcp` credentials never get them by default. *Trade-off:* a hand-run agent using the local key can still write `main` without review until it runs under sub-project 7 or uses an agent credential. Revoking it now forces every `fin` flow and test onto branches and reviews before the runtime that makes those branches exists.
6. **Until the sub-project 7 sandbox exists, human-only is advisory against agents running in a raw shell (they can read the key files, and can drive a pseudo-terminal through the console's confirmation). Accept that, or require hand-run harnesses to use agent credentials?** *Recommended:* accept it for now, document it in the README's security model, and offer agent and MCP credentials (`finisherd agent mint`, `finisherd mcp setup`) as the recommended way to run a harness by hand, without forcing them. *Trade-off:* forcing credentials (refusing writes from `local`) would not stop a raw-shell agent either (it can read `local.key` or `console.key` as easily as its own credential), so it buys attribution, not security, at the cost of breaking every script and test that drives `fin` directly; the real fix is sub-project 7 denying agents read access to `<server-data>/auth/`.

---

## 13. Risks

| Risk | Mitigation |
|---|---|
| Effect 4.0.6's RPC client ids or protocol hooks differ from what §4.3 assumes | 5A Task 3 starts by reading the source and recording the answer; the chain needs only a connection id per request |
| The native pipe listener (napi threads, overlapped I/O) is new native code | its own task with Windows-only tests; the handshake still protects a libuv pipe if the listener must be disabled |
| `node:sqlite` blocks the event loop | per-statement budget; no autocheckpoint on the main connection; batched deletes; fallback: the same schema through `rusqlite` in `fin-node` |
| Electron's Node (24) differs from the repository's (26) | only APIs present in both; sub-project 9 decides whether the desktop ships its own Node for `finisherd` |
| Keychain prompts, missing or locked Secret Service | file store with optional wrapping; never prompt |
| Browsers and self-signed TLS | Owner question 3 |
| Warm workers leak memory or carry state between jobs | recycling after 500 jobs / 30 minutes; fresh-vs-reused equality tests per family; a crash loses only the worker |
| `fin-sandbox` arrives after 5C | the `Spawner` trait; unsandboxed spawns with the env allow-list, reported in `server.status` |
| NAS scans hang, disks sleep | worker processes, abandonment limits, spin-up-tolerant timeouts, demand polling |
| Sync providers hydrate on access | enumeration without opening; process-wide materialisation off on macOS; placeholder markers on every OS; an `open` counter in tests |
| A `drop` record makes a project unreadable to 4C-era servers | by design read-only (`NEWER_FORMAT`), never wrong; written only when someone rejects |

---

## 14. Findings in the architecture spec

Contradictions and gaps found while designing this; each batch's "as built" pass records the resolution in arch §27.

1. **§21.1 decides trust by transport** (local socket → `fin`, WebSocket → `ui`), but the WebSocket token sits in a file any same-user process can read, and loopback is reachable through SSH tunnels. Resolved by §3.4 and §4.3.
2. **§8.4 "peer-credential checks on the local socket"** are impossible from Node, Landlock (ABI 4) does not mediate `connect()` on pathname sockets, and libuv's Windows pipes take the default DACL. Resolved by key-file proofs and the native pipe listener (§4.2, §4.3).
3. **§8.4's env allow-list** lacked a way to hand `fin` its credential without a secret in the environment. Amended: `FIN_CREDENTIAL` (a path).
4. **§16 pins a self-signed certificate "at pairing"** for every client; browsers cannot pin, and the web client is v1. Owner question 3.
5. **§16 DPoP-bound tokens** assume usable client clocks, while §27.9 records clients off by ~10 s for a day. Resolved by server nonces (§5.3).
6. **§21.1 phasing puts human-only operations in sub-project 7** although permissions are sub-project 5, and §24 puts `review.*` verdicts in sub-project 10 while §11.3's per-transaction accept/reject is sub-project 5's. Owner question 2; Decision 30.
7. **§11.3 review markers `open/review/resolved`** do not exist in the 4C model (`Marker` has no status); sub-project 10 needs an op.
8. **§6.4 "conflicts become review items"** presumes per-transaction action, but 4B's merge has no inputs for it, and nothing records a rejection. Resolved by `include`/`sourceUpTo` and `drop` records (§8.3, §8.4).
9. **§5 "single owner"** vs §16's device "role". Resolved: every device is the owner in v1.
10. **§16 "environments … sync"** is undefined. Read as §5.6.
11. **§6.7 fingerprints include mtime,** so copies never match across machines and relinking (§20) falls back to full hashes over the network. Accepted, on demand only (§7.3).
12. **§6.7 change detection** ignores inotify limits, cloud placeholders (any open or read downloads them, and on macOS whole directories can be dataless), unmounted mount points that look like empty libraries, and NAS hibernation. Resolved by §7.1–7.3.
13. **§7 gives TS "its own SQLite"** but forbids synchronous calls on the event loop. Resolved by the budget and rules in §3.1.
14. **§16 audit log vs D34 "never stored raw: host names, IPs"**. Owner question 4.
15. **4A/4B expose "a person's decision"** (`takeover {confirm}`, `open {relocate}`) to `fin`, which D16 makes agent-only. Resolved by §4.6.
16. **The repository pins Node 26.12, but Electron 44/45 embed Node 24.22**, so an embedded `finisherd` (arch §4, desktop) runs on a different Node than the one it is tested on. Sub-project 9 must decide; sub-project 5 stays within both (§1.5).
17. **The server id is reminted on a move** (4A), so it cannot be an environment id (§3.7).
18. **Tracked files hold personal data,** against the public-repo rule: arch §27.8 names the Apple Developer team's owner and team id, and D3 holds an absolute local path.

---

## 15. Review findings (revision 2) and where they are resolved

| Finding | Resolution |
|---|---|
| **B1** console could opt in to training; console commands scriptable; fixes printed commands | §3.4, §4.6, §4.7 (training on device-only; `CONSENT_UI_ONLY` from the console; TTY + server-issued typed challenge), §3.5 (fix texts) |
| **B2** plan commits not green; D4/D6 missing | plan reordered: the `fin` handshake is Task 4, enforcement switches on in that commit; Tasks 1, 2 and 10 can start now; Tasks 3+ wait for D4 and D6 |
| **B3** `exclude` merged unseen commits; pending meant accept | §8.2, §8.4 (`include`/`sourceUpTo` in the write transaction; pending only within the snapshot, and only on request) |
| **B4** rejections not durable | §8.3 (`drop` records: format 3, no upcaster, derived index); split out of Owner question 1 |
| **B5** envId = server id | §3.7, §5.8 (separate `envId` in `control.db`, bound to the identity key, `server_ids` map) |
| **H1** restarts looked like attacks; no MCP principal | §4.2 (one re-read and retry, `keyStale`), §3.2 (`mcp`) |
| **H2** socket beside secrets; pipe security; sandbox reach; `FIN_CREDENTIAL` | §4.1, §4.3 (native pipe listener decided), §4.11, arch §8.4 amended |
| **H3** no middleware chain, branch refs unresolved, no `ask`; `branches` dropped from mint | §4.4, §4.1, §4.8 |
| **H4** RPC scope errors | §4.5: `project.open` by state; resolvers for `dir`; `asset.probe {projectId}` with reach; protected-branch guards in the write transaction (§4.4); job ownership (§6.1); `jobs.submit` all-of scopes; `auth.revokeSelf` without an id; capability URLs cut to 6 (with reach); `diagnostics.record` scope; `fonts.fetch` scope |
| **H5** jobs kill tree and bisection | §6.2, §6.5 |
| **H6** library enumeration and mounts | §7.1–7.3 |
| **M1** `node:sqlite` under Electron | checked (§1.5); repeated on macOS and Windows in 5A Task 14 |
| **M2** WAL checkpoint on the event loop; unbatched deletes | §3.1 |
| **M3** connection ids; decoding | §4.3 |
| **M4** keyring pins and locked keyrings; DPoP `htu`; ticket in URL; Host allow-list | §5.3, §5.5, §4.3 |
| **M5** audit aggregation; MAC binding | §3.3, §4.2 |
| v1 cuts | §1.2 |
| Defaults: credential expiry, NAS poll | §4.1 (24 h / 7 d, justified), §7.2 (demand polling, justified; no owner question needed) |

---

## 16. Cross-design reconciliation (revision 3)

| Item | Resolution |
|---|---|
| 5C owns the shared job system (model, scheduler, pools, the one `fin-workers <family>` binary, `fin-jobs`, kill tree, retries, poison handling, `jobs.*`, `fin wait`, `job.done`, `worker.exit`) | §6.0; batches §1.4; kinds §3.4; nothing in sub-project 5 builds media, plugin or sandbox code |
| Sub-project 6 contributes only the `media` family, the mapped-file channel crate and `job.done` media fields | §6.0, §6.4 (5C creates and deletes the files, never reads them), §3.4 (`kv` 2) |
| Sub-project 8 contributes plugin job adapters | §6.0, §6.4 |
| Warm workers for `fin look` ≤ 300 ms | §6.3 (interactive pool, idle expiry 60 s, recycling, ≤ 5 ms dispatch) |
| Helpers are siblings started by 5C; workers never start processes | §6.2, §6.4 |
| Sandboxing is sub-project 7's `fin-sandbox`; 5C calls it at spawn, unsandboxed with the env allow-list until then | §6.2 (`Spawner` trait in `fin-node` `jobhost`), §1.2 |
| 5C runs beside sub-project 6's 6A-1; what it needs from 5A | §6.8, §1.4: 5C-1 needs 5A Tasks 1–2; 5C-2 needs 5A Tasks 3–6; per-area migration chains in `control.db` so they never collide (§3.1) |
| Electron embeds Node 24 | §1.6 (note for sub-project 9), §1.5, §14 item 16 |
| Kill-tree race on Windows | improved: processes are created suspended, assigned to their Job, then resumed (§6.2), instead of assignment by pid after spawn |
