# Sub-project 5: Control plane breadth — Design

- **Status:** revision 3 (2026-11-07). Revision 2 answered an independent review of revision 1 (`45039ab`; §15 maps every finding). Revision 3 applies the cross-design reconciliation with sub-projects 6, 7 and 8 (§16): 5C owns the shared job system, adds warm workers and helpers, calls sub-project 7's `fin-sandbox`, and is split so its engine can start after 5A Tasks 1–2. Revision 4 records the owner's decisions of 2026-11-07 (§12; §17 maps where each one landed): the six sub-project 5 answers as recommended, Node 24 as the repository's Node, `auto` merges under a per-project owner policy, and the platform floor (Windows 11, Linux kernel ≥ 6.2). Revision 5 (2026-11-08) records **5C-1, the job engine, as built** (§6.0–6.6 and §6.8–6.12; §6.10 maps every decision of its plan to As built or Deviation); Revision 6 (2026-11-08) records **5A, principals, local credentials and scopes, as built** (§3.1–3.7 where 5A built them, and §4; §4.15 maps every decision of its plan to As built or Deviation, §4.16 lists the owner decisions and review fixes, §4.17 the residuals). The rest of this design is not built yet, apart from the parts other sections mark.
- **Parent:** `docs/specs/2026-11-01-finisher-architecture-design.md` (cited below as *arch §n*). This document refines arch §4 rule 6, §5 (`AuthProvider`, `ConnectRelay`), §6.4 ("conflicts become review items"), §6.7 (media libraries and indexing jobs), §8.3 (allow/ask/deny), §8.4 (token delivery, the env allow-list), §9.9 (jobs), §11.3 (per-transaction accept/reject), §16 (pairing, auth, security), §20 (library root mapping), §21.1 (sources and human-only operations) and §27.12.5 (the local transport and its WebSocket token). Where the two disagree, this document wins for sub-project 5, and arch §27 gets a pointer when each batch merges.
- **Builds on:** sub-projects 2 and 4 (4A, 4B, 4C, on stacked branches; 4C is `sp4c-edit-breadth`) and the diagnostics hooks D1–D7 on the 4C branch. D1–D7 are done (2026-11-07); 5A changed D4 (sources per transport) and D6 (actor headers) as §3.4 describes.
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
| **5C-1** Job engine (built; §6, cross-platform validation in Task 14) | the shared job system (§6.0): `fin-jobs` (protocol types, envelope), the one `fin-workers <family>` binary with the `test` family, spawning in `fin-node` through a sandbox hook, a host-owned kill tree on every OS, the persisted scheduler with pools and warm workers, helpers and mapped files, retries, poison handling and isolation, stall detection, `job.done` and `worker.exit` | 5A Tasks 1–2 | 5D; sub-project 6 (6A-1 runs beside it), 8 |
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
- **Electron's Node is the repository's Node** (Electron 44/45 embed Node 24.22.0; the repository moved from Node 26.12.1 to 24.22.2 on 2026-11-08, §1.6).
- **`TASKS.md` "Follow-ups for sub-project 5":** Windows named-pipe DACL and pipe squatting, server-data ACL check, graceful stop without SIGTERM; decode failures and unknown tags as `FinError`; relative paths refused server-side; a client deadline and frame-size cap; test hooks out of release builds; `childEnv` provider variables (sub-project 7).

### 1.6 Cross-cutting notes for other sub-projects

- **Sub-project 6 (media):** the job system is 5C's (§6.0). Sub-project 6 contributes the `media` family inside `fin-workers`, its job specs, the mapped-file bulk channel (its own shared-memory crate) and `job.done` `kv` 2 with media fields; `fin look` uses a warm `interactive:media` worker (§6.3). It can build 6A-1 beside 5C-1 against the `JobEngine` API and the `test` family, and needs 5C-2 only for `fin` commands that return job ids.
- **Sub-project 7 (agent runtime):** owns `fin-sandbox` (profiles `agent`, `hook`, `worker`, `plugin`), which 5C's `Spawner` trait calls at every spawn; owns approvals behind `APPROVAL_REQUIRED` and the rules and hooks of the chain's policy and after stages (§4.4); mints `agent` credentials (§4.1); runs the `auto` merge policy as the server-internal `policy` actor (§8.5, agent §6.5); applies the sandbox reachability rules of §4.11. Its `SecretStore` uses 5B's secret store; in v1 it holds only the OpenRouter key the user pastes, because CLI agents use the user's own Claude Code or Codex sign-in, which Finisher never copies or stores (agent §8.6).
- **Sub-project 8 (plugins):** contributes plugin job adapters, started by 5C as helpers or as a job's only process under the `plugin` profile (§6.4); `worker.exit` records them with a keyed plugin id. Third-party code plugins are allowed before sub-project 7's sandbox (owner decision, plugins §20 item 1): installing one stays human-only (`plugin.install`, §4.6), and until `fin-sandbox` exists 5C starts plugin processes like workers, unsandboxed with the env allow-list (§6.2). Generation providers add `externalRef` to jobs (cut from 5C, §1.2).
- **Node 24 (owner decision, 2026-11-07).** **Electron 44 and 45 embed Node 24.22.0**, so the repository targets **Node 24** too: a `finisherd` started by the desktop shell as an `ELECTRON_RUN_AS_NODE` child then runs on the Node it is tested on, and the desktop ships no second Node. The repository moved from Node 26.12.1 to Node 24.22.2 on 2026-11-08 (`engines`, `.node-version`, CI, docs; `TASKS.md` records the verification of `node:sqlite` and every Node API in use). Checked on 2026-11-07 on Linux x64 with Electron 44.9.3 and 45.0.0: `node:sqlite` loads without an experimental warning, with SQLite 3.53.4, FTS5 and WAL. Sub-project 5 uses only APIs present in Node 24 (§3.1). `fin-node`'s napi build targets the same Node-API level in both runtimes (arch §27.1 measured the Electron loads).
- **Sub-project 9 (frontend and Electron):** owns SSH/WSL routes, the device key in the keychain, and the onboarding question that is the only RPC route to training content (§3.4). It runs `finisherd` on Electron's Node 24, the repository's Node.

---

## 2. Principles

1. **Trust comes from a credential, never from a transport.** Loopback is not local (an SSH tunnel ends on loopback; so does a browser tab after DNS rebinding). A Unix socket is not the user (a sandboxed agent can still `connect()` to it). Every connection proves a credential; with none, it may only say hello.
2. **The client authenticates the server too,** and both sides bind the proof to the environment and the endpoint. A squatted pipe, a stale `finisherd.json` or a relay to another server never receives a request.
3. **Human-only is a property of the principal, enforced in one place,** before any handler runs. A request from `fin` (agent-only, D16) can never be human, whatever its headers claim. **Until sub-project 7's sandbox (7C), this is advisory against code running in a raw shell as the user** (it can read key files); owner decision 6 accepts that and has it documented (§4.6).
4. **No opt-in to training content from any command line.** The only routes stay D14b's: the onboarding question (sub-project 9), a hand edit of `privacy.training` in `settings.json`, and `FIN_TRAINING` in `finisherd`'s own environment.
5. **Error fixes address the user and never hand an agent a ready-to-run command** for a human-only operation.
6. **Least privilege by default, the status quo by explicit grant.** The `local` principal keeps what `fin` can do today, minus the human-only operations (owner decision 5).
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
- **Node versions:** the target is **Node 24** (owner decision, §1.6), the Node Electron 44/45 embed (24.22). Node 24 is the only Node tested; only APIs present in Node 24 are used.
- **Versioning:** `meta` holds `envId` (§3.7) and `createdAt`; `migrations {chain, n, appliedAt}` records what ran. **Migrations come in independent chains, one per area** (`core`, `auth`, `jobs`, `libraries`, `reviews`, `devices`), each a frozen ordered sequence (`control/migrations/<chain>/0001.ts`) with its own `formatVersion`/`minReaderVersion` in `meta`, and no foreign keys across chains. Batches that run in parallel (5A and 5C-1) therefore never fight over migration numbers. Pending migrations run in one transaction at start, after a `VACUUM INTO control.db.pre-migrate.bak` (the newest two kept). A chain whose `minReaderVersion` is above this code fails the start with `CONTROL_DB_TOO_NEW`.
- **Damage:** `PRAGMA quick_check` at open. A failure moves the files aside to `control.db.corrupt-<ulid>` and starts empty with an audit entry: the environment id and devices are lost (devices re-pair: fail closed), catalogs rescan, jobs are lost, open reviews are re-derived (§8.1). Rejections are not lost: they are in the op log (§8.3).

- **As built (5A Task 1):** `serve` opens the store after the server lock and before any listener; only `quick_check` damage or a file that is not SQLite resets it, while a schema this SQLite cannot parse refuses the start (`CONTROL_DB_TOO_NEW`); the statement budget is tested as a median ≤ 1 ms and a p99 ≤ 50 ms under a full-suite load, and p99 ≤ 5 ms with `FIN_STRICT_TIMING=1` (the per-OS p99 is measured in 5A Task 14). The checkpoint worker is loaded from its `.ts` file through Node's type stripping, which a packaged `finisherd` must keep working (sub-project 9).

Tables per chain: `core` (5A Task 1) `meta`, `migrations`, `server_ids`, `audit`; `auth` (5A Task 9) `credentials`; `jobs` (5C-1, three steps) `jobs`, `job_items`; `devices` (5B) `devices`, `pair_offers`, `pair_pending`; `libraries` (5D) `libraries`; `reviews` (5E) `reviews`, `review_events`, `review_comments`, `branch_policy`.

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
- **Kinds:** `auth.fail` (bad proof, bad ticket, bad DPoP proof, rate-limited), `auth.denied` (a human-only, scope, reach or policy denial: RPC, scope, principal kind), `auth.ticket`, `credential.mint|revoke|expire`, `pair.offer|redeem|approve|reject|expire`, `device.revoke|rename`, `consent.change`, `diagnostics.purge`, `settings.write`, `project.takeoverConfirmed`, `project.relocated`, `listener.start|stop` (with the SPKI pin), `env.moved|adopted|reset`, `library.add|remove|set`, `history.drop`, `review.merge` (protected targets; `auto: true` for auto merges, §8.5), `review.policy` (old and new value), `console.refused` (a console command refused for lack of a confirmation; a missing TTY is refused by the console itself, before it connects), `audit.purge` (written in the same transaction as the purge, with its count and cutoff `before`; an owner's purge never removes `audit.purge` rows, so the log always says it was purged, how far back and by whom); reserved for 7/8: `grant`, `approval`, `hook.block`, `hook.modify`, `sandbox.violation`, `plugin.install`.
- **Aggregation:** `auth.fail` rows are aggregated **per credential kind (and remote address, in 5B) per minute**, with `count`, so a flood of connections cannot flood the log.
- **Always on.** Consent does not gate it, `DO_NOT_TRACK` does not stop it; it is never uploaded, never part of `fin diagnostics export`, never read by the dataset builder.
- **What it may hold** that the journal may not (owner decision 4): device names and remote addresses, **for pairing, revocation and failed authentication only** (`pair.*`, `device.revoke`, `credential.revoke`, `auth.fail`); every other kind records neither. Never secrets, tokens, MACs, nonces, links, paths or request payloads.
- **Retention:** 180 days or 100k rows, swept hourly in batches. Readable and purgeable only by the owner (`auth.audit.list`, `auth.audit.purge`).
- **Never blocks:** a failed audit write is logged to stderr and counted in `server.status`; the request still gets its answer (denials still deny).
- **As built (5A):** every kind's `details` is a closed schema (names only for pairing, revocation and auth failures; addresses, and on Windows the pipe `peer {pid, sid}`, only for pairing and auth failures); `auth.fail` and `auth.denied` rows carry the `transport`, and `auth.denied` aggregates per principal, code and RPC per minute; 5A adds `credential.rotateMaster {count}` and `audit.purge {reason, count, before}`, written in the purge's own transaction and never purged; `auth.legacy` is a reserved kind, never written. The row-limit sweep cuts at `max(id) − maxRows`, and the age purge walks the `at` index.

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
- **As built (5A):** class 7 (`DENIED_CODES`) also holds `CREDENTIAL_EXPIRED`, `CONSOLE_NEEDS_TTY` and `CONFIRMATION_REQUIRED` (§4.13); sub-project 8 adds its trust and acceptance codes. The console exits with the same classes.

### 3.6 Protocol 2

5A bumps `PROTOCOL_VERSION` to **2**: the handshake (§4.2), tickets (§4.3) and the new `asset.probe {projectId}` (§4.5) are not compatible with protocol-1 clients, and every client lives in this repository and ships version-locked (arch §21). Protocol 2 also takes sub-project 4's pending changes: `tx.commit.generation` and `ops.subscribe.generation` become required, and the `project.recover` result change of 4B is recorded. The WebSocket upgrade refuses another version with **HTTP 426** and `{protocol: {min, max}}` (arch §16). **As built (5A Task 6):** `fin`'s `versionMismatch` is `protocolVersion < 2` alone; the RPC fixtures were re-captured (writes carry a generation, request ids restart at 1 after the handshake, the incarnation and the console's codes are normalised and substituted by `rpc_contract.rs`).

### 3.7 The environment id

- **A separate `envId`** (a ULID) is minted at the first start and stored in `control.db` `meta`. It is **not** the server id: the server id is reminted by `fin-store::server_id` when server data moves, and moving server data must not change the environment.
- **Bound to the identity key** (5B): the pairing link pins `(envId, spki)`, and `env.describe` returns both. Before 5B there is no identity key, and the id is bound to `control.db` alone.
- **`server_ids {serverId, envId, since}`** maps every server id this server data has had to the environment, so a lease holder named by server id (`project.status` `holder.serverId`) maps to an environment: `project.status` gains `holder.envId` when the holder is this environment, and `env.describe` lists `serverIds` so a client can map another environment's lease.
- **Copies and moves** (§5.8): a moved server data keeps its `envId`; a copy that runs beside its original must get a new one (`env reset`), because two servers holding one identity key could impersonate each other.

---

## 4. 5A — Principals, local credentials and scopes

> **5A is built** (2026-11-08; Linux x64 verified; the Windows paths are compiled and linted, and only the native pipe listener's own tests have run on Windows 11; the full macOS and Windows runs are 5A Task 14, still owed, §4.17). §4.1–4.14 below are rewritten **as built**. Marks: **As built** is a detail the plan left open or the build fixed; **Deviation** is where the build differs from revision 3 of this design or from the 5A plan. The plan is `docs/superpowers/plans/2026-11-07-sub-project-5a-principals-and-scopes.md`; its Decision numbers appear in parentheses, §4.15 maps all of them, §4.16 lists the owner decisions and review fixes, and §4.17 the residuals.

### 4.1 Data model and files

- **Server data layout:**
  - `<server-data>/auth/` (0700; owner-only DACL on Windows): `local.key`, `console.key` (32 random bytes, base64url, then `\n`, 0600), **rewritten at every start** after `control.db` opens and before the socket or pipe is claimed (Decision 5); `credential.master` (created once, kept until the owner replaces it with `finisherd auth rotate-master`, which ends every credential). Key files are written temp-then-rename, the file fsynced before the rename and the directory after it; a malformed `credential.master` refuses the start (`KEY_FILE_INVALID`). **As built:** on macOS `auth/` is excluded from Time Machine at every start (`tmutil addexclusion`, the sticky `NSURLIsExcludedFromBackupKey` form; a failure is a warning, never a failed start); Windows File History does not back up the default server data (`%LOCALAPPDATA%`), and elsewhere the README tells the owner to leave `auth/` out of backups.
  - `<server-data>/run/` (0700): **only** the Unix socket `fin.sock`. A sandbox may be allowed to reach this directory and nothing else of server data (§4.11). Windows uses the named pipe and needs no `run/` entry.
  - `finisherd.json` (0600) in server data: `{pid, wsPort, socket, protocolVersion: 2, envId, diagnostics: {enabled}}`, **no token** (Decision 25).
- **`credentials`** (chain `auth`, migration `0001`; Decision 32): `{id, kind: agent|mcp, label, scopes, projects, branches, createdBy, createdAt, lastUsedAt, idleExpiryS, absoluteExpiryAt, revokedAt, revokedReason}`. **As built:** ids are lowercase ULIDs; `revokedReason` is `owner|self|expired` (a column the plan did not list); rows load at start and a row ended more than 180 days ago is deleted then (its id is unknown again); `lastUsedAt` is written at most once a minute; at most 4,096 live credentials (`CREDENTIAL_LIMIT`, exit **4**, owner decision on the Tasks 8–9 review). A credential's key is never stored: it is `HMAC(credential.master, "fin/credential-key/v2" ‖ envId ‖ kind ‖ id)` (length-prefixed fields), recomputed to verify. **Deviation** (Decision 6 had `v1` without `envId`; review of Tasks 8–9, M3): the environment id is bound in, so a `credential.master` copied into another server data, or kept across a `control.reset`, derives no key there; the shared vectors carry one case per environment.
- **Revocation** sets the row ended in memory first (`markEnded`), then ends the credential everywhere (§4.8), then writes the row with retries (1 s growing to 60 s; a last try at shutdown), so a busy store never stops a revocation. Until it is written a restart would bring the credential back (§4.17).
- **Residual: restoring an old `control.db`** (with the same `credential.master`) brings back the rows of credentials revoked after that copy was made, and their files prove again. The remedy is `finisherd auth rotate-master` (`auth.credential.rotateMaster`, `auth.admin`, a TTY and a typed code): a new master is written durably first, every credential not ended yet is revoked through the one revoke path, and every credential file made before stops working. It is audited `credential.rotateMaster {count}`.
- **Lifetimes as built:** an expired credential's requests are refused at once (every request re-checks liveness), but a stream it opened runs until the next expiry sweep (every 60 s) ends it, up to **60 s past the expiry**. Idle expiry is measured from the last use, so an `mcp` credential (30 days idle, no absolute limit) or one minted with `--absolute none` **lives indefinitely while it is used**; only the owner's revocation (or `rotate-master`) ends it.
- **Expiry defaults: 24 hours idle, 7 days absolute** for `agent`; `mcp` 30 days idle and no absolute limit. Mint bounds: `idleS`/`absoluteS` from 60 s to 366 days, `absoluteS: null` for none. *Why:* an agent cooking a library overnight is idle between turns for minutes, not a day, so 24 h idle never cuts a working session, and it ends credentials a harness forgot; 7 days caps how long a copied credential file works. Sub-project 7 mints a fresh credential per session, so neither limit is felt there. `mcp` credentials (one per adapter install, long-lived by nature) are revoked by the owner.
- **The credential file** (written by sub-project 7 inside the session sandbox, or by `finisherd agent mint --out` / `mcp setup --out` for a hand-run harness): `{"v":1,"kind":"agent"|"mcp","id":"<ulid>","key":"<base64url>","env":"<envId>"}`. **As built** (review of Tasks 8–9, M2): `--out` is created by fin-node `createPrivateFile` before the mint (`O_EXCL` 0600 on Unix; `CREATE_NEW` with a protected owner-only DACL on Windows) and checked on the open handle (Unix: a regular file with one link, ours, no group or other bit; Windows: owner and DACL), then written in full and fsynced. A file that is not private is removed (`PRIVATE_FILE_UNSAFE`); a failed write, a cancelled code or Ctrl-C removes the file and revokes a credential already minted (the connection that minted it may revoke it without a second code). Without `--out` the file's contents are shown once. `fin` and the MCP adapter find it through **`FIN_CREDENTIAL`**, a path, which joins `childEnv`'s allow-list (Decision 34).

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

- `K` is the key of the credential named in the hello. Nonces are 32 random bytes; HMAC-SHA-256; every label is domain-separated, and length-prefixed fields (`u16` big-endian before each variable field; nonces as their raw bytes) keep concatenations unambiguous. Comparison is constant time on both sides. `tests/fixtures/auth/handshake.json` pins the bytes for TypeScript, Rust and the e2e harness (Decisions 6, 7).
- **Binding:** `envId` (from `finisherd.json`, or the credential file's `env`) and the canonical `endpoint` (the socket's real path, or the pipe name lower-cased) go into both MACs, so a proof made for one server or one socket is useless on another, and a relay between two servers fails.
- **Why a server proof:** a pipe name can be squatted, and a stale `finisherd.json` can name a dead socket that someone else re-created. Only a process that can read the key can produce the proof. `fin` checks the proof before it sends anything else, and the console does the same.
- **`fin`'s side** (`crates/fin/src/auth.rs`, Decisions 10, 11): discovery is `FIN_CREDENTIAL`, else `<server-data>/auth/local.key`; keys decode strictly (43 canonical base64url characters); on Unix a key or credential file that is not the user's or not private (`mode & 0o077`) is refused (`credentialUnsafe`), and on Windows a `FIN_CREDENTIAL` file's owner and DACL are checked on the opened handle; only regular files are read, opened without blocking, and never past 4 KiB. **As built** (review of Tasks 3–4): **with no credential `fin` sends nothing**: it fails locally with `UNAUTHENTICATED {reason: noCredential}` (exit 7) and a fix naming `FIN_CREDENTIAL` and the server's data, `fin whoami` answers `anonymous` without connecting, and diagnostics send nothing, so an unproved server never receives a request. A server from before the handshake is `versionMismatch` (no MAC sent); any error answer to `server.hello` is `helloFailed`.
- **A restart is not an attack.** On `noProof`, `badProof`, `envMismatch`, `endpointMismatch` or `helloFailed`, `fin` re-reads `finisherd.json` (unless `FIN_SERVER` names the server) and the key file **once**, reconnects and retries; a restart between reading the key and connecting is then invisible. Only a second failure is **`SERVER_UNTRUSTED`** (exit 7) with `details {endpoint, reason}` and its fix in words. A retry that succeeds through a changed key notes `keyStale`; `fin whoami` answers `{principal, retried?}`. The e2e scenario runs 100 `fin whoami` across a restart (each `ok` or `SERVER_UNAVAILABLE`, never untrusted), but a run seldom lands in the retry window, so the deterministic check stays the Rust test `restart_between_read_and_connect_is_retried` behind `fin`'s test hook.
- **Unknown credential, wrong MAC:** `UNAUTHENTICATED` (exit 7), audited `auth.fail` (aggregated, §3.3). The hello answers an unknown credential with a `serverNonce` and no proof, so ids cannot be probed. One prove per hello; a second `auth.prove` on a proved connection is `INVALID_REQUEST`; five failures on one connection close it after the reply.
- **Expired or revoked credential: Deviation** (Decision 8 gave such a credential no proof; owner decision on the review of Tasks 8–9, L4): a row that exists but has ended gets the server's proof made with its derived key, as a live one does, and a correct `auth.prove` is refused **`CREDENTIAL_REVOKED`** or **`CREDENTIAL_EXPIRED`** (exit 7, audited `auth.fail`), so `fin` says which instead of suspecting a squatter. A row deleted 180 days after it ended is unknown again. `fin`'s `noProof` fix says the server does not know the credential.
- **Before the proof** a connection may call only `server.hello` and `auth.prove`.
- **Cost:** one round trip and four HMACs per `fin` run; budget ≤ 1 ms added to `fin`'s p50. Not measured yet: 5A Task 14 measures it per OS.

### 4.3 Transports, connections and tickets

- **Connection identity: Deviation** (Decision 12). Reading Effect 4.0.6's `RpcServer` (recorded at the top of `connections.ts`) confirmed the client id: each server `Protocol` numbers its connections and handlers see `options.client.id`, so ids are unique only within one transport. But `RpcServer` answers an undecodable payload or an unknown tag with a defect, so **`finisherd` serves `FinRpcs` through its own NDJSON protocol** (`protocol.ts`) instead of Effect's `makeProtocolSocketServer`. It decodes each frame with the codec and parse options `RpcServer` uses, so the server's own decode never fails, and answers bad JSON, unknown frames and tags, bad headers and refused payloads with `INVALID_REQUEST` (exit 3) itself; a frame with no id to answer closes the connection. `Connections` is one map per transport (`{clientId, transport, principal, hello?, failures, peer?}`; an unknown id is `anonymous`, never created); `ConnectionRegistry` sums the transports. Each connection registers a `Link` (close, close after the reply, answer open requests then close), which revocation and `server.stop` use on every transport.
- **Frames** (Decision 13): inbound lines over 16 MiB answer `FRAME_TOO_LARGE` (exit 2) to the request id read from the line's start, then close; the WebSocket's `maxPayload` is the cap plus 64 KiB. `fin` caps what it reads at 16 MiB too (`PROTOCOL_ERROR {max}`). Client headers with the reserved `x-fin-` prefix are dropped.
- **Unix socket:** `<server-data>/run/fin.sock`, created under umask 0177 and chmod 0600.
- **Windows: a native named-pipe listener in `fin-node`** (Decision 38; `pipe_win`, `pipe_loop`, `pipe_io`, `pipe_sec`, napi-free, and the napi `PipeServer`; `PIPE_UNSUPPORTED` elsewhere). `finisherd` no longer lets libuv create the pipe. One thread owns every instance and one I/O completion port (overlapped `ConnectNamedPipe`, one pulled read of ≤ 64 KiB per connection, writes, posted commands). Instances are byte mode with `PIPE_REJECT_REMOTE_CLIENTS` and the descriptor `D:P(A;;GA;;;<user SID>)(A;;GA;;;SY)`; at most 64 client instances. **As built:** the first instance (`FILE_FLAG_FIRST_PIPE_INSTANCE`, else `PIPE_SQUATTED`, exit 1) is an **anchor**: disconnected at once and kept until shutdown, it takes no client, keeps the name held while every other instance closes, and holds `WRITE_OWNER`, which Windows refuses to every later instance (found on a Windows host) and which `allow(sid)` needs. **Deviation:** an allowed SID gets `0x12008b` (`FILE_GENERIC_READ | FILE_WRITE_DATA`), not `GRGW`, because `FILE_APPEND_DATA` is `FILE_CREATE_PIPE_INSTANCE` on a pipe and would let a sandbox host the pipe; once any SID is allowed the descriptor carries the low mandatory label `S:(ML;;NW;;;LW)` (an AppContainer runs at low integrity). Each client is checked before its first byte from `GetNamedPipeClientProcessId` and its token (user, integrity, AppContainer): a client that is not this user or an allowed SID, or a low-integrity one that is not an allowed container, is disconnected and audited `auth.fail {credentialKind: pipe, reason: peer|unidentified, peer}`. Close is graceful (writes finish; the handle closes once the client has read everything, left, or 2 s passed), so `Link.terminate` reaches pipe clients. In `finisherd`, `pipe.ts` wraps each connection as a `Duplex` that `NodeSocket.fromDuplex` makes a `Socket`, so the protocol is the socket's; `peer {pid, sid}` reaches `Connections` and the audit rows. **`fin` opens the pipe itself** (`pipe_client.rs`): `GENERIC_READ | FILE_WRITE_DATA` with `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`, so a squatter cannot impersonate the user (the `interprocess` crate connected at the impersonation level), waiting out `ERROR_PIPE_BUSY`. The handshake still runs: the DACL and the proof are two layers.
- **WebSocket** (Decision 24): our own `node:http` server on `127.0.0.1:0` with `ws` (a direct, pinned dependency) in `noServer` mode, so the upgrade request is ours. In order: path `/rpc` (404); `finisher.v2` offered in `Sec-WebSocket-Protocol` (otherwise 426 with `{"protocol":{"min":2,"max":2}}`); `Host` one of `127.0.0.1:<port>`, `localhost:<port>`, `[::1]:<port>` (403); `Origin` absent, one of those `http://` origins, or in `settings.json` `server.origins` (403; the file is re-read only when it changes); then exactly one live `finisher.ticket.<t>` (401). The answer carries `Sec-WebSocket-Protocol: finisher.v2`; `?ticket=` is never read; refusals other than path and version are audited `auth.fail {credentialKind: ticket, reason}`. The socket carries the ticket's principal from its first frame (the WebSocket never starts as `local`). **As built** (reviews of Tasks 6, 7 and 8–9): a client gets 10 s for its headers; at most 64 WebSockets in all (16 of them only for `device` and `console`), 32 for `local`, 8 per ticket owner, and 16 sockets that have not upgraded, where a new one first closes those that waited over 1 s; past a cap the upgrade is 503 `tooMany` (not an auth failure).
- **Tickets** (Decision 23): `auth.ticket {ttlS?}` (`server.read`; 30 s default, at least 1, at most 60) returns 32 random bytes base64url bound to the caller's principal; only `SHA-256(ticket)` is kept, with the principal and the expiry; one use, and an attempt with an expired ticket uses it up too; swept every 10 s; audited `auth.ticket`. **Deviation** (review of Task 6): instead of dropping the oldest ticket at the global 1,024, a principal holds at most 8 live (a newer one drops its own oldest) and mints at most 4/s (burst 16), `local` counts per minting connection (`local:<transport>:<clientId>`) under 64 live and 16/s (burst 64) for `local` as a whole, and past a cap a mint is refused (`TICKET_LIMIT`, exit 5) rather than evicting another principal's ticket. Devices get tickets through DPoP in 5B.

### 4.4 The middleware chain

Every request passes the same stages, in order, in one place (`chain/chain.ts` `withChain`, Decision 14), for unary and stream RPCs alike:

1. **Decode** (protocol, §4.3): `INVALID_REQUEST` on failure.
2. **Authenticate:** the connection's principal, re-checked on every request (`ConnectionRegistry.useLiveness`: a revoked or expired credential is `anonymous` from that moment and the chain answers `CREDENTIAL_REVOKED`/`CREDENTIAL_EXPIRED`); anything but `public` RPCs from `anonymous` is `UNAUTHENTICATED`. A credential's request is noted for `lastUsedAt`.
3. **Resolve** (`chain/resolve.ts`, Decision 15): the RPC's projects; `project.open.dir` → `{projectId, dir, open here?}` from `finisher.project.json` (opened `O_NOFOLLOW | O_NONBLOCK`, a regular file of at most 64 KiB); `project.create.dir` → canonical `{dir, parent}`; `asset.probe.path` and `project.recover.importDir` → canonical absolute paths, canonicalised through their nearest existing ancestor (`INVALID_PATH` for relative or non-normalised input, §4.8). The handler gets the resolved values.
4. **Authorise** (`chain/authorize.ts`): **As built**, in this order (review of Task 5, L1): human-only first, then every scope the RPC requires (**all of them**, §4.5), then **reach** (the project must be in `principal.projects`, else `PROJECT_NOT_FOUND`, so ids don't leak; a resolved path outside the principal's roots is `PATH_NOT_ALLOWED`), then the escalations its resolved facts add, so the order of answers leaks no open state. Roots of a principal with project limits are its projects' folders: open ones, and indexed ones (`app.db`) while their `finisher.project.json` still names the project. `project.open` of a folder that holds another project answers like a folder with none.
5. **Policy** (`chain/policy.ts`, Decision 16): **Deviation:** branch references (`branch`, `from`, `into`: a name, an id or `name#id`) are resolved here, **after** authorisation, not at stage 3: resolving needs the project's engine, which an unauthorised caller must not get opened, and only the policy reads them. Each is resolved through the store (`engine.resolveBranch`) to ids, so a name, an alias or a rename cannot slip past a rule written for an id. Rules answer `allow | deny | ask`. 5A ships **agent branch limits**: for `agent`/`mcp` principals with `branches` other than `"*"`, a write whose resolved target is outside them is `BRANCH_NOT_ALLOWED`; `"own"` = branches whose `branch` record's author (`createdBy`) is `<kind>:<id>`. **As built** (review of Tasks 8–9, M1): the server stamps `author = <kind>:<id>` on `branch.create` and `project.recover` for every credential principal, limited or not, so an unlimited agent cannot plant an `own` branch for another. Rules declare `writes` (payload fields naming a write target) and `targets` (branches acted on without a guarded write: `project.acknowledgeRewind`). **`ask`** answers `APPROVAL_REQUIRED {approvalId}` (exit 7) with an approval kept in memory: at most 256, 16 per principal (its own oldest goes first), each for 10 minutes; no 5A rule asks, a test rule covers it, and sub-project 7 owns real approvals.
6. **Confirmation** (Decision 30, §4.7): a call that needs a human-only scope from a `console` principal needs its connection's typed code, unless its rule opts out with `unconfirmed: readOnly | service`.
7. **Handler,** with the resolved values, never the raw ones. **State-dependent rules are checked again inside the store's write transaction** (Decision 17): every write call into `fin-node` (`commit`, every 4B history and branch call, `recover` when it writes) carries a guard `{denyBranches, allowBranches?, credential?}` in its `metaJson`; `fin-store` checks the resolved target in `WriteCtx::target`, inside `Store::write`'s IMMEDIATE transaction, before planning, and refuses with `BRANCH_NOT_ALLOWED` / `BRANCH_PROTECTED` (5E) writing nothing. In 5A `denyBranches` is empty. **As built** (review of Task 5, H1): every write of a branch-limited principal carries `allowBranches` (`[]` allowed, `null` refused), and a write without the chain's guard is a defect. **`credential`: Deviation** (added by the review of Task 7, M3): every `agent`/`mcp` write names `<kind>:<id>`, and `Store::write` checks it under a process-wide read lock held through the SQLite commit; `fin_store::end_credential` takes the write lock, so a write past the check commits before a revocation returns and every later one is `CREDENTIAL_REVOKED` inside its transaction. Paths: for a principal with roots, `asset.probe` opens the file from its root's folder through no symlink (`fin-probe` `probe_beneath`: Linux `openat2` `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS` with a per-component `O_NOFOLLOW` walk as fallback, also on macOS; Windows no reparse point and a final path inside the root), so a dangling symlink or one swapped in after the check is `PATH_NOT_ALLOWED` and nothing of its target is read. Every probe opens without blocking and refuses anything but a regular file (`ASSET_UNREADABLE {reason: NotAFile}`), so a FIFO cannot hold a libuv thread. `Engine.open(…, {expectId})` refuses a folder that no longer holds the checked project before it rebuilds, locks or leases anything.
8. **After:** `project.open` by a principal with project limits must open the project its folder named; sub-project 7 adds `PostCommand` hooks here.

A denial at stages 2–6 is audited as `auth.denied {rpc, rule, scope|scopes, reason, transport}` (aggregated per principal, code and RPC per minute) and counted in `rpc.stats`, never with the payload.

### 4.5 Scopes

A closed vocabulary in `@finisher/contracts` (`scopes.ts`). **`RPC_SCOPES`** (Decision 18) is a mapped type over `FinRpcTag`, so an RPC without a rule is a type error, and a test compares its keys with `FinRpcs`; each rule lists its scopes (all required), its resolvers, its escalations (typed by the RPC's payload), `writes`/`targets`, `confirm`, `unconfirmed` and `deviceOnly`. The job RPCs have their rules since 5C-2 (§6.7); 7A-2 adds the agent ones, each with a rule. The table below is as built.

| Scope | Covers | Human-only |
|---|---|---|
| `public` | `server.hello`, `auth.prove` | — |
| `server.read` | `auth.ticket`, `auth.whoami`, `auth.revokeSelf`, `env.describe`, `fonts.catalog`, `fonts.status`, `server.status` | — |
| `project.read` | `snapshot.get`, `ops.subscribe`, `project.events`, `project.status`, `project.verify`, `tx.get`, `branch.list`, `eval.*`, `fonts.plan`, **`project.open` of a project already open here** (attaching); 5E `review.list/get` | — |
| `project.create` | `project.create` | — |
| `project.write` | **`project.open` that loads a project** (it may take the lease; also serving a project that is not open, `RuleFacts.reopens`), `asset.probe {projectId, path}`, `tx.commit`, `branch.create`, `branch.set`, `history.*`, `project.acknowledge*` | — |
| `project.admin` | `project.rebuild`, `project.compact`, `project.recover`, `project.takeover` | — |
| `project.confirm` | escalation: `project.takeover {confirm: true}`, `project.open {relocate: true}` | **human** |
| `fonts.fetch` | `fonts.fetch` (network downloads from the pinned catalog) | — |
| `branch.protected` | escalation (5E): a write whose resolved target is protected | — |
| `review.write`, `review.decide` | 5E (§8.6) | — |
| `jobs.read`, `jobs.submit`, `jobs.control` | the `jobs.*` RPCs (5C-2, §6.7): read, submit and act on one's own jobs, act on every owner's | — |
| `library.read`, `library.scan` | 5D (§7.6) | — |
| `library.admin` | `library.add`, `library.remove`, `library.set` of hydration, bandwidth, aliases, an accepted volume, a removed nested mount (`acceptUnmounted`) or an empty root (`acceptEmptyRoot`) | **human** |
| `diagnostics.read` | `diagnostics.status`, `diagnostics.export` without content, `diagnostics.project {collect: never}` | — |
| `diagnostics.record` | `diagnostics.record` (it writes the journal) | — |
| `diagnostics.admin` | `diagnostics.consent`, `diagnostics.purge`, `diagnostics.project {collect: default}`, escalation: `diagnostics.export {content: true}` | **human** (training on: `device` only, §3.4) |
| `settings.admin` | `settings.write` (5B) | **human** |
| `auth.admin` | `console.challenge`, `auth.credential.*`, `auth.audit.*`, `server.stop`, pairing and devices (5B), `env.*` writes | **human** |
| `plugin.read`, `plugin.suggest` | sub-project 8A: plugin and registry reads, refreshes and plans; suggestions (with `project.read`) | — |
| `plugin.install`, `plugin.admin` | sub-project 8A: install, update and removal; enable and disable | **human** |

**Default scopes per principal** (Decision 20, as built):

| Principal | Scopes |
|---|---|
| `anonymous` | `public` |
| `console`, `device` | all (training on still `device` only) |
| `local` | all but the human-only rows; it keeps `branch.protected` and `review.decide` (owner decision 5) |
| `agent`, `mcp` | exactly the credential's scopes, never a human-only one (a mint that asks for one is `SCOPE_DENIED` naming them). The mint defaults (`MINT_DEFAULT_SCOPES`, Decision 33) are `server.read`, `project.read`, `project.write`, `review.write`, `jobs.read`, `jobs.submit`, `library.read`, `library.scan`, `diagnostics.read`, `diagnostics.record`; never `branch.protected`, `review.decide`, `jobs.control`, `project.admin`, `project.create` or `fonts.fetch` unless asked for. No credential carries the right to auto-merge: the `auto` merge policy is a server function (§8.5) |

- **Two scopes where an RPC does two things:** `jobs.submit` of a `library.scan` job needs `jobs.submit` **and** `library.scan` (5C-2); `review.merge` into a protected branch needs `review.decide` only; `history.drop` into a protected branch needs `review.decide` (5E).
- **Exports carry content only on request: Deviation** (Decision 22 redacted only non-human exports; owner decision on the review of Task 7, L4): `diagnostics.export` without `content: true` is redacted for **every** principal, a person included (content fields and their blobs removed, entries of kinds or kind versions this build does not know dropped, as the upload view does; `redacted: true`). `content: true` escalates to `diagnostics.admin`, so only a person reads content; the console's `diagnostics export --content` needs no typed code (it changes nothing).
- **`auth.revokeSelf {}`** revokes the caller's own credential through the one revoke path and closes its connection after the reply; it takes no id, so it can never name another principal (`local`/`console`: `INVALID_REQUEST`).
- **Training on** (Decision 21) is refused for every principal but a `device` with `CONSENT_UI_ONLY` (exit 2), before the human check, the console included (rule `deviceOnly`).

### 4.6 Human-only operations

These are refused with **`HUMAN_ONLY`** (exit 7) for every non-human principal, at stage 4, with a fix that addresses the user (§3.5). **The owner's interface for them is the TTY-gated console** (owner decision 2, §4.7) until sub-project 9's app.

| Operation | Why |
|---|---|
| any consent change; **training on only from a `device`** (onboarding) | arch §21.1, D14b |
| export of recorded content | it is the user's content |
| `diagnostics.purge` (every selector) | destroys data |
| `diagnostics.project {collect: default}` (setting `never` stays open) | clearing a never-collect flag binds collaborators |
| `project.takeover {confirm: true}` | arch §6.7: "the user confirms" |
| `project.open {relocate: true}` | "a person's decision; never set it automatically" (4A) |
| `library.add`, `library.remove`, `library.set` of hydration or bandwidth | grants read access to a tree; downloads |
| credential mint, list and revoke (beyond `revokeSelf`), `rotateMaster`, pairing, devices, audit read and purge, `server.stop`, environment commands | account takeover vectors |
| settings writes through RPC (5B) | opens the server to the network |
| `review.policy` writes (5E): protected branches, `requireReview`, auto merge | decides whether an agent can reach `main` without a person (§8.5) |
| plugin install, update, enable and removal (sub-project 8) | arch §8.3 |

**Before sub-project 7's sandbox (7C), this binds agents that use `fin` or MCP, not code in a raw shell.** The owner accepted this (decision 6), and the README's security model says it plainly. **An unsandboxed agent harness runs as the owner**, so it can read `auth/console.key` and `auth/local.key`, hand-edit `settings.json`, drive a pseudo-terminal through the console's confirmation, and make links or files anywhere the owner can. **Until sub-project 7's sandbox exists, agent and MCP scopes, project and branch limits constrain only well-behaved harnesses** that use `fin` or MCP with their credential file; they are attribution and a guard against mistakes, not a boundary against hostile code. Agent and MCP credentials (`finisherd agent mint`, `finisherd mcp setup`) are offered as the recommended way to run a harness by hand, not forced, so `fin` run by a person needs no mint. The mechanism is complete in 5A; sub-project 7 makes it hold by denying agents read access to `<server-data>/auth/`.

### 4.7 The console (`finisherd <command>`)

- **Dispatch** (Decision 29): `finisherd` alone, `finisherd serve` or flags only serves. Any other first word is a console command (`src/console/`) that finds the server through `finisherd.json`, checks the server's proof with `console.key` first (a squatted socket is `SERVER_UNTRUSTED`), proves as `console` and prints text for a person (`--json` for read-only commands). Errors print `finisherd: <CODE>: <message>` and the fix, with `fin`'s exit codes (`exit-codes.ts`, checked against `crates/fin/src/output.rs` by a test).
- **Read-only and service commands run anywhere:** `status [--json]`, `audit [--since] [--kind] [--limit] [--json]`, `agent ls [--all] [--json]`, `stop` (`server.stop`, the graceful stop Windows lacked; service managers need it non-interactively, and a same-user process can kill the server anyway).
- **Every mutating human-only command needs an interactive terminal and a typed code** (Decision 30). stdin and stdout must be TTYs, checked before anything connects (`CONSOLE_NEEDS_TTY`, exit 7; not audited, the server never hears of it). The command says what will happen, fetches `console.challenge` on its own connection (a 6-character Crockford code valid 60 s, kept as SHA-256 on that connection, at most 4 live), shows it, and sends the call with `challenge: <typed code>` once the person types it; typed codes are read the Crockford way (case, `I`/`L`/`O`). A code is used by the call it confirms; a wrong answer voids all of the connection's codes; after `wrong` or `expired` the console asks again with a new code, up to 3 tries; nothing typed cancels (`CONFIRMATION_REQUIRED {reason: cancelled}`). The server refuses a confirmed call from a `console` principal without its connection's live code (`CONFIRMATION_REQUIRED {reason: missing|wrong|expired}`, exit 7, audited `console.refused`). Prompts go through an injected `Prompt` so tests drive them without a terminal.
- **Confirmation by default: Deviation** (the plan listed the confirmed RPCs; owner decision on the review of Task 7, L2): every call that needs a human-only scope needs the typed code unless its rule opts out with `unconfirmed: readOnly` (`console.challenge`, `diagnostics.export`, `auth.credential.list`, `auth.audit.list`) or `unconfirmed: service` (`server.stop`). **`server.stop` needs no code from the console only** (owner decision on the review of Tasks 8–9): any other human principal (5B's devices with `auth.admin`) must confirm it, and until 5B gives devices a way to it is `CONFIRMATION_REQUIRED`.
- **What the typed code protects against:** `yes |` pipes, pasted scripts and an agent's `fin` (which cannot reach these calls at all). **What it does not:** a same-user process that can read `<server-data>/auth/` (it proves as `console` itself) or drives a pseudo-terminal (`script`, `expect` pass the terminal check). Sub-project 7 closes both (owner decision 6, §4.6).
- **Training cannot be turned on from the console** (§3.4): `finisherd diagnostics consent training on` is refused locally with the server's `CONSENT_UI_ONLY` (exit 2), before any code; the help offers only `off|unset`.
- **5A commands, as built:** `status`, `stop`, `audit`, `audit purge (--all | --before <date>)`, `diagnostics consent diagnostics <on|off|unset>`, `diagnostics consent training <off|unset>`, `diagnostics <on|off|unset>`, `diagnostics purge (--all | --project <id> | --before <date>)`, `diagnostics project default <projectId>`, `diagnostics export --content [--kind]… [--since] [--project] [--limit]` (JSONL on stdout), `project takeover <dir|projectId> --confirm`, `project open <dir> --relocate`, `agent mint --label <name> [--scope]… [--project <id>]… [--branch own|<id>]… [--idle <d>] [--absolute <d>|none] [--out <file>]`, `agent ls`, `agent revoke <id>`, `mcp setup --out <file> [--label] [--scope]…`, `auth rotate-master`. **Deviation:** `diagnostics consent diagnostics on` is offered (turning basic diagnostics back on is a person's choice like turning it off; only training on is device-only), and `auth rotate-master` is new (§4.1). The mint's confirmation names what it grants; with no `--project` it says "ALL projects (*): every project on this server, and any folder path…". Sub-project 8 adds `plugin …` and `registry …`; 5B adds `pair`, `devices`, `revoke`, `remote …`, `tls show|rotate`, `env show|name|adopt|reset`; 5D adds `library add|rm|set`; 5E adds `review policy …`.
- **Output** is English, like `fin` (arch §17.1).

### 4.8 RPCs and `fin` (5A)

| RPC | Scopes | Payload → result |
|---|---|---|
| `server.hello` (changed) | `public` | `{clientNonce?, env?, endpoint?, credential?}` → adds `envId`, `endpoint`, `serverNonce`, `proof?` |
| `auth.prove` | `public` | `{mac}` → `{principal}` |
| `auth.ticket` | `server.read` | `{ttlS?}` → `{ticket, expiresAt}` |
| `auth.whoami`, `auth.revokeSelf` | `server.read` | → `{principal}` / `{}` |
| `auth.credential.mint` | `auth.admin`, confirmed | `{kind: agent\|mcp, label, scopes?, projects?, branches?, idleS?, absoluteS?}` → `{credential, file}` (the file contents; shown once). Labels 1–64 characters, no control characters; `projects` ids (default `"*"`); `branches` `own` or ids or names (default `"*"`) |
| `auth.credential.list` | `auth.admin`, read-only | `{all?}` → the live credentials, or all rows still kept |
| `auth.credential.revoke` | `auth.admin`, confirmed | `{id}` → `{revoked}` (`false` for one already ended) |
| `auth.credential.rotateMaster` | `auth.admin`, confirmed | `{}` → `{ended}` (§4.1) |
| `auth.audit.list`, `auth.audit.purge` | `auth.admin` | list (read-only): `{since?, kind?, cursor?, limit ≤ 1000}` → `{entries, next?}`; purge (confirmed): `{all: true} \| {before}` → `{removed}`, audited `audit.purge {reason, count, before}` in the same transaction; `audit.purge` rows are never purged |
| `console.challenge` | `auth.admin`, read-only | `{}` → `{code, expiresAt}` (console principals only; `INVALID_REQUEST` otherwise) |
| `server.status` | `server.read` | → `{version, protocol, envId, pid, listeners, connections: {kind: count}, journal, auditFailures, jobs: {caps}, warnings}` (5C-2: `jobs.caps` and the `SANDBOX_DISABLED` warning, §6.7) |
| `server.stop` | `auth.admin`, service | → `{stopping: true}`; once that reply is written (`Link.afterReply`) `serve` returns and its finalizers stop everything (exit 0) |

The payloads of the confirmed RPCs (`project.open`, `project.takeover`, `diagnostics.consent|purge|project` and the `auth.*` writes) take `challenge`. **`INVALID_PATH`** (Decision 35, Task 10): `project.create.dir`, `project.open.dir`, `asset.probe.path` and `project.recover.importDir` must be absolute and normalised; relative paths, `.`/`..`, doubled separators, Windows drive-relative `C:foo` and rooted-without-drive `\foo`, the device namespaces (`\\.\`, `\\?\` other than `\\?\C:\` and `\\?\UNC\`), a `:` after the drive (streams), reserved device names (`CON`, `NUL.txt`, `COM1`, …) and segments ending in a dot or a space answer `INVALID_PATH {field, path}` (exit 3).

**What a revocation stops, as built** (review of Tasks 8–9, L1). Revocation (by the owner, by the credential itself, or by expiry) is one path (`auth/revoke.ts`): the row in memory, every transport's per-request refusal, fin-store's in-flight writes (`endCredential`), the credential's tickets, then every connection it proved on every transport, ended with `CREDENTIAL_REVOKED`, which ends its streams (a live `ops.subscribe` ends within 100 ms in the tests). From that moment the credential proves nothing and every next request of it is refused. **Nothing it sent lands afterwards** for every store write (`tx.commit`, the branch and history RPCs, `project.recover`) and for `project.acknowledgeRewind`, `project.acknowledgeSalvage` and `diagnostics.project`, which hold the credential's check through their change (`fin_store::live_credential`); the revocation waits for those already past it. Other calls already past the chain's check may still finish: the long maintenance calls (`project.rebuild`, `compact`, `takeover`), `project.create`/`open`, `asset.probe`, `fonts.fetch`, library scans, job submissions, plugin plans and refreshes, `diagnostics.record`. The wait runs on the server's main thread, so revoking while a long commit is in flight stalls the server until it lands (§4.17).

**`fin` changes:** the handshake (§4.2); exit class 7; `fin whoami`; `fin asset add` and `fin asset probe` send `projectId`; paths normalised before they are sent (`..` resolved through the file system on its deepest existing prefix); a server line over 16 MiB is `PROTOCOL_ERROR {max}`; **`FIN_DEADLINE_S`** (default 300 s, `0` off; streams and `fonts fetch` keep their own timeouts) bounds a run with `SERVER_TIMEOUT {deadline}` (Decision 36); `FIN_OFFLINE_SNAPSHOT`, `FIN_TEST_COMMIT_GATE` and `FIN_TEST_AUTH_GATE` are read only with `fin`'s `test-hooks` feature, which its own tests enable through a self dev-dependency, and `scripts/check-test-hooks.mjs` keeps release builds free of them (Decision 37). `fin diagnostics consent|purge|project default|export --content`, `fin project takeover --confirm` and `fin project open --relocate` reach the server and answer `HUMAN_ONLY`; `fin diagnostics consent training on` stays a local usage error (exit 2, Decision 28).

### 4.9 Storage

`<server-data>/auth/{local.key, console.key, credential.master}`, `<server-data>/run/fin.sock`; `control.db` chains `core` (`meta`, `migrations`, `server_ids`, `audit`) and `auth` (`credentials`). Nothing in project folders.

### 4.10 Security and threat model (5A)

Arch §16 priority: injected agent > malicious plugin > stolen device > network attacker.

| Threat | Mitigation | Residual |
|---|---|---|
| Injected agent runs `fin diagnostics consent training on`, `purge --all`, `takeover --confirm` | `HUMAN_ONLY` for every non-human principal at stage 4; training on is device-only | a raw-shell agent before sub-project 7 can read `console.key` (accepted and documented, owner decision 6) |
| Injected agent is handed a fix text and runs it | fixes never contain a runnable command (§3.5); console mutations need a TTY and a typed code | a PTY-driving agent before sub-project 7 |
| Injected agent opts the user in to training through the console | refused: `CONSENT_UI_ONLY` from every principal but a device | — |
| Injected agent reads `finisherd.json` to become `ui` | no token in the file; a ticket carries its minter's principal | — |
| Another OS user squats the pipe, or opens it | native listener: an anchor first instance, explicit DACL, remote clients rejected, peer user, integrity and AppContainer checked before the first byte; `fin` connects at the identification level; the server proof | denial of service only |
| A squatter re-creates a dead socket that a stale `finisherd.json` names | `fin` and the console check the server's proof before sending anything; one retry, then `SERVER_UNTRUSTED` | — |
| Proof relayed to another server or socket | envId and canonical endpoint inside both MACs | — |
| Sandboxed agent `connect()`s to the socket | no proof without a key → `UNAUTHENTICATED`; its credential gives only its scopes | — |
| A copied `credential.master` or an old `control.db` | credential keys bind `envId`; `rotate-master` ends every credential | restoring an old `control.db` beside the same master revives credentials revoked since (§4.1) |
| A credential revoked while its write is in flight | the guard's `credential` is checked under a lock held through the commit | the calls outside the covered paths (§4.8) |
| A rule written for a branch id bypassed by a name, alias or rename | references resolved by the store before the policy; guards re-checked in the write transaction | — |
| A probe follows a symlink or blocks on a FIFO | `probe_beneath` (no symlink, beneath the root), non-blocking open of regular files only | a mount point or hard link inside a project folder (§4.17) |
| A client claims another actor | the principal bounds the actor (§3.4); `x-fin-` headers stripped | `local` can still claim `agent` or `user` (as in D6) |
| Audit or connection flood | `auth.fail` and `auth.denied` aggregated per minute; 5 failed proofs close a connection; ticket and WebSocket caps (§4.3) | a local process reopening sockets faster than the 1 s grace holds the 16 pre-upgrade slots (§4.17) |

### 4.11 Cross-platform (5A)

- **Windows server data:** `ensureServerData` calls `fin-node` `checkPrivateDir(path)`: the owner is the current user, and the DACL allows nothing to `Everyone`, `Users`, `Authenticated Users` or another user (SYSTEM and Administrators may stay). A directory `finisherd` creates gets a protected owner-only DACL (`makePrivateDir`); an existing one that fails is `SERVER_DATA_UNSAFE`, whose fix says why.
- **Windows pipe:** the native listener (§4.3); graceful stop through `finisherd stop`.
- **Key files** are written temp-then-rename inside the private directory (the temp file inherits its DACL); rename-over retries sharing violations.
- **As built, run so far:** the pipe listener's Rust tests (`crates/fin-node/tests/pipe_server.rs`: squatting, the remote-rejection flag and the DACL read back, a deny-only-user token refused, peers, a graceful close of 1 MiB, the cap, shutdown, a real AppContainer refused and then allowed) ran on Windows 11 during Task 11. Everything else of 5A on Windows (private server data under a second account, `fin`'s DACL check of a `FIN_CREDENTIAL` file, the console's `--out` file, the finisherd pipe tests, `finisherd stop`, the e2e `auth` scenario) is compiled and linted for `x86_64-pc-windows-gnu` but not yet run, and nothing of 5A has run on macOS (Time Machine exclusion included): 5A Task 14.
- **Windows 11 run (2026-11-08, 5A Task 14, Windows half; integration `3897add` plus `xwin-fixes`; a non-elevated, medium-integrity session in the console session; Defender real-time protection on, no exclusions):** `cargo test --workspace` with `FIN_REQUIRE_SERVER=1`, `pnpm test`, every `pnpm e2e` scenario (`auth` included) and clippy with warnings denied. The pipe listener tests (DACL read back, remote clients refused, squatting, a deny-only token kept out, AppContainer refused then allowed, the instance cap), `fin`'s identification-level client, the squatted-pipe and stale-`finisherd.json` tests (`cli_auth`), the WebSocket ticket and Host/Origin refusals and `pipe.test.ts` pass. **New test:** `fin` refuses a `FIN_CREDENTIAL` file whose DACL admits Users or Everyone (`credential_files_must_be_private_windows`; it had none). **`node:sqlite` under Electron 44.9.3** (`ELECTRON_RUN_AS_NODE=1`: Node 24.22.0, SQLite 3.53.4): `control.test.ts` 13 of 13. Control-store statement p99 with 100,000 audit rows: ≤ 0.28 ms (`control-budget.ts`). **Not done:** a second account (none with a known password on the host; the deny-only-token and AppContainer tests stand in), `finisherd stop` and private server data by hand, the handshake's added p50/p99 to `fin view` and the checkpoint worker's WAL size; macOS.
- **Sandbox reachability** (sub-project 7 applies these; 5A fixes the layout they depend on):
  - **macOS Seatbelt:** `(allow network-outbound (remote unix-socket (path-literal "<server-data>/run/fin.sock")))` plus `file-read-metadata` on the path's ancestors; everything else under server data denied;
  - **Linux** (kernel ≥ 6.2, the platform floor, so Landlock ABI ≥ 3 wherever Landlock is enabled): Landlock does not mediate `connect()` on pathname sockets at any ABI (ABI 6 scopes abstract sockets only), and sub-project 7's seccomp filter refuses `AF_UNIX` for agents, so a sandboxed `fin` reaches `finisherd` over sub-project 7's credentialed loopback agent listener (agent §8.3), never `fin.sock`; server data stays outside every granted tree. There is no bubblewrap fallback (agent §8.3). A host where Landlock is disabled cannot sandbox CLI agents (agent §8.9);
  - **Windows AppContainer:** the pipe's DACL grants the project's AppContainer SID (`finisher-<project-uuid>`) `0x12008b` through `PipeServer.allow(sid)` (§4.3); `auth/` stays owner-only, which an AppContainer cannot read;
  - in every sandbox, `fin` proves with `FIN_CREDENTIAL` (a file inside the sandbox), never with `local.key`.
- **Clocks:** nothing in 5A compares clocks; TTLs use the server's monotonic clock (tests inject a clock).

### 4.12 Journal (5A)

As built (Task 7, Decision 27): `fin_core::events::Source` gains `console` and `mcp` (`mcp` may record what `fin` records, `cli.command` and `error.panic`; `console` nothing); D4's per-transport `RpcSource` is gone: `sourceOf(principal)` (`local`/`agent` → `fin`, `mcp` → `mcp`, `console` → `console`, `device` → `ui`) and `actorFor(principal, headers)` (§3.4); one run id per connection across transports; the `diagnostics.record` rate limit per principal (`local` per connection under 200/s, burst 800, for `local` as a whole; at most 1,024 buckets); new `RPC_METHODS` entries for every new RPC and `CLI_FLAGS` += `content`. Consent changes and purges are audited (`consent.change {purpose, value}`, `diagnostics.purge {reason: all|project|before}`). An `agent` credential records `cli.command` only with `diagnostics.record`; its `log.commit` entries carry its credential id as `actor.session` (keyed in the journal).

### 4.13 Error codes (5A)

| Exit | Codes |
|---|---|
| 2 | `FRAME_TOO_LARGE`, `CONSENT_UI_ONLY` (now also from the console), `USAGE` (the console's own, e.g. a `--out` file that would not be private) |
| 3 | `INVALID_REQUEST`, `INVALID_PATH` |
| 4 | `CREDENTIAL_LIMIT` (**Deviation**, owner decision: not 5; the server's state refuses it and waiting changes nothing, and not 7, since only the owner mints: revoke some, then retry) |
| 5 | `CONTROL_DB_BUSY`, `TICKET_LIMIT`, `SERVER_TIMEOUT` |
| 6 | `PROTOCOL_ERROR` (`fin`'s frame cap) |
| 7 | `UNAUTHENTICATED`, `SERVER_UNTRUSTED`, `SCOPE_DENIED`, `HUMAN_ONLY`, `PATH_NOT_ALLOWED`, `BRANCH_NOT_ALLOWED`, `BRANCH_PROTECTED` (5E), `CREDENTIAL_REVOKED`, `CREDENTIAL_EXPIRED` (**new**, owner decision: revoked and expired credentials get codes of their own), `APPROVAL_REQUIRED`, `CONSOLE_NEEDS_TTY`, `CONFIRMATION_REQUIRED` |
| start (stderr, exit 1) | `CONTROL_DB_TOO_NEW`, `SERVER_DATA_UNSAFE` (now also on Windows), `PIPE_SQUATTED`, `KEY_FILE_INVALID` |

### 4.14 Testing (5A)

- **Shared vectors:** `tests/fixtures/auth/handshake.json` (fake keys, nonces, envIds, endpoint → proofs, MACs and credential keys), read by a vitest, a Rust test and `scripts/e2e.test.mjs`.
- **Unit (vitest):** every RPC has a rule (`rpc_scopes_cover_every_rpc`); the human-only matrix over every principal kind and escalation; training on refused for `console`, `local`, `agent`, `mcp`; the chain's order; resolvers (branch names, renames between resolve and write, directories, symlinked paths); `ask` and its caps; tickets (one use, TTL, caps, `Sec-WebSocket-Protocol`, never the query); Host and Origin; WebSocket limits; fix texts carry no command; the console commands and their confirmations through an injected `Prompt`; credentials with an injected clock (mint, expiry, revocation of live streams, `own` branches, limits, the review cases in `credentials-review.test.ts`); control store (migrations, `CONTROL_DB_TOO_NEW`, corruption, no autocheckpoint on the main connection, batched deletes, the statement budget: median ≤ 1 ms and p99 ≤ 50 ms under a full-suite load, p99 ≤ 5 ms with `FIN_STRICT_TIMING=1`); audit aggregation; the pipe protocol over a fake native listener on every platform.
- **Rust (`fin`, `fin-node`, `fin-store`):** `SERVER_UNTRUSTED` against a fake server; the restart retry (`keyStale`); `FIN_CREDENTIAL` and revoked or unknown credentials (`cli_auth.rs`); exit 7; the write guard and an ended credential refused inside the transaction (`fin-store` `tests/guard.rs`); the Windows pipe listener (§4.11).
- **e2e `auth` scenario** (`scripts/e2e/auth.mjs`, Task 12): an unproved connection; `fin` as `local`; the human-only refusals and the console's codes; a ticketed WebSocket and its refusals; a read-only and a `branch own` writer credential (scope, branch and human-only denials, attribution, `TICKET_LIMIT`, revocation ending its stream and its next `fin`); a squatted socket after a stale `finisherd.json`; a restart with key rotation and an expiry; the audit rows; and a **leak grep** of every key, nonce, proof, MAC, ticket and code the harness handled over `finisherd.json`, `events/`, `control.db` and its WAL, the exports, the server's stderr and every output: no hit.
- **Windows and macOS:** 5A Task 14 (§4.17).

### 4.15 The plan's decisions, as built

The numbers are the **plan's** Decision numbers, not §11's. **As built** = built as decided, with the detail in the section named; **Deviation** = built differently, with the reason there.

| # | Decision | Mark | Section |
|---|---|---|---|
| 1 | `control.db` on `node:sqlite`, WAL, `busy_timeout` 250 (`CONTROL_DB_BUSY`), no autocheckpoint on the main connection, a checkpoint worker | As built | §3.1 |
| 2 | Per-area migration chains, `VACUUM INTO` backups, `CONTROL_DB_TOO_NEW` | As built; a schema this SQLite cannot parse also refuses the start (`CONTROL_DB_TOO_NEW`) instead of a reset | §3.1 |
| 3 | `quick_check` at open; damage → `control.db.corrupt-<ulid>`, `control.reset` | As built; only `quick_check` damage or a file that is not SQLite resets | §3.1 |
| 4 | The audit table, aggregation, retention in batches of ≤ 500 | As built; every kind's `details` is a closed schema | §3.3 |
| 5 | `auth/` and `run/`, keys rewritten before any listener | As built (after the store opens, before the socket is claimed; fsync of file and directory); Time Machine exclusion and `KEY_FILE_INVALID` added | §4.1 |
| 6 | The MACs and `credentialKey` | **Deviation:** `credentialKey` is `fin/credential-key/v2` and binds `envId` | §4.1, §4.2 |
| 7 | The canonical endpoint | As built | §4.2 |
| 8 | `server.hello`; no proof for an unknown, expired or revoked credential | **Deviation:** an ended credential gets the proof, then `CREDENTIAL_REVOKED`/`CREDENTIAL_EXPIRED` on prove | §4.2 |
| 9 | `auth.prove`, one per hello, five failures close | As built | §4.2 |
| 10 | `fin`'s one retry, `SERVER_UNTRUSTED` | As built, with `helloFailed` and `versionMismatch` | §4.2 |
| 11 | Credential discovery; no credential is none | As built, as amended: no credential sends nothing | §4.2 |
| 12 | Connection ids from Effect RPC | **Deviation:** finisherd's own NDJSON protocol over Effect's codec; `Connections` per transport | §4.3 |
| 13 | Decoding at the protocol level; 16 MiB frames; `x-fin-` dropped | As built | §4.3 |
| 14 | One chain for unary and stream RPCs | As built, with a confirmation stage | §4.4 |
| 15 | Resolvers | As built; **Deviation:** branch references resolve in the policy stage, after authorisation | §4.4 |
| 16 | Policy, agent branch limits, `ask` | As built; authors stamped for every credential principal; approvals capped | §4.4 |
| 17 | The write guard inside the IMMEDIATE transaction | As built; **Deviation:** the guard also names the credential, checked under a lock held through the commit | §4.4 |
| 18 | `RPC_SCOPES` as a mapped type | As built, with `writes`, `targets`, `confirm`, `unconfirmed`, `deviceOnly` | §4.5 |
| 19 | The scope table | As built | §4.5 |
| 20 | Default scopes per principal | As built | §4.5 |
| 21 | Escalations; training on device-only | As built | §4.5 |
| 22 | Non-human exports redacted | **Deviation:** every export is redacted unless it asks for content | §4.5 |
| 23 | Tickets | As built; **Deviation:** per-principal caps and `TICKET_LIMIT` instead of evicting the oldest | §4.3 |
| 24 | The upgrade checks | As built, with header timeout and connection caps | §4.3 |
| 25 | `finisherd.json` without a token | As built | §4.1 |
| 26 | Protocol 2 | As built; RPC fixtures re-captured | §3.6 |
| 27 | Sources and actors from principals | As built | §4.12 |
| 28 | `fin` training on stays a usage error; the rest answers `HUMAN_ONLY` | As built | §4.8 |
| 29 | Console dispatch | As built, with `agent`, `mcp`, `auth` (and 8A's `plugin`, `registry`) | §4.7 |
| 30 | TTY and a typed, per-connection challenge | As built; **Deviation:** confirmation by default for every human-only call, with opt-outs | §4.7 |
| 31 | Console training on → `CONSENT_UI_ONLY` | As built (refused locally, before any code) | §4.7 |
| 32 | `credentials`, expiry, revocation closes connections | As built, with `revokedReason`, `CREDENTIAL_LIMIT` (exit 4) and in-memory-first revocation | §4.1 |
| 33 | Mint defaults; human-only scopes refused | As built | §4.5 |
| 34 | `FIN_CREDENTIAL` in `childEnv` | As built | §4.1 |
| 35 | `INVALID_PATH` | As built, with Windows device namespaces, streams and reserved names | §4.8 |
| 36 | `fin` deadline and frame cap | As built | §4.8 |
| 37 | `fin`'s test hooks behind a feature | As built (`FIN_NODE_BINARY_DIR` stays sub-project 9's) | §4.8 |
| 38 | The Windows pipe listener | As built, with the anchor instance; **Deviation:** allowed SIDs get `0x12008b` (not `GRGW`) and a low mandatory label | §4.3 |

### 4.16 Owner decisions and review fixes

**Owner decisions** (spec §12 decisions 2, 5 and 6, and those taken on the reviews):
- **Human-only actions go through the TTY console** (decision 2) until the app; agents and tests lost `fin diagnostics consent|purge|project default`, `export --content`, `fin project takeover --confirm` and `--relocate` (§4.6, §4.7).
- **`local` keeps `branch.protected` and `review.decide`** (decision 5, §4.5).
- **Human-only is advisory against raw-shell agents until 7C, and documented** (decision 6, §4.6; README "Security model").
- **Exports include content only on request**, for a person too (review of Task 7, L4; §4.5).
- **Confirmation by default:** every call that needs a human-only scope needs the console's typed code unless its rule opts out as read-only or a service call (review of Task 7, L2; §4.7).
- **`server.stop` needs confirmation for non-console callers;** the console's stop stays a service command (review of Tasks 8–9; §4.7).
- **Revoked and expired credentials get clear codes** (`CREDENTIAL_REVOKED`, `CREDENTIAL_EXPIRED`) instead of looking unknown (review of Tasks 8–9, L4; §4.2).
- **`CREDENTIAL_LIMIT` exits 4** (review of Tasks 8–9, L4; §4.13).

**Review fixes** (each landed with tests; the details are in the sections named):
- **Tasks 3 and 4:** `fin` sends nothing without a credential and trusts nothing an unproved server says (`helloFailed`, `versionMismatch`, `credentialUnsafe`, key files read regular-only and non-blocking, no lines read after a flagged close); `Connections.get` never creates (§4.2, §4.3).
- **Task 5:** H1 every branch-limited write carries `allowBranches`; H2 `project.open`/`create` use the canonical folder and `registry.open` re-checks the project; M1 serving a project not open needs `project.write`; M2 canonicalising through the nearest existing ancestor; M3 `finisher.project.json` opened no-follow, non-blocking, ≤ 64 KiB; M4 authors stamped; M5 `auth.denied` aggregated, stream refusals counted; L1 the authorisation order; L2 `allowBranches: null` refused; L5 escalations typed by payload; L7 regular key files only (§4.4, §4.5).
- **Task 6:** non-blocking, regular-file-only probes; `probe_beneath`; one revoke path; ticket caps and `TICKET_LIMIT`; `project.open` of another project's folder answers like an empty one; `expectId`; `targets`; WebSocket header timeout, connection cap and cached origins (§4.3, §4.4).
- **Task 7:** M1 `local` tickets and `diagnostics.record` limited per connection; M2 pre-upgrade and per-owner WebSocket caps; M3 credentials revoked while running stay refused and in-flight writes are refused through the guard's credential; L2 confirmation by default; L3 `probe_beneath` opens its root no-follow; L4 content only on request; L5 unknown kinds dropped from redacted pages (§4.3–4.5, §4.7).
- **Tasks 8 and 9:** M1 authors for every credential principal; M2 private `--out` files, removed and the credential revoked on failure; M3 `envId` in credential keys, Time Machine exclusion and `rotate-master`; L1 the covered paths of "what a revocation stops"; L2 in-memory-first revocation with retried writes; L3 the lifetimes stated; L4 clear codes for ended credentials and `CREDENTIAL_LIMIT` exit 4; L5 per-principal approval caps; L6 the mint's "ALL projects" wording and indexed roots kept only while they still name the project; L7 the raw-shell statement; `audit.purge` rows never purged and written in the purge's transaction; `server.stop` confirmable only from the console; the 1 s grace at the pre-upgrade cap (§4.1–4.8).

### 4.17 Known residuals and follow-ups

**Accepted residuals:**
- **Unsandboxed harnesses can read `console.key` and `local.key`** (and drive a pseudo-terminal through the console's code) until sub-project 7C denies agents `<server-data>/auth/` (owner decision 6, §4.6).
- **Restoring an old `control.db`** beside the same `credential.master` revives the credentials revoked since that copy; `finisherd auth rotate-master` is the remedy (§4.1). Likewise a revocation not yet written when the server stops or crashes (a store busy past its retries) brings the credential back at the next start.
- **The 16 pre-upgrade WebSocket slots are not reserved for anyone:** a local process reopening sockets faster than the 1 s grace blocks new app WebSockets meanwhile (§4.3).
- Calls outside the covered paths that were already past the chain's check may finish after a revocation, and the revocation's wait for writes in flight runs on the main thread (§4.8).
- A Ctrl-C while a mint is in flight can leave a live credential whose file was removed: no one has its key, `agent ls` shows it and `agent revoke` ends it.
- A project evicted between the chain's open-state check and its handler is reopened for a `project.read` caller: reach is checked first and a reopen cannot take another server's lease, so the caller gains nothing it lacked a moment before.
- `probe_beneath` follows a mount point inside a project folder (`RESOLVE_NO_XDEV` is not set) and probes a hard link there to a file outside it; both need the user's own file access in the folder, no RPC makes one, and sub-project 7 must keep it so (no Landlock `REFER`/`MAKE_REG` across the session's trees, no mount syscalls).
- `local` can still claim another actor through D6's headers (§3.4).

**Owed:**
- **5A Task 14, cross-platform validation:** the whole of 5A on macOS and Windows 11 as a non-elevated user (private server data and `SERVER_DATA_UNSAFE`, `PIPE_SQUATTED` and a second account kept off the real pipe, a stale `finisherd.json` naming a squatted pipe, `finisherd stop`, the credential file's DACL, the e2e `auth` scenario); `node:sqlite` under Electron's Node on macOS and Windows; and the measurements: the handshake's added p50/p99 to `fin view` per OS (budget 1 ms), control-store statement p99 per OS (budget 1 ms) and the checkpoint worker's WAL size under the e2e load. **Windows has run** (§4.11, 2026-11-08) except the second-account checks, `finisherd stop` by hand and the two measurements; macOS has not.
- **5B** must make `server.stop` (and every other confirmed call) confirmable from a device with `auth.admin`; until then the chain refuses it.
- **Sub-project 7:** the sandbox rules of §4.11, approvals behind `APPROVAL_REQUIRED`, `childEnv`'s provider variables.
- **Sub-project 9:** `FIN_NODE_BINARY_DIR` out of release builds; the control store's checkpoint worker is loaded from its `.ts` file through Node's type stripping, so a packaged `finisherd` must ship it (or a compiled `.js`) beside `control/db.ts`.

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
- **Pinning** is for the desktop shell and native clients. **Browsers cannot pin:** they reach a remote server only over a route whose certificate they trust (owner-supplied files such as `tailscale cert`, a reverse proxy) or over loopback through an SSH tunnel (owner decision 3). No ACME client, and no route that asks a browser to click through a certificate warning.
- **Rotation is re-pairing** (signed rotation statements are cut, §1.2): `finisherd tls rotate` makes a new identity key (TTY-gated), keeps the `envId`, and every device pairs again.
- **Secret store** (`fin-node`; it also backs sub-project 7's `SecretStore`, which in v1 holds only the user's OpenRouter key): `keyring` 3 pinned exactly in `Cargo.toml` (`=3.x.y`, recorded in the 5B plan) with `default-features = false` and `apple-native`, `windows-native`, `async-secret-service`, `async-io`, `crypto-rust` (the pure-Rust `zbus` Secret Service, also pinned; never the `dbus`/libdbus backend). **When the Secret Service is absent, or present but locked** (no unlock prompt is possible for a server), the key goes to the file store and `server.status` says so; the server never prompts and never blocks its start. File store: `<server-data>/secrets/identity.pem` (0600), optionally wrapped with AES-256-GCM under a key from `FIN_SECRETS_KEY_FILE` (a Docker secret).
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

> **5C-1 (the engine) is built** (Linux x64 verified; macOS and Windows are compiled, linted and, for Windows, run during the tasks, but their full runs wait for 5C-1 Task 14; §6.12). §6.0–6.6 and §6.8–6.11 below are rewritten **as built**. Marks: **As built** is a detail the plan left open or the build fixed; **Deviation** is where the build differs from revision 3 of this design or from the 5C-1 plan. §6.7 (the RPCs, 5C-2) is still a design. The plan is `docs/superpowers/plans/2026-11-07-sub-project-5c1-job-engine.md`; its Decision numbers appear in parentheses and §6.10 maps all of them.

### 6.0 Ownership (cross-design reconciliation, 2026-11-07)

**5C owns the shared job system**, and no other sub-project builds a second one:
- the job model, the scheduler and its pools, persistence and restart;
- **one worker binary, `fin-workers <family>`**, and the **`fin-jobs`** crate with the protocol types and the envelope;
- spawning and the kill tree, retries, poison handling and isolation (bisection);
- warm workers for short interactive jobs (§6.3);
- helper processes started beside a job's worker (§6.4) and the lifecycle of the job's mapped files;
- the `jobs.*` RPCs, `fin job …` and `fin wait` (5C-2);
- the diagnostics kinds **`job.done`** and **`worker.exit`** (every process 5C starts, plugin job processes included).

**Contributions:**
- **Sub-project 6** contributes only the **`media` family** (its job specs and the family's code inside `fin-workers`), the **mapped-file bulk channel** (a shared-memory crate that sub-project 6 owns; 5C only creates, sizes, hands over and deletes the files), and the **media fields of `job.done`** (as `kv` 2 of the kind, §3.4).
- **Sub-project 7** owns sandboxing (`fin-sandbox`, profiles `agent`, `hook`, `worker`, `plugin`); 5C calls it at every spawn (§6.2).
- **Sub-project 8** contributes **plugin job adapters**: a job kind whose work is done by a plugin process, started by 5C as a helper (§6.4) or as the job's process, under the `plugin` profile.
- **5D** contributes the `library` family (§7).

**As built:** three crates carry the engine: **`fin-jobs`** (the envelope and the worker and helper runtimes; it creates no process), **`fin-jobhost`** (the host side as an rlib: spawning, kill domains, framing, timers, memory; Plan Decision 1) and **`fin-workers`** (the binary). `fin-node`'s `jobhost` module is only the napi binding over `fin-jobhost`, so spawning lives in `finisherd`'s process and `fin-sandbox` can implement `Spawner` without napi. The TypeScript side is `apps/finisherd/src/jobs/` (store, kinds, pools, scheduler, host wrapper, files, runner, hooks, calls, helpers, control, restart, retry and isolation, warm pools, events, engine, layer), `@finisher/contracts` `jobs.ts` (the job records as Effect Schemas) and `control/migrations/jobs/` (the `jobs` chain, §3.1).

### 6.1 Data model

```text
Job {
  id: ULID, kind: Name (JOB_KINDS), kindVersion, family: Name (WORKER_FAMILIES),
  params: JSON (validated by the kind), owner: {kind, id}, via?: {plugin}, rights?: opaque,
  project?: ProjectId, library?: LibraryId,
  priority: interactive | normal | background,
  state: queued | running | paused | succeeded | failed | cancelled | interrupted,
  phase: run | hooks | announce | done,
  attempts, maxAttempts, idemKey? (per owner), dedupeKey? (per kind),
  progress: {done, total?, unit: bytes|items|frames|seconds, phase?: Name, at},
  checkpoint?: JSON (resumable kinds), result?: JSON, error?: FinError, itemErrors?: [...],
  files?: [MappedFile], helpers?: [HelperSpec],
  parent?: JobId, createdAt, startedAt?, finishedAt?, nextRunAt?
}
MappedFile { name, bytes?, mode: read | write | readWrite }          (created by 5C, format owned by the family)
HelperSpec { kind: codec | plugin, id, profile: worker | plugin, files: [name] }
```

- **Kinds** are declared in `finisherd` (`jobs/kinds/*.ts`) as a `KindSpec`: `{kind, version, family, scopes, resources, warm?, defaultPriority, qos, timeoutS, stallS?, maxAttempts?, resumable?, batch?, dedupe?, rememberPoison?, resourceHealthy?, files?, helpers?, memory?, prepare?, onSucceeded?, hookTimeoutS?, journal?}`. A family is a `FamilySpec` (`program`, `profile`, `memory` per class, `warm {slots, idleS, maxJobs, maxAgeS}`, `warmKey?`, `onCall?`, `diagKinds`, `pluginId?`). Params are validated by Rust (`validateJobParams`, below), not by a TypeScript schema. 5C-1 ships the `test` family (behind `test-hooks`) and its kinds; 5D registers `library.*`; sub-project 6 `media`; sub-project 8 plugin adapters. Release builds know no kinds until a family registers its params.
- **Ownership:** a job belongs to its owner principal (`local` runs share the owner `local`; each credential and device is its own owner; in-process callers inside `finisherd` use the owner `system`). Owners see and control their own jobs; `jobs.control` sees and controls all; jobs on projects or libraries outside a principal's reach are invisible to it. **As built (5C-1):** owner, `via` and `rights` (the invoker ∩ the plugin's grants, opaque to 5C) are recorded and handed to hooks, `onCall` and the bus; **nothing is checked** until 5C-2 (ownership, reach) and sub-project 8 (rights in its `onCall` methods).
- **As built, the store** (`jobs` chain in `control.db`, registered when `jobs/store.ts` is imported before the store opens): `jobs` and `job_items` tables; step 1 as in Plan Decision 25 (`formatVersion` 1), **step 2** adds `jobs.stats` (the run statistics `job.done` needs after a restart; `formatVersion` 2), **step 3** is indexes only (`jobs_created (createdAt, id)`; `jobs_owner_created` replacing `jobs_owner`; `formatVersion` 3, readers ≥ 1), added when the budget run showed the unfiltered `list` sorting every row (6.3 ms at 10,000 jobs, 0.26 ms after). `phase` marks where a job is: `run`, `hooks` (the result is persisted, `onSucceeded` may not have finished), `announce` (terminal, `job.done` not yet marked sent) and `done`. Transitions are one `UPDATE` that checks state and phase; legal ones: queued → running / paused / cancelled / failed; running → running (another run in place) / succeeded / failed / cancelled / interrupted / queued / paused; paused → queued / cancelled; interrupted → queued. `list` orders by `(createdAt, id)` descending with the cursor `<createdAt>.<id>` (an unreadable cursor is `INVALID_REQUEST {field: cursor}`).
- **Writes (D26):** a transition is written at once; progress stays in memory (events at most every 250 ms per job); a checkpoint at most every 10 s per job (the newest kept, flushed at the end of the attempt); item outcomes in one transaction per second or 256 rows. **Items are therefore at-least-once:** an item begun but not reported re-runs after a crash, and so may one reported in the last second before `finisherd` itself dies; a family's per-item effects must tolerate a re-run. Items acknowledged in `job_items` never re-run.

### 6.2 The worker binary, the envelope and spawning

- **`fin-workers <family> [--warm]`** is the one worker binary (arch §7's `fin-workers`), one subcommand per family; it is found beside the `fin-node` `.node` file (`packages/fin-node/`, or `packages/fin-node/test/native/` for the `test-hooks` build; `FIN_NODE_BINARY_DIR` selects the directory) and versioned with it. A family's program is `Bundled(name)` (a bare name resolved in that directory, limited to `fin-workers` and, for 8C, `fin-analyze`) or `Path(abs)` (plugin processes, set only by a registered family or helper resolver, never from params). **As built:** the other entries are `fin-workers watchdog <host pid>` (below), `--helper test:<ok|exitEarly|hang|spawn|call>` (test helpers), `--spawn-broker` (Windows, below) and `--version`; exit 2 for usage and unknown families; the release build refuses `test`. **Workers never start processes**: anything a job needs beside its worker (a codec backend, a plugin) is a helper that 5C starts (§6.4). `fin-sandbox`'s `worker` profile denies process creation once it exists; until then **a static gate and a run-time check hold the rule**: `scripts/check-worker-spawn.mjs` (in `pnpm gates`) refuses `std::process::Command`, `fork`, `vfork`, `posix_spawn*`, `exec*`, `CreateProcess*`, `ShellExecute*` and `clone(2)` in `crates/fin-workers/src/` and `crates/fin-jobs/src/` outside `test_family/` and `watchdog.rs` (over code with comments and strings blanked, so `.clone()` passes), and the host counts foreign processes in a job's group (below).
- **The envelope** (`fin-jobs`, Rust → JSON Schema → generated TS): UTF-8 newline-delimited JSON on the worker's stdin/stdout, `{"v":1,"t":<type>,"seq":n,"job"?:<id>,…}`, one message a line (≤ 1 MiB; a checkpoint's data ≤ 256 KiB; `seq` from 1 in each direction, growing by exactly 1). Bad JSON, an unknown type, a `seq` gap, an oversized line or a message naming another job than the one started kills the process as `protocolError` (the job sees `WORKER_CRASHED {protocol}`). stderr is a log stream (lines over 4 KiB cut; the last 8 KiB stored as `error.details.stderrTail` of a crashed attempt, in `control.db` only, never the journal).
  - host → worker: `start {job: {id, kind, kindVersion, params, checkpoint?, attempt, extra?}, limits: {aliveMs, memoryBytes?, readBytesPerSec?}, files, helpers, dirs: {job, tmp}, diagnostics}`, `cancel {job, reason: cancel|pause}`, `shutdown`, `reply {id, value|error}`;
  - worker → host: `hello {v, minor, role, family, kinds, warm, envelope?, selfSandbox?, pid}`, `begin`, `item {value?|error?}` (an item with `error` is recorded in `itemErrors` and the job goes on), `progress`, `checkpoint`, `alive`, `result`, `error {code, message, details, retry: never|backoff|fresh}`, `ready`, `diag {kind, payload}` and `call {id, method, params}`. Any message counts as alive; the runtime sends `alive` every `aliveMs` (5 s) from its own thread.
  - Bulk data never travels in the envelope: it goes through the job's mapped files, in the format of the family's channel.
  - **Worker runtime** (`fin_jobs::worker::run`), in order: the self-sandbox entry (below), on Windows crash-UI suppression (`SetErrorMode`, `WerSetFlags`), `hello`, then `start`, the family's `run`, `result` or `error`, and exit 0 (cold) or `ready` (warm). **stdin EOF exits 70 at once** (the host is gone). A panic is answered `WORKER_CRASHED` and exits 101 (a warm worker is not reused after one). Before `ready` a warm family has closed every file and mapping under the job directory (a contract; tested below).
- **Host calls (Deviation from revision 3: added by the plan, Decision 5a).** A process asks its host for something by `call {id, method, params}`; the host routes it to the family's (or the helper's) `onCall` with the job's owner, `via`, `rights`, role and helper, and answers `reply`. At most 16 calls are open per process (the 17th gets `CALL_LIMIT`); a family without `onCall`, or an unknown method, gets `INVALID_REQUEST {method}`; calls still open at the exit are interrupted and the attempt waits for them to end before it reports. After the worker's `result` or `error`, new calls get `INVALID_REQUEST {after: answer}` and `item`, `begin`, `checkpoint`, `progress` are dropped. Sub-project 8 serves its host API this way (`project.snapshot`, `media.pcm`, `net.fetch`, `job.submit`, ring `slot`/`release`); a plugin process speaks only the envelope. 5C-1 ships the mechanism and test methods.
- **Versions (Deviation, Decision 3):** `hello` carries `v` (1), `minor` (0), `envelope` (the first 16 hex digits of BLAKE3 over the envelope's canonical schema without string `description`s) and the `{kind, version}` pairs served. **Bundled** processes must match `v` and the envelope hash exactly and serve the dispatched kind at its version; anything else is `WORKER_PROTOCOL_MISMATCH {expected, got}`, never retried (this includes a `hello` with the wrong `role` or family). **Plugin** processes (`Path`, profile `plugin`) need only `v` and the kind; any `minor` is accepted, and an unknown message type from a process whose `minor` is above the host's gets an error `reply` (when it has an `id`) instead of a kill; at or below the host's minor, an unknown type is a protocol error. Additions within a major are optional messages and optional fields only.
- **Spawning lives in `fin-node`** (`jobhost`), not in `child_process`, because the process must enter its kill domain before it runs and `fin-sandbox` (a Rust crate) must wrap the spawn. Every process gets `childEnv()` (the allow-list; never a credential) plus `TMPDIR`/`TEMP`/`TMP` = its private `tmp/`, cwd the job directory (cold) or the worker directory (warm), and inherits exactly stdin, stdout and stderr (Linux `close_range(CLOEXEC)` in `pre_exec`; macOS `POSIX_SPAWN_CLOEXEC_DEFAULT`; Windows a handle list).
  - **One spawner thread** does every spawn and kill (on Linux `PR_SET_PDEATHSIG` fires when the *thread* that forked exits); each process has a stdout reader, a stderr reader, a writer thread on a bounded queue (256 messages or 4 MiB; a full queue means a process that does not read, killed as hung) and a waiter. Nothing allocates or locks in `pre_exec`; every blocking syscall retries `EINTR`.
  - **The JS thread never blocks (Decision 11a).** `spawn`, `killGroup` and `shutdown` are promises settled from the spawner thread; `JobHost.start` is a promise too (it waits for the watchdog on Unix and makes the broker on Windows). Events cross through one unref'd threadsafe function, bounded by our own in-flight count (1,024 events and 16 MiB of JSON; lifecycle events are not counted): when JS falls behind, the readers wait, the pipes fill and the worker blocks on its own writes. Progress is coalesced in Rust (the newest kept, at most every 250 ms; any other message from the process flushes it first); `alive` never reaches JS. No event of a process precedes its `spawn` resolving. Every promise is settled (`INTERNAL {reason: unanswered}` if an answer is dropped). **Deviation from the plan:** the bound is our counter, not napi's blocking mode, which blocks inside napi-rs's abort lock that the JS thread takes at teardown.
  - **`Spawner`** is called for every process with its role, profile (`worker`|`plugin`), family, program, args, env, cwd, path grants and QoS, and may rewrite the program, arguments and environment, add Linux `pre_exec` steps or Windows attributes and a token, and set `job_limits {active_processes?, ui?}` for the process's leaf domain and a `self_sandbox {profile, args}`. The default is `Unsandboxed`: `caps().sandboxed` is false, the engine logs one warning at start and `workers()` reports it (5C-2 puts it in `server.status`). **Until `fin-sandbox` exists, 5C spawns unsandboxed with the env allow-list only**; this includes third-party code plugins, which the owner allowed before 7C (plugins §20, decision 1); their profile applies automatically once `fin-sandbox` lands. Where the OS mechanism is disabled every profile runs unconfined, with a `SANDBOX_DISABLED` warning in `server.status` and a diagnostics event (agent §8.9).
  - **Self-sandbox entry (Deviation, Decision 9a)** for sandboxes a process applies to itself after `exec` (seccomp, Seatbelt): the host passes `FIN_SELF_SANDBOX` (JSON, no secret); `fin_jobs::worker::run` and `helper::run` first call the function `fin_jobs::sandbox::register` was given (sub-project 7's), then send `hello` with `selfSandbox`. Fail closed: the variable with no function, or a function that fails, exits 78 before `hello` (the host reports `SpawnFailed("selfSandbox")`), and a `hello` whose `selfSandbox` differs from the request is a protocol error. Nothing registers a function in 5C-1; a test `Spawner` exercises both paths.
- **The kill tree, owned by the host, on every OS.** A **group** is one kill domain per job (a cold job's worker and helpers share one; a warm worker has its own, and each job it runs gets another for that job's helpers); every process also has a **leaf** inside its group, carrying `job_limits` (**Deviation: leaf domains**, Decision 17). A group leader is observed with `waitid(WNOWAIT)` and reaped only at `close_group`, so a pgid is not reused while the group is open; `kill_group` on a closed group is a no-op.
  - **Windows:** a host Job (`KILL_ON_JOB_CLOSE | DIE_ON_UNHANDLED_EXCEPTION`, no breakaway, a non-inheritable handle only `finisherd` holds), a nested group Job per group (with `JOB_OBJECT_LIMIT_JOB_MEMORY` when limited) and a leaf Job per process (`ACTIVE_PROCESS`, `UILIMIT_*`). Each process is created **suspended** (`CREATE_SUSPENDED | CREATE_NO_WINDOW | …`), assigned host → group → leaf, then resumed, so nothing runs outside it and the nesting holds even when `finisherd` already runs inside a terminal's or Electron's Job. Cancel is `TerminateJobObject` on the group; if `finisherd` dies, the host Job's handle closes and everything dies. Exits come from the process handle (the completion port's messages are only a hint). **Deviation (spawn broker, Decision 10):** making the pipe ends inheritable around `CreateProcessW`, even only on the spawn thread, leaked them into concurrent inheriting spawns in `finisherd` (libuv's), measured. So the host never holds an inheritable handle: each host keeps a **broker**, a suspended, never-resumed process in the host Job; the ends are duplicated into it for the call only, and each child is created with the broker as its parent (`PROC_THREAD_ATTRIBUTE_PARENT_PROCESS`) and `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`. The broker image is the bundled `fin-workers.exe --spawn-broker` (0.4 MB of commit; it exits at once if resumed), else the system's `cmd.exe /d /c exit 0`, never `finisherd`'s or Electron's own executable (8 MB, and a GUI that would start if resumed). It is made at host start with a DACL that admits `SYSTEM` only. Without a broker the host refuses to spawn (`WORKER_UNAVAILABLE {reason: noBroker}`); a Job `finisherd` runs in that allows no more processes gives `{reason: processLimit}`, and a commit its memory limit refused is `WORKER_OOM`; an outer Job's memory limit makes an abnormal exit after a new outer peak above it `Oom` too. A killed process exits `fin_jobhost::KILLED_EXIT_CODE` (137) with no signal.
  - **Linux:** each group is a process group (the first process makes the pgid, later ones join it) and each process sets `PR_SET_PDEATHSIG` = SIGKILL (exiting 70 if `getppid()` already changed). **Where cgroup v2 is delegated** (`/proc/self/cgroup` names a writable v2 path whose `cgroup.procs` holds only `finisherd` and that has `cgroup.kill`, kernel ≥ 5.14; a systemd user service with `Delegate=yes` or a container; never a terminal's scope, which `finisherd` does not rearrange; `FIN_CGROUPS=off` disables) the host moves itself into `<cg>/host`, enables `+memory +pids`, makes `jobs/g<group>` per group (`memory.max`, `memory.swap.max` 0, `memory.oom.group` 1) and `p<proc>` per process (`pids.max`), and kills with `cgroup.kill`; leftover `jobs/g*` of an earlier run are killed and removed at start. **Deviation:** `pids.max` counts threads, so `active_processes: n` becomes `n × 256` tasks (a fork-storm stop, not an exact count). Otherwise the host kills `-pgid`.
  - **macOS:** process groups, spawned with `posix_spawn` (`SETPGROUP`, `CLOEXEC_DEFAULT`, **`START_SUSPENDED`**, a QoS class); the host registers the pgid with the watchdog, then `SIGCONT`s the process (`sigcont_resumes_start_suspended` ran on macOS and passes). macOS takes no `pre_exec`: a sandbox there rewrites the program or uses the self-sandbox entry.
  - **Unix host death: the watchdog (Deviation, Decision 15).** Process groups alone do not cover a dead host (`PR_SET_PDEATHSIG` reaches only direct children; macOS has no equivalent), so `Host::start` starts `fin-workers watchdog <host pid>` in its own process group, ignoring `SIGINT`, `SIGTERM`, `SIGHUP` and `SIGPIPE`, with an empty environment and cwd `/`, and waits for its `ready`. The host writes `+g`/`-g <pgid>` and `+c`/`-c <cgroup>` lines as groups open and close (`-g` always before the leader's reap). When the host exits (Linux `pidfd_open` + `poll`; macOS `kqueue` `NOTE_EXIT`; plus a `getppid` check each second) or stdin reaches EOF, the watchdog drains stdin (a late `-g` still counts), sends `SIGKILL` to every listed group, writes `1` to every listed `cgroup.kill` and exits. If the watchdog dies the host logs it, restarts it (at once the first time, then at most once a minute) and re-sends the list. It is host machinery: it serves no job and starts no process.
  - **Everywhere:** a worker or helper exits when its stdin reaches EOF.
  - **Abandonment (Decision 18):** `kill_group` kills the group and waits `killWaitMs` (5 s); a process still alive after any kill (uninterruptible I/O) is **abandoned**: reported once (`worker.exit` reason `abandoned`), its pool slot released, and logged when it finally exits (`AbandonedExit`). Two abandoned processes of different jobs on one `io:` pool mark it **stuck**: nothing is dispatched there, and submits that need it fail `RESOURCE_STUCK {pool}` (exit 5) until one really exits; a queued job whose own process is abandoned is held until it exits, else `RESOURCE_STUCK {reason: abandoned}` after 10 minutes.
  - **Deviation from §6.9 (Decision 18): the `setsid` gap.** With process groups (macOS; Linux without a delegated cgroup) a grandchild that calls `setsid`/`setpgid` out of its group survives a cancel and host death; Windows Jobs and delegated cgroups have no such gap (`cgroup_kill_takes_setsid_grandchild`, `setsid_grandchild_gone_after_host_sigkill`; `setsid_grandchild_escape_is_documented` asserts the survival). Members forked in the instant between the host's death and the watchdog's `kill(-pgid)` can also be missed. Sub-project 7's denial of process creation in the `worker` profile closes both.
- **Cancel (Decision 23)** sends `cancel {reason: cancel}`, waits **2 s (500 ms for interactive jobs)**, then kills the job's groups. A queued or paused job ends `cancelled` at once; a running one before the worker's `start` (helpers starting, `hello` pending) is killed at once; a job in its hook phase is returned unchanged (it ends `succeeded`). `cancel` resolves once the run ended.
- **Liveness and progress are separate (Decision 19):** no message for 30 s after `hello` is `WORKER_HUNG` (the clock counts only while a reader waits on its pipe, never time blocked handing events to JS); no `begin`, `item` or `progress` for the kind's `stallS` (default 600 s) while a job runs is `JOB_STALLED`; the kind's `timeoutS` from `start` is `JOB_TIMEOUT`; no `hello` 10 s after spawn is `noHello` (`WORKER_UNAVAILABLE`, retried). Each kills the job's groups. **Exit after drain (Decision 19a):** the stdout reader drains to EOF or 200 ms after the exit, and `Exit` goes out on the same per-process queue as the messages, so a `result` written just before `exit(0)` always precedes it. A process that exits before `hello` is `Exited` (the engine sees no hello); after the worker's answer the engine kills a worker that has not exited 2 s later.
- **Memory (Decision 20): one meaning everywhere, per group** (the worker and its helpers for a cold job; for a warm job the worker's group takes the family's interactive rule and the job's helper group the kind's). A `MemoryRule {fixedMiB?, ramFraction?, capMiB?}` resolves at dispatch (`resolveMemoryRule` in fin-node, Rust). Enforcement: Windows `JOB_OBJECT_LIMIT_JOB_MEMORY` on the group Job (confirmed at exit by the limit-violation information and `PeakJobMemoryUsed`); Linux `memory.max` on the group cgroup (OOM = `memory.events` `oom_kill > 0`); elsewhere a sampler on the timer thread checks each limited group every `rss_sample_ms` and kills it over the limit. Sampled figures: Linux **`RssAnon + RssShmem`** (file-backed pages are reclaimable and not the job's), macOS **`ri_phys_footprint`**. All answer **`WORKER_OOM {limit, used?}`**, and every `ready` carries the worker group's current figure (the recycling input, §6.3).
- **Foreign processes (Decision 22)** are those in a job's group beyond the ones the host spawned: counted from `TotalProcesses` of the leaf Jobs (less the process itself and console hosts), the group's `cgroup.procs` tree, or a `/proc`/`proc_listpids` pgid scan, at each `result`, before each kill and at each exit, into a seen set so a grandchild killed with its group still counts. The engine logs any non-zero count; with the `test-hooks` build of `fin-node`, a job of any family but `test` with `foreign > 0` fails `WORKER_CRASHED {foreign}`, so 6A-2's and 5D's suites catch a family that starts processes. **Residual:** the check runs at an exit, so a warm worker's jobs are not checked.
- **QoS (Decision 21):** `normal`; `utility` (macOS `QOS_CLASS_UTILITY`, Linux nice 5, Windows `BELOW_NORMAL_PRIORITY_CLASS`) for library scans and probes; `background` (macOS spawned at utility with `FIN_QOS=background`, the runtime lowering itself after `hello`; Linux nice 10; Windows below normal) for CPU-heavy background kinds. Interactive jobs run at `normal` whatever their kind says.
- **Bandwidth:** a per-root byte budget is enforced in the worker by a token bucket on reads (`ReadBudget`, a 100 ms bucket).

### 6.3 Scheduling and warm workers

- **Pools** are named slot counters: `cpu` (cores − 1, at least 1), `gpu` (1; sub-project 6), `hwenc` (2; sub-project 6 resizes it), `net` (4; sub-project 8), `io:<rootKey>` per library root (defined by 5D; network and sync: 1 scan + 2 probe jobs; local: 4), and the `interactive:<family>` warm pools below. A job runs when every pool it needs has a slot. **As built:** a kind that needs an undefined pool refuses the submit (`WORKER_UNAVAILABLE {pool}`, no row).
- **Cold jobs** (the default): one worker process per job; small work is batched (a `library.probe` job takes up to 256 files).
- **Order (Decision 34):** effective priority is the base level minus one level per 10 minutes waited (never above `interactive`), then `createdAt`, then id. **The scheduler is a pure function `schedule(state, now) → dispatches`**, over a queue indexed by (resource set, priority, warm key) FIFOs so one blocked head never hides a runnable job. Property-tested against an independent brute-force reference (1,000 runs per property; two mutations of `schedule` fail the tests). **Deviation: multi-slot jobs can starve.** The scheduler reserves nothing, so a job needing several slots at once (two `cpu`, or `gpu` + `hwenc`) waits while single-slot jobs keep taking the slots as they free, aging notwithstanding. Accepted; a reservation for the oldest blocked job is the fix if a kind needs it. Within one pass, dispatches come first, then warm spawns and retirements, and a ready worker whose key has a due job is never retired.
- **Warm workers** (for sub-project 6's `fin look` ≤ 300 ms): kinds marked `warm` submitted at `interactive` priority (any other priority runs cold) run on a **kept-alive worker of their family** that serves jobs back to back.
  - **Pool:** `interactive:<family>` has its own slots (default 1, at most 2); a warm dispatch takes a `cpu` slot while it runs but never waits for one: `cpu` may run over by at most the sum of every warm family's slots (`warmOverrun`, checked by `schedule` exactly as `Pools.take` applies it).
  - **Warmness comes from the engine (Deviation, batch A review):** a family declares one cold argv and the engine appends `--warm` for a warm process (`fin-workers media --warm`; a plugin's `<path> <args…> --warm`); a family argv holding `--warm` is refused; `Family::warm()` only declares that it can serve warm jobs. Sub-project 6's `media` and sub-project 8's SDK must accept the trailing flag and send `ready` after each job. The host picks the mode per process, since a family such as `test` has warm and cold kinds.
  - **Warm keys (Deviation, Decision 35):** `FamilySpec.warmKey(job)` binds a worker to the key of its first job (sub-project 8: plugin, project, invoker class); keys and FIFOs are `JSON.stringify` tuples. The family's slots are shared across keys; with no worker for a key and a full pool, the longest-idle ready worker of a key with no due job is retired for it.
  - **Hand-off:** `submitJob` calls `handOff` before any `pump()`: a warm job heading its key's FIFO with a ready worker is dispatched in the submit's own turn, with no `schedule()` call. **Deviation:** the decision and the worker's reservation are in the submit's turn, but the `start` is written once the job directory's async `mkdir`s are done (no blocking fs on the JS thread); the budget is measured submit → `start` handed to the host's writer queue (p99 2.0 ms measured, below). Otherwise `schedule()` spawns one (one per key at a time, when the family has room) and the job waits for its `hello`. Slots go back, and `pump()` runs, when the attempt ends, before `onSucceeded`, so a ready worker never waits for a hook. `pump()` never nests.
  - **Lifecycle (Decision 36):** each worker has its own group (the family's interactive memory rule, also `start.limits.memoryBytes`) and directory `<jobs>/warm-<run>-<n>/` (cwd and `TMPDIR`; its `tmp/` is emptied after each `ready`, a failure killing the worker; removed before `mkdir`; stale ones go with the start-up sweep). Idle **60 s** → `shutdown` (`idleExpired`); at a `ready`, **500 jobs**, **30 minutes** or a reported figure above **80 %** of the group's limit spawns the replacement first, the old worker serves until the replacement's `hello`, then gets `shutdown` (`recycled`); a worker asked to go is killed after 2 s. `warm(family)` (used by sub-project 6 when a project opens) starts one worker now if the pool has none (`JOB_KIND_UNKNOWN`; `WORKER_UNAVAILABLE {notWarm}` for a family without a warm kind; nothing for a keyed family or one whose program is a function of the job). `workers().warm` lists `{family, key, pid, state, served, ageS, idleS, memory}`. No state between jobs except caches keyed by content hash (a family contract). Times come from Effect's `Clock`.
  - **The "closed before `ready`" contract is checked in test-hooks builds** after each job: the host lists the worker's open files and mappings under the job directory (`/proc/<pid>/fd` and `maps`; `proc_pidinfo` via `fin_jobhost::held_paths` on macOS, compiled for `aarch64-apple-darwin`, not yet run; on Windows a rename of the directory and back) and a hit fails the job `WORKER_CRASHED {heldFiles}` (deterministic, the worker killed). A check that cannot be made fails the job `{heldFiles: [], checkFailed}` on every OS.
  - **Cancel and failure (Decision 23, Deviation L6):** `cancel`, 500 ms for the worker's `error JOB_CANCELLED`, then both groups killed. **As built, the worker's `ready` after that answer gets the 2 s `EXIT_GRACE_MS`, not the remainder of the 500 ms**; Decision 23's text ("answers within the grace: `error JOB_CANCELLED` then `ready`") read literally would give `ready` 500 ms. The answer is the cancel's measure; `ready` is the worker's housekeeping (closing files) and a slower one is not worth losing a warm process for, and a worker that never answers is still killed at 500 ms. A helper that fails during a warm job also gets the worker a `cancel`, not a kill; a `ready` within 500 ms keeps it and the attempt fails with the helper's error.
  - **Crash and retry:** a crash, kill or OOM during a job loses the worker; nothing respawns until a job needs one. A warm job that ran before is retried **once, at once, on a fresh worker** (`warm.fresh`: `schedule()` gives it only a worker with `served: 0`, spawning one when there is room, else retiring a reused ready one for it; Deviation: crashes included, not only `fresh`/`oomWarm`); a warm worker that dies before its `hello` fails the attempt of its key's oldest due job as a cold spawn would. A `hello` that is not a warm worker of its family (role, family, `warm: false`) counts as one that never came up. Warm jobs are never isolated.
  - **No warm worker under a sandbox (owner decision, 5C-1 Task 11 review H3).** A warm worker is granted the whole jobs root `readWrite`, since the jobs it will serve are not known when it starts. That grant never ships under a sandbox: while `caps().sandboxed` or a worker's `hello.selfSandbox` is set, warm kinds run cold (queued warm jobs are queued again as cold, every warm worker goes, and a warm job's worker is retired after it rather than handed another job). Sub-project 7C must grant each job's paths to its warm worker before warm workers are enabled there (TASKS follow-up).
- **Pause (Decision 37):** `{job}` on a queued job → `paused` (`pausedFrom: queued`, it keeps its place and `createdAt`); on a running resumable job (a `resumable` kind, or a batch kind with items left) → `cancel {reason: pause}`, 10 s for the checkpoint, then `paused` (`pausedFrom: running`; the stopped run is not counted: `attempts` goes back by one); a running non-resumable job finishes. `{all}` stops dispatch and pauses the running resumable jobs; `resume {job}` re-queues a paused job (a `{all}` pause still holds dispatch); `resume {all}` re-queues every paused job and restarts dispatch. Requests rank cancel > close > pause; a run that succeeded first succeeds; warm workers are kept until they expire.
- **Limits:** 50,000 unfinished jobs (`JOB_QUEUE_FULL`); finished jobs kept 7 days or the newest 10,000 (retention at start and hourly: items first, then rows, ≤ 500 per statement through `deleteInBatches`).
- **Idempotent submit (Decision 27):** the same `idemKey` from the same owner returns the existing job; the same `dedupeKey` **from the same owner** while queued, running or paused returns that job; a kind with `rememberPoison` also returns that owner's finished job of the same `dedupeKey` that failed `POISON_ITEM`. **Deviation (5C-2 Decision 7):** dedupe was per kind across owners; across owners a submit would answer a job its caller cannot see. An unknown kind is `JOB_KIND_UNKNOWN`, params that Rust rejects (`validateJobParams`, ≤ 512 KiB, `kindVersion` checked as a `u32`) `JOB_PARAMS_INVALID {errors}`, `maxAttempts` must be an integer 1..10, and the decoded params are stored. The job is inserted, indexed and dispatched in the same turn, so `submit` resolves with the job `running` when a slot is free.

### 6.4 Helpers and mapped files

- **Mapped files:** a kind may declare files; 5C creates them in `<server-data>/jobs/<jobId>/` (0700 directory made with `ensurePrivateDir`, 0600 files; local disk), sizes them, passes their paths to the worker and its helpers in `start`, and deletes the directory when the job ends (after `onSucceeded`, with Windows sharing-violation retries 20 × 25 ms; also after a crash, at the next start and hourly). Their layout is the family's; 5C never reads them.
  - **As built (Decision 38):** names are relative, segments `[A-Za-z0-9._-]+` (no `..`, ≤ 4 deep, no Windows device names, no segment ending in a dot, no case-only duplicates, no `~`); a name ending in `/` is a directory, and a `write`/`readWrite` directory is emptied before each attempt; a file **with** `bytes` is created at that size and recreated before each attempt when writable; a file **without** `bytes` is written by `prepare` and checked to be a regular file with one link; ≤ 1 GiB a file, 4 GiB a job. A creation error fails the attempt `IO_ERROR`, retried with backoff.
  - **Hardening (9a/9b reviews):** a reused directory is `lstat`ed segment by segment before it is prepared again (the job directory, `tmp/`, every mapped path, each helper directory); a symlink or junction fails the job `JOB_FAILED {file, problem: link}`, cause `tampered`, never retried. Files are made with `O_EXCL | O_NOFOLLOW`, never truncated through a link, and hooks reach files only through `ctx.open` (mapped names only, `O_NOFOLLOW | O_NONBLOCK`, `fstat` regular with one link). Grants: the job dir `read`, `tmp/` `readWrite`, each file at its mode; workers and helpers get no grant on the job dir itself. Other Windows reparse points are invisible to `lstat`; sub-project 7's sandbox keeps a worker from making them.
- **Helpers** (codec backends, plugins) are **siblings of the worker, started by 5C**, never by the worker:
  - order (Decision 39): directory and files → `prepare` → each helper spawned in turn in the job's group (cwd `helpers/<id>/`, its own `tmp/`; `id` matches `[A-Za-z0-9._-]{1,64}`; an unregistered `(kind, id)` fails `HELPER_FAILED {code: WORKER_UNAVAILABLE}`) → its `hello` (10 s; `role: helper`, its `kind` and `id`; helper hellos carry no family) → `attach {job, files: only its own, dirs}` → the worker, once every helper is attached, with `start.helpers` listing `{id, kind, files}`;
  - connected to the worker **only through the job's mapped files**; each has its own envelope channel to the host (`hello`, `alive`, `error`, `call`), not to the worker; a helper's `call` goes to its `HelperProgram.onCall`;
  - a helper that exits or sends `error` before the worker's answer fails the job `HELPER_FAILED {helper, code}` (the code from the exit verdict, or the helper's own; a helper's hang is `{code: WORKER_HUNG}`, a fault of its own beating a later error) and kills the group; after a `result` the worker's exit is followed by `shutdown` to live helpers, ≤ 2 s, then the group is killed;
  - warm workers keep no helpers between jobs: helpers are per job, in a group of their own.
- Sub-project 8's plugin job adapters are helpers (or, for a job that is all plugin, the job's only process) under the `plugin` profile, resolved as `Program::Path`.
- **Kind hooks (Decision 40; sub-project 6's 6A plan Task 12).** `prepare(ctx)` runs until it has succeeded once per job directory (a `.prepared~` marker, so retries reuse it and `extra` stays the first attempt's), after the directory and files exist and before any process starts; it writes the files without `bytes` and may return `{extra}`, passed as `start.job.extra` and kept in `jobs.extra`; it is bounded by `hookTimeoutS` and interrupted by a stop. **`onSucceeded(ctx)` is at least once and must be idempotent:** after the worker's `result`, 5C persists the result and `phase: hooks` in one `UPDATE`, then runs the hook after every process that could still write the directory has exited (a cold job's whole group; for a warm job the helper group, the worker having closed its files) and before the directory is deleted; it may return a replacement result; its budget is `hookTimeoutS` (60 s, `JOB_TIMEOUT {hook}`); a failure fails the job with the hook's error, never retried; a cancel during it is too late. If `finisherd` dies during the hook the restart keeps the directory and runs it again with `rerun: true`. Both hooks are Effects; long work runs off the JS thread.

### 6.5 Persistence, retries and isolation

- Jobs live in `control.db` (the `jobs` chain, §3.1); see §6.1 for what is written when. Warm-worker state is in memory only.
- **Restart (Decision 28):** at start the jobs root is swept except `phase: hooks` directories; terminal rows not yet announced are announced again (below); each `running` row is examined: `phase: hooks` re-runs `onSucceeded` with `rerun: true` (an unregistered kind fails `JOB_KIND_UNKNOWN`); otherwise it becomes `interrupted` and, when resumable with attempts left, `queued` again from its checkpoint and recorded items (a batch kind's next run gets `batch.restrict(params, items not yet in job_items)`); the others stay `interrupted` with `JOB_INTERRUPTED` and emit `job.done`. A row found `running` means a crash, so its run spends an attempt. `paused` rows stay paused; queued rows are indexed; retention runs now and hourly.
- **Graceful close (Decision 29; owner decision, 5C-1 9b review): a graceful stop spends no attempt for a resumable job.** `engine.close` stops dispatch and timers; idle warm workers get `shutdown`; running `phase: run` jobs get `cancel {reason: pause}` and 2 s, then their groups are killed and the rows recovered at once as at a start, so a clean stop leaves no `running` row in `phase: run`, **except that the stopped run is not counted**: a resumable job goes back to the queue with its attempts as before the run, as a pause leaves them. A job whose pause was asked before the close stays `paused`; a run that ended with its own definitive outcome before the stop reached it follows the retry policy; hook-phase jobs finish their hook; then the host shuts down (2 s). A submit while closing is `WORKER_UNAVAILABLE {reason: closing}`; a second `close` waits for the first. A crash (`SIGKILL`) still spends the attempt.
- **Retries (Decision 31).** Failures are classified in this order; the last failure's code is the job's error:

  | Cause | Retry | Isolate |
  |---|---|---|
  | a **resource fault**: `SIGBUS` (`EXCEPTION_IN_PAGE_ERROR`, `0xC0000006` on Windows), the host's `resourceFault`, an abandoned process, or any crash when the kind's `resourceHealthy(job)` answers false or fails | backoff | never |
  | crash: exit without `result`/`error`, another signal, `protocolError`, a caught panic | backoff | yes (cold batch jobs only) |
  | `WORKER_OOM` on a cold job | with an item in flight: as a crash; otherwise none | only the in-flight rule |
  | `WORKER_OOM` on a **reused warm worker** | once, at once, on a fresh worker | never |
  | `WORKER_HUNG`, `JOB_STALLED`, `JOB_TIMEOUT`, `HELPER_FAILED`, `noHello`/`SpawnFailed` | backoff | never |
  | worker `error` with `retry: backoff` / `fresh` / `never` (default) | backoff / once at once on a fresh process / none | never |
  | `WORKER_PROTOCOL_MISMATCH`, a failed `prepare` or `onSucceeded`, `tampered`, foreign processes, a file `prepare` left wrong | none | never |
  | cancel | none (`JOB_CANCELLED`) | — |

  Backoff is `5 s × 2ⁿ`, n the attempts made (10 s, 20 s, 40 s …), at most 5 minutes (`nextRunAt`, persisted, on the engine's Effect clock), up to `maxAttempts` (default 3). **Interactive jobs retry once, at once, on a fresh worker** (`maxAttempts` 2) and are never isolated. A stopped run is never retried; a pause asked during a run the policy re-queues still pauses the job.
- **Poison handling and isolation of a crashing batch (Decision 32; pure `isolation.ts`, state in `jobs.isolation`).** A batch kind declares `batch {items(params), restrict(params, items), combine?}`.
  1. Items the worker already reported (`item`) are never re-run.
  2. **The items in flight** (`begin` without `item`, in `begin` order — **a set**, since 5D probes groups of up to 32) are retried: several are halved, one runs **alone**, then the rest. A run of one item that crashes again poisons it (`POISON_ITEM {item, cause}`, written to `job_items`) and the rest re-run without it.
  3. Only when no item was in flight (a crash between items) is the remainder split in halves.
  4. **Never bisected:** `WORKER_HUNG`, `JOB_STALLED`, `HELPER_FAILED`, I/O errors, an offline root and resource faults. `resourceHealthy` is asked before any poison verdict.
  
  Isolation runs start at once without backoff and spend no attempt. **Budget (Deviation: tightened by review): `min(2n, 2⌈log₂ n⌉ + 2p)` crashing runs**, n the items, p the poison items found; past it the job fails `WORKER_CRASHED {isolation: budget, crashRuns, poisoned}`, the case of crashes no single item explains. The between-items property is asserted for p ≤ 2 (halving with no item in flight costs about ⌈log₂ m⌉ + 1 runs per poison item, so many poison items found that way can pass the budget, which treats them as unexplained). **Every batch kind's success result is `combine(results, items)`** (default `{items: {<item>: value}, itemErrors}`), also for a single run.

### 6.6 Events and hooks

- **`JobEventSink {jobDone, workerExit, forward}`** (Decision 42) receives the events; `finisherd` wires `journalSink` (diagnostics D4/D34): `job.done` (`kv` 1; a kind's extra fields dropped) and `worker.exit` recorded as source `finisherd`, forwarded `diag` payloads of journal kinds; the journal decides consent, and a refused event is logged, never thrown. Tests use `layerMemorySink`.
- **`job.done`** for every job that reaches a terminal state, **at least once**: the terminal `UPDATE` marks the row `announce`, the event goes out, and only after the sink returned is the row marked `done` (and the directory deleted); one cut short by a crash or a failing sink is sent again at the next start, with the run statistics stored in `jobs.stats`, and reaches `onDone`/`events()` subscribers for 30 s. A consumer dedupes by job id, which `jobDone(event, job)` passes beside the payload (the journal payload has none). Fields: `kind`, `family`, `outcome: succeeded|failed|cancelled|interrupted`, `code?`, `attempts`, `priority`, `warm`, `queuedMs`, `runMicros`, `items?`, `itemsFailed?`, `isolated`, `helpers`, plus the kind's `journal(job)` fields (sub-project 6's `kv` 2).
- **`worker.exit`** for every process 5C started, the watchdog and the broker excepted: `role`, `family`, `helperKind?`, `pluginId?`, `reason` (`done|idleExpired|recycled|cancelled|killed|crashed|hung|stalled|abandoned|spawnFailed|oom|protocolError`; **`oom` and `protocolError` are additions**), `exitCode?`, `signal?`, `jobsServed`, `lifetimeS`, `sandboxed`. **Residual:** `journalSink` leaves the `plugin` field out: the journal's hashing key does not reach TypeScript, and no plugin process runs before sub-project 8 (§6.12).
- **Name tables** in `fin-core/src/events/names_jobs.rs` (`JOB_KINDS`, `WORKER_FAMILIES`: `test`, `testbad`, `testplugin`, `library`, `media`, `analyze`, `plugin`; `SIGNALS`: lower case without `SIG`), with a drift test that keeps the registered kinds and families inside them.
- **The in-process bus (Decision 43):** `onDone(listener)` receives the `JobView` after the terminal row is written (sub-project 7's hooks attach here; 8's budgets read `owner` and `runMicros`); `events()` is a `Stream` of `{type: state|progress|item, job}` for 5C-2's `jobs.watch`. A throwing listener is logged and never affects the job.

### 6.7 RPCs and `fin` (5C-2, as built)

> **5C-2 is built** (plan `docs/superpowers/plans/2026-11-08-sub-project-5c2-job-rpcs.md`; its Decision numbers in parentheses). Linux x64 verified.

| RPC | Rule (`RPC_SCOPES`) | Payload → answer |
|---|---|---|
| `jobs.submit` | `jobs.submit` **and** the kind's own scopes (escalation from the `jobKind` resolver), plus reach over `project` (`jobProject`) | `{kind, params (≤ 256 KiB as JSON), priority?, idemKey? (≤ 128 chars), project?, challenge?}` → `JobView` |
| `jobs.get` | `jobs.read` | `{job}` → `JobView` |
| `jobs.list` | `jobs.read`, reach over `project` | `{state? (≤ 8, deduped), kind?, project?, mine?, cursor?, limit? ≤ 500}` → `{jobs, next?}` |
| `jobs.wait` | `jobs.read` | `{ids (1–100), any?, timeoutMs? (1–600,000; default 60,000)}` → `{jobs}` |
| `jobs.watch` (stream) | `jobs.read` | `{ids? (1–100)}` → `{type: state\|progress, job}` \| `{type: item, job, item, error?}` |
| `jobs.cancel` | `jobs.read`, then by ownership (`job` resolver): `jobs.submit` for one's own, `jobs.control` for another's | `{job}` → `JobView` once the run ended |
| `jobs.pause`, `jobs.resume` | as `jobs.cancel` for `{job}`; `jobs.control` and every project in reach for `{all: true}` | `{job} \| {all: true}` → `{job?}` |
| `jobs.workers` | `jobs.control`, every project in reach | → `{pools, warm, processes, stuck, caps, diagDropped}` |

- **Ownership and reach (Decisions 2–3; `jobs/access.ts`).** A job's owner is the submitting principal's `{kind, id}`. A principal sees a job when the job is in its reach and it owns the job or holds `jobs.control`; an agent session's credential id is its session id, so it sees only its session's jobs (no agent mode grants `jobs.control`, sp7 §6.1). Reach: a job's project must be in the principal's `projects`; a library job needs `library.read` (5D Decision 47). **A job the caller cannot see answers `JOB_NOT_FOUND` with the same message and details as an unknown id**, in every RPC, `jobs.watch` included; a `project` the caller names out of reach is the chain's `PROJECT_NOT_FOUND`. `jobs.list` filters in SQL (owner unless `jobs.control` without `mine`; `project IS NULL OR project IN (…)`; `library IS NULL` without `library.read`).
- **Resolvers (Decision 4):** `jobProject` (the payload's `project` into the reach check; unlike `project` it opens nothing, so no reopen rule), `jobKind` (`RuleFacts.kindScopes`), `job` (`RuleFacts.job: own | other | none`, `none` = unknown or invisible). An unknown or invisible job costs what one's own does (`jobs.submit`), so the scope answer leaks nothing either. The chain gets them as `ChainDeps.jobs` (`chainJobs(engine)`).
- **No jobs RPC writes through the store's guard**, so no rule has `writes`/`targets`; no job scope is human-only, but a kind whose scopes include a human-only one makes `jobs.submit` `HUMAN_ONLY` for non-humans and confirmed by default for the console (Decision 5).
- **Only kinds that opt in are submittable** (`KindSpec.rpc: true`, Decision 6); any other is `JOB_KIND_UNKNOWN`, the same as an unknown kind. `media.look` and `media.capabilities` do not opt in (their own RPCs check `out`, the branch and the project); the test kinds do. A kind that opts in must accept only params whose paths and project agree with the job's `project`.
- **`jobs.watch` (Decision 8):** with `ids`, each job's current view first (read and sent in the turn that subscribes), then only their events; without, every visible job's events. The engine hands every event to `onEvent` listeners synchronously; each watcher has its own buffer of **256** events (`WATCH_BUFFER`), and a watcher that falls that far behind ends with **`WATCH_LAGGING {buffer}`** (exit 5): it lists again and watches again. Nothing is dropped silently; one slow watcher holds back neither the engine nor other watchers.
- **`server.status` (Decision 9)** carries `jobs: {caps: {killTree, watchdog, cgroupMemory, leafLimits, sandbox, sandboxed}}` and, while `sandboxed` is false, the warning `SANDBOX_DISABLED {sandbox, message, fix}`; `finisherd status` prints a `Jobs:` line.
- **`fin` (Decision 10):** `fin job ls [--state <s>]… [--kind <k>] [--mine] [--limit <n>] [--cursor <c>]`, `fin job show|cancel <id>`, `fin job pause|resume (<id> | --all)`, `fin wait <id>… [--any] [--timeout <s>]` (default 300 s, at most 600; its own connection, outside `FIN_DEADLINE_S`): exit 0 when every waited job (with `--any`, every finished one) succeeded, 4 `JOB_FAILED` (`details.failed` `{id, state, code?}`, `details.jobs` the views), 5 `WAIT_TIMEOUT`, 3 `JOB_NOT_FOUND`. `fin job show` of a failed job is a read (exit 0). Commands that start jobs print `{job}` and return at once (arch §9.9): `fin look --no-wait` then `fin wait <job.id>`. Inside `finisherd`, other services (5D, sub-project 6's `look` handler) use `JobEngine.submit(owner, spec)` directly; their own RPCs are authorised by the chain (§4.4).
- **Bounds (review of 5C-2, M1–M4).** One principal holds at most **16** open `jobs.wait` and `jobs.watch` calls together (`JOB_CALLS_MAX`; the 17th is **`TOO_MANY_WATCHES {limit}`**, exit 5); the engine indexes waiters by job id, so an ending job wakes only its own waiters, and a watcher checks each event in O(1) (`access.ts` `seer`). `{all}` and `jobs.workers` act on every project: they need `jobs.control` **and** `projects: "*"` (`RpcRule.allProjects`; otherwise `SCOPE_DENIED {reason: reach}`, after the scope checks). **`local.key` carries `jobs.control`**, so while workers run unsandboxed (`SANDBOX_DISABLED`) whoever reads `local.key` controls every job and sees every pool and process. A principal limited to some projects names the job's `project` on `jobs.submit` (`INVALID_REQUEST {field: project}`): a job without one is in everyone's reach. A kind may also require one (`KindSpec.requiresProject`), and an RPC kind declares `paramsReach(params, {project})`, called on every submit (`JOB_PARAMS_INVALID {errors}`); the registry refuses `rpc: true` without it, refuses it from a plugin family (sub-project 8 decides how plugins' kinds are submitted), and refuses kind scopes outside `SCOPES`. Owners other than `local`, the console, a device and `system` hold at most **1,000** unfinished jobs each (`JOB_OWNER_LIMIT`, `JobEngineOptions.ownerLimit`; past it `JOB_QUEUE_FULL {limit, reason: owner}`), leaving the 50,000 global headroom to the person's own; `priority: interactive` is honoured for human principals only (anyone else gets the kind's default). A jobs rule reaching a chain without `ChainDeps.jobs` is `INTERNAL`, never let through.
- **Codes (Decision 11):** `WATCH_LAGGING` (5) and `TOO_MANY_WATCHES` (5) join `JOB_ERROR_CODES`, `fin`'s table and the console's; `WAIT_TIMEOUT` and `JOB_NOT_FOUND` now carry fixes. `RPC_METHODS` lists the `jobs.*` RPCs, `CLI_COMMANDS` `job.*` and `wait`, `CLI_FLAGS` `any`, `cursor`, `mine`, `state`, `timeout`.

### 6.8 Split and what 5C needs from 5A

- **5C-1, the engine (built):** `fin-jobs`, `fin-workers` with the `test` family, `fin-jobhost` and `fin-node`'s `jobhost` module (spawner trait, kill tree, envelope framing, timers), the TypeScript scheduler with pools and warm workers, persistence, restart, retries and isolation, mapped files and helpers, the `job.done`/`worker.exit` kinds in `fin-core::events`, and the `JobEngine` API. **It needs only 5A Task 1** (`control.db`, its migration chains and the audit log) **and Task 2** (`<server-data>` layout and private directories, for `jobs/`); it uses `openControlStore`/`ControlStore {db, tx, deleteInBatches, close}`, `registerChain`, `ensurePrivateDir` and `childEnv()`, and no principals or audit kinds. **As built:** `finisherd` starts the engine with the server (`main.ts`: `jobs/store.ts` imported before the control store opens; `layerJobEngine` built after the control store and the diagnostics journal, so it closes first: `engine.close()` before the journal and the store; `<server-data>/jobs/` private; one start line `finisherd job engine: killTree=… watchdog=… cgroupMemory=… leafLimits=… sandbox=… sandboxed=…`; `start.diagnostics` follows `diagnostics.enabled()`; `ServeOptions.jobs` passes engine options). Events go to `journalSink` (diagnostics D4/D34 landed before 5C-1 Task 12); owners are recorded (`system` for in-process callers) but not checked.
- **5C-2, the surface:** the `jobs.*` RPCs with their scopes, ownership and reach checks, `jobs.watch`, `fin job …` and `fin wait`. **It needs 5A Tasks 3–6** (principals, the `fin` handshake, the chain and scopes, protocol 2); it must also put `caps()` (kill tree, watchdog, leaf limits, sandboxed) into `server.status`.
- What 5C never needs from 5A: credentials, the console, tickets (beyond what the chain gives), the native pipe listener.

### 6.9 Security, codes, cross-platform, tests (5C)

- **Security:** workers and helpers get the cleared environment and no credential, speak only to their host over their own stdio, never start processes (gate and foreign-process check, §6.2), and never receive a path outside the job's project, library root or job directory. Sandboxing is `fin-sandbox`'s (sub-project 7), applied at every spawn once it exists; **until then nothing a worker or plugin does is confined beyond the env allow-list, the private job directories and the kill tree.** Warm workers are not used under a sandbox until 7C grants per-job paths (§6.3). A third-party plugin process is unsandboxed until 7C (plugins §20, decision 1).
- **Codes (Decision 41; `crates/fin/src/output.rs`, mirrored in `JOB_ERROR_CODES`):** exit 3 `INVALID_REQUEST` (also the job list cursor and the `reply` errors of Decision 5a), `JOB_KIND_UNKNOWN`, `JOB_PARAMS_INVALID`, `JOB_NOT_FOUND`, `JOBHOST_STATE` (a job-host call on a group or host the caller should know is gone, `details.reason` `unknownGroup|groupClosed|shutDown`, a caller's bug); 4 `JOB_FAILED`, `JOB_CANCELLED`, `JOB_INTERRUPTED` and the result codes `WORKER_CRASHED`, `WORKER_HUNG`, `JOB_STALLED`, `JOB_TIMEOUT`, `POISON_ITEM`, `HELPER_FAILED` (`JOB_RESULT_CODES`); 5 `JOB_QUEUE_FULL`, `WORKER_UNAVAILABLE`, `WORKER_PROTOCOL_MISMATCH`, `WAIT_TIMEOUT`, `RESOURCE_STUCK` and **`WORKER_OOM`** (5C-1 enforces the limit, so it owns the code; 6A maps it to 5 too). `UNKNOWN_MESSAGE` and `CALL_LIMIT` appear only in `reply` errors. `JOBHOST_UNSUPPORTED` was removed with Task 5 (every OS has a kill domain).
- **Cross-platform:** the kill tree of §6.2; signals do not exist on Windows (cancel is the envelope message, then the job kill; a killed process exits 137); a killed process's open file on SMB causes transient sharing violations for the next attempt (Windows directory deletion retries); mapped files on local disk only. Verified so far: **Linux x64** (all tests, including the kill suites 20 runs in a row; cgroup mode under `systemd-run --user -p Delegate=yes`), **Windows 11** during Task 5 (kill and death suites ×20; fin-jobhost and fin-workers suites; the job engine's TypeScript tests have not run there), **macOS** compiled and linted for `aarch64-apple-darwin` only, apart from the Task 3 `SIGCONT` proof. Task 14 runs everything on all three (§6.12).
- **Tests:**
  - scheduler properties (pools never over-committed, priority and aging, warm pool separation, the `cpu` overrun bound), isolation properties (every poison item found alone, no re-runs of acknowledged items, the budget) with `fast-check` 4;
  - `test.spawn` (test-only: it breaks the no-processes rule on purpose) with its double-forked grandchild gone after cancel and after `finisherd` is killed with `SIGKILL`: `host_death_unix`, `host_death_windows` and `host_cgroup` in Rust, and `finisherd_kill_leaves_no_process` in TypeScript (`test/support/engine-child.ts` runs the real engine layer with cgroups and the watchdog as in production, in a child that is killed; every pid and the watchdog gone within 5 s; `restart_after_kill_interrupts_and_sweeps`; `graceful_close_leaves_no_running_rows`); the spawn gate and its own test;
  - `test.crash` with a known poison item isolated; `test.hang` and helper failures never bisected; `test.stall` hits `JOB_STALLED`; `test.oom`, `test.fail`, `test.files`, `test.warm`, `test.helper`;
  - warm workers: back-to-back jobs on one process with the hand-off budget, idle expiry and the 500-job/30-minute/80 % replacement with `TestClock`, a crash mid-job retried on a fresh worker, fresh-vs-reused results, files closed before `ready`, keys, sandbox fallback to cold;
  - helpers start before the worker, die with the job, and only see their mapped files; mapped-file directories are gone after the job and after a crash; restart resumes from the checkpoint and re-runs `onSucceeded`; `job.done` and `worker.exit` reach the journal sink;
  - `fin wait` exit codes (5C-2).
  - Counts at the 5C-1 integration (2026-11-08, Linux x64): `FIN_REQUIRE_SERVER=1 cargo test --workspace` 2,356; `cargo test -p fin-jobhost -p fin-workers --features test-hooks` 136; `pnpm test` 926 (5 skipped); the jobs suite 231 (21 files; 1 skipped), run twice in a row.

### 6.10 The plan's decisions, as built

The numbers are the **plan's** Decision numbers (`docs/superpowers/plans/2026-11-07-sub-project-5c1-job-engine.md`), not §11's. **As built** = built as decided, with the detail in the section named; **Deviation** = built differently, with the reason in that section.

| # | Decision | Mark | Section |
|---|---|---|---|
| 1 | Three crates: `fin-jobs`, `fin-jobhost` (rlib), `fin-workers`; `fin-node` only binds | As built | §6.0 |
| 2 | `fin-workers` beside the `.node` file; `Program::Bundled`/`Path` | As built | §6.2 |
| 3 | `hello` carries `v`, `minor`, `envelope`; exact match for bundled, major + kind for plugins | As built; the minor rule is **new since revision 3** | §6.2 |
| 4 | NDJSON framing, size caps, `seq`, `protocolError` | As built | §6.2 |
| 5 | The message set | As built | §6.2 |
| 5a | Host calls `call`/`reply`, 16 open per process | As built; **new since revision 3** | §6.2 |
| 6 | Worker runtime: self-sandbox entry, crash-UI suppression, EOF exit 70, panic exit 101 | As built | §6.2 |
| 7 | `diag` forwarded only when `start.diagnostics` and in `diagKinds`; else dropped and counted | As built | §6.6 |
| 8 | Helper channel (`attach`, own `hello`) | As built | §6.4 |
| 9, 9a | `Spawner` hook, `Unsandboxed` default; the self-sandbox entry, fail closed | As built; **new since revision 3** | §6.2 |
| 10 | Environment and handles; Windows pipe handles inheritable only around `CreateProcessW` | **Deviation:** the spawn broker; no handle is ever inheritable | §6.2 |
| 11 | One spawner thread; per-process threads; bounded writer queue | As built | §6.2 |
| 11a | The JS thread never blocks | As built, **Deviation:** the event bound is our counter (not napi's blocking mode); `JobHost.start` is a promise; `spawn` resolves through the event queue | §6.2 |
| 12 | Linux `pre_exec` order, no allocation | As built | §6.2 |
| 13 | macOS `posix_spawn` with `START_SUSPENDED` | As built (the `SIGCONT` proof ran on macOS) | §6.2 |
| 14 | Windows host → group → leaf Jobs; exits from process handles | As built, with the broker (Decision 10) and an outer-Job check (active-process and memory limits) | §6.2 |
| 15 | Unix host death: the watchdog | As built; it writes `ready` after masking signals (a signal right after `exec` had killed it 3 runs in 20) and is restarted at most once a minute | §6.2 |
| 16 | cgroup v2 where delegated | As built; **Deviation:** `active_processes: n` becomes `pids.max` `n × 256` (threads count) | §6.2 |
| 17 | Kill domains: group and leaf, reap at `close_group` | As built; the trait grew `close_leaf`, `hello`, `kill_tree`, `leaf_limits`, `kernel_memory` | §6.2 |
| 18 | Kill and abandonment; stuck pools | As built; **Deviation from §6.9: the `setsid` gap** with process groups | §6.2 |
| 19, 19a | Timers; exit after drain | As built | §6.2 |
| 20 | Memory limits per group; `RssAnon + RssShmem`, `ri_phys_footprint` | As built | §6.2 |
| 21 | QoS | As built (macOS `background` is spawned at utility and lowered by the runtime) | §6.2 |
| 22 | Foreign processes | As built; checked at exits only (warm jobs excepted) | §6.2 |
| 23 | Cancel: 2 s, 500 ms interactive; a warm worker that answers keeps its process | As built; **Deviation (L6):** the `ready` after the answer gets 2 s | §6.3 |
| 24 | Workers never start processes: the gate and Decision 22 | As built | §6.2 |
| 25 | The `jobs` chain, step 1 | As built; **steps 2 (`stats`) and 3 (indexes) added** | §6.1 |
| 26 | Write rules; items at-least-once | As built | §6.1 |
| 27 | Submit rules | As built; an undefined pool refuses the submit | §6.3 |
| 28 | Restart | As built | §6.5 |
| 29 | Graceful close | As built; **Deviation (owner decision):** a graceful stop spends no attempt for a resumable job | §6.5 |
| 30 | Retention | As built | §6.3 |
| 31 | Failure classes and backoff | As built | §6.5 |
| 32 | Isolation of a crashing batch | As built; the in-flight items are a set; budget `min(2n, 2⌈log₂ n⌉ + 2p)` | §6.5 |
| 33, 34 | Pools; order and aging | As built; multi-slot jobs can starve | §6.3 |
| 35 | Warm pools, keys, hand-off, `--warm` | As built; **Deviations:** the engine appends `--warm`; the `start` follows the directory's `mkdir`s; fresh retries for every crash | §6.3 |
| 36 | Warm lifecycle; files closed before `ready` | As built; the check fails closed when it cannot be made | §6.3 |
| 37 | Pause | As built; a paused run gives its attempt back | §6.3 |
| 38 | Job directories and mapped files | As built; hardened against links (9a/9b reviews) | §6.4 |
| 39 | Helpers | As built | §6.4 |
| 40 | `prepare` and the at-least-once `onSucceeded` | As built | §6.4 |
| 41 | Codes | As built; `INVALID_REQUEST`, `JOBHOST_STATE` added, `JOBHOST_UNSUPPORTED` removed | §6.9 |
| 42 | The event sink | **Deviation:** `journalSink`, not `memorySink` (D4/D34 landed first) | §6.6 |
| 43 | The in-process bus | As built | §6.6 |
| 44 | Plugins: owner, `via`, `rights` recorded; `Program::Path`, `warmKey`, calls | As built (recorded, not checked) | §6.1, §6.2 |

### 6.11 Owner decisions, review fixes and measured budgets

**Owner decisions taken during 5C-1:**
- **A graceful stop spends no attempt for a resumable job** (9b review L6): a clean `finisherd` stop is not a crash, so the job goes back to the queue as a pause leaves it; only a crash (a row found `running` at start) spends an attempt (§6.5).
- **Warm kinds run cold when sandboxed** (Task 11 review H3): a warm worker is granted the whole jobs root because its jobs are unknown when it starts, and that grant never ships under a sandbox. Until sub-project 7C provides per-job grants, `finisherd` runs warm kinds cold whenever `caps().sandboxed` or a worker's `hello.selfSandbox` is set (§6.3).

**Review fixes** (each is a `fix(…)` commit with tests that fail without it; `TASKS.md` has the detail):
- *Batch A* (scheduler and store): an explicit class field because `node src/main.ts` strips types (`erasableSyntaxOnly` and a type-stripping gate); the warm `cpu` overrun as a sum over families; two scheduling passes; warmness from the engine; the cancel upgrade of a pause; mapped-file names refuse Windows device names, trailing dots and case-only duplicates.
- *Tasks 3–5* (host): macOS zombie accounting and `EPERM` for a zombie-only group; QoS `background`; the pgid stored only after the placement; callbacks through `catch_unwind`; a failed spawn settles its group; `close_group` kills what a member left; `Host::send` names the job before it queues the `start`; the Windows broker hardening (§6.2); an outer Job's memory limit.
- *Task 6* (binding): the event queue bounded by count and bytes; `spawn` through the event queue; every promise settled; generated wire types; `INVALID_REQUEST` for bad arguments.
- *Task 9a:* link checks on reused directories; grants; the terminal-announce phase; `INTERNAL` when anything throws; calls awaited before `worker.exit`; `prepare` once per directory.
- *Task 9b:* hooks reach files only through `ctx.open`; an abandoned process holds its queued job; a pause or close takes over only a run it stopped; `job.done` at least once; the graceful close spends no attempt.
- *Task 11:* a warm `hello` that is not a warm worker counts as never come up; a throwing attempt idles or kills its worker; per-run warm directories and `tmp/` emptying; sandbox fallback to cold; keys as `JSON.stringify` tuples; `fresh` only for a job whose last attempt lost its worker; `pump()` never nests; no spawn while closing; abandoned idle workers registered; the macOS held-files read; a failing check fails the job.
- *Task 12 and after:* load-tolerant timing in the tests (p99 ≤ 5 ms only under `FIN_STRICT_TIMING=1`), zombies counted as gone, engines closed before their data is removed, Windows clippy cleanliness.

**Measured budgets** (Linux x64 VM, release `test-hooks` binary, process groups: no delegated cgroup; `apps/finisherd/scripts/jobs-budget.ts`, `--json`, `--quick`; Task 14 records the other OSes):

| Budget | Measured |
|---|---|
| store statements, p99 (budget ≤ 1 ms with 10,000 jobs) | ≤ 0.07 ms; `list` of 50: 0.35 ms (0.26 ms p50 with the step 3 indexes; 6.3 ms before them) |
| warm hand-off, submit → `start` handed to the writer queue (≤ 5 ms p99, 10,000 queued, cold spawns cycling) | p50 0.64 ms, p99 1.96 ms (1,000 samples). Under a full `pnpm test` on a busy machine p99 reached 9–21 ms, so the test asserts p99 ≤ 5 ms only under `FIN_STRICT_TIMING=1` and otherwise median ≤ 2 ms, p99 ≤ 50 ms |
| `schedule()` over 50,000 queued jobs in 20 resource sets | median 0.18 ms |
| spawn → `hello`, `test.echo` | p50 2.5 ms, p95 3.1 ms |
| spawn → `hello` with 1 GiB more resident (fork cost; follow-up if over 150 ms) | p50 18.7 ms, p95 19.3 ms: no spawn helper process is needed |
| cancel → tree gone | p50 10.0 ms, p95 15.5 ms |
| Windows spawn broker (suspended, release) | `fin-workers.exe` 0.4 MB commit, 1.2–1.4 ms (31 ms cold); `cmd.exe` 1.5 MB, ≈ 1 ms |
| **Windows 11** (Ryzen 7 7800X3D, Defender on, no exclusions; 2026-11-08): store statements p99, warm hand-off, spawn → `hello` | statements ≤ 0.24 ms p99 (`list` of 50: 0.57 ms); hand-off p50 1.57 ms, p99 3.15 ms; spawn → `hello` p50 15.9 ms, p95 16.9 ms, with 1 GiB more resident p50 16.3 ms, p95 17.7 ms (the p50 sits on Windows' 15.6 ms timer period; Defender does not push it near 150 ms) |

The watchdog's cleanup time after host death, the macOS spawn times, and the p99 hand-off under load on macOS are Task 14's.

### 6.12 Known residuals and follow-ups

- **Windows 11, run (2026-11-08, non-elevated):** `cargo test --workspace` (with test-hooks `fin-workers`), `pnpm test`, every `pnpm e2e` scenario and clippy; the release `fin-workers.exe test` exits 2; Job Objects (`host_windows`: caps, leaf limits, breakaway refused, the tree killed on cancel, OOM confirmed by the violation info, only stdio inherited), the spawn broker (`host_broker_windows`, its DACL keeping other processes out), outer Jobs (`outer_*`), the held-files check through the directory delete (`files_closed_before_ready`) and warm workers pass. Fixed in tests only: `gone()` took the broker's access-denied `OpenProcess` for an exit (it passed elevated through `SeDebugPrivilege`), and the warm test proxy held a replacement whose spawn resolved after `release()`. Not done by hand: killing a real `finisherd` with `taskkill /F` and the nesting from Windows Terminal and PowerShell (the runs were started by Task Scheduler in the console session).
- **Not yet run on macOS and Windows** (Task 14, which needs the hosts; the cross-platform list is in `TASKS.md` and the plan): `cargo test --workspace`, `cargo test -p fin-workers` (test-hooks), `pnpm test`, `pnpm gates` and `pnpm e2e` on each OS, and that the release `fin-workers` refuses `test` (exit 2); kill trees by hand on each OS (`kill -9` / End task / `taskkill /F` on a real `finisherd` running `test.spawn` with `none` and `setsid`, and `test.hang`; expected none left except the `setsid` gap; Linux also under `systemd-run --user -p Delegate=yes`); Windows: the host → group → leaf nesting from Windows Terminal and PowerShell, no WER dialog for `segv` and `fastfail`, Defender's effect on spawn → `hello`; macOS: the watchdog surviving `Ctrl-C` in the terminal that runs `finisherd`, QoS read back, `ri_phys_footprint` against Activity Monitor for `test.oom`; and the budget numbers above on each OS.
- **The held-files check has not run on macOS or Windows** (`fin_jobhost::held_paths` on `proc_pidinfo`; the directory rename on Windows). It is compiled for `aarch64-apple-darwin` and fails closed, so a wrong answer there fails a test-hooks job rather than passing silently. The macOS `host_common` review-fix tests (`close_group_ends_what_a_member_left`, `a_missing_waiter_thread_still_reports_the_exit`, `an_answer_before_send_returns_is_on_its_job`) and `host_unix`/`host_death_unix` have not run on macOS either.
- **Before the Windows release: an endpoint-security check.** A process created suspended with a spoofed parent (`PROC_THREAD_ATTRIBUTE_PARENT_PROCESS`) and handles duplicated into it is the pattern EDR products flag as parent-PID spoofing (MITRE ATT&CK T1134.004). Run the job engine under Microsoft Defender for Endpoint (and the Defender ASR rules) before release, record whether it alerts, and change the design if it does.
- **Flakes (pre-existing, not from the engine's code):** Linux `host_cgroup` `delegation_detected_only_when_alone` fails about 1 run in 20 (the second host's cgroup setup gets `EBUSY` while the first still holds a thread; fix: retry the move and enable on `EBUSY` for a bounded time); Windows `fastfail_dies_without_wer` fails 4 runs in 10 non-elevated (about 1 in 10 elevated): `__fastfail` bypasses `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION`, so the WER service sometimes starts a headless `WerFault.exe -u -p <pid>`; no dialog appeared and no report was filed (`ReportArchive`/`ReportQueue`, user and machine, hold nothing for `fin-workers`). Fix: assert on a window or a report rather than the process, or exclude the worker from WER at install (sub-project 9). Both `warm.test.ts` cancel tests cancelled before `start` reached the host (the pool marks a worker busy first, and the engine rightly ends such a job at once), on Linux and Windows alike, 1 run in 3 to 5 alone; fixed in the tests (2026-11-08).
- **Gaps:** the `setsid` escape with process groups (§6.2); `pids.max` counts threads; the foreign-process check does not cover a warm worker's jobs; multi-slot jobs can starve (§6.3); a warm worker is granted the whole jobs root until 7C; items reported in the last second before a `finisherd` crash may re-run (§6.1); `clippy -p fin-node --features test-hooks` refuses `Path::exists` in `test_registry.rs` (pre-existing, not in `clippy-allow.json`).
- **Follow-ups by owner:**
  - *5C-2:* done (§6.7). Left: no `fin job watch` or `fin job workers` (the RPCs exist; the spec named no command); `jobs.submit` takes no `library` until 5D's kinds opt in (5D adds the field with its reach rule).
  - *Sub-project 7:* implement `Spawner`, the self-sandbox function and `job_limits` (active process 1 for the `worker` profile); deny process creation in the `worker` profile (closing the `setsid` gap); 7C grants a warm worker each job's paths before warm workers are enabled under a sandbox.
  - *Sub-project 8:* serve its host API through `onCall`; key plugin warm processes by `warmKey`; implement the envelope (minor rule) and the trailing `--warm` in its SDK; **hash a plugin process's id into `worker.exit`'s `plugin`** (`journalSink` leaves it out: the journal's hashing key does not reach TypeScript, and no plugin process runs before 8).
  - *Sub-project 6:* `fin-workers media` accepts a trailing `--warm`; `FamilySpec.program` for `media` is the cold argv only; 6A-2 appends `fin_media`'s params to `validateJobParams`.
  - *Sub-project 9:* package and sign `fin-workers` beside `fin-node`.
  - *Elsewhere:* a spawn helper process only if the large-heap spawn passes 150 ms (measured 19.3 ms p95); a reservation for the oldest blocked multi-slot job if a kind needs it.

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
| `library.add`, `library.remove`, `library.set` (hydration, bandwidth, aliases, `acceptVolume`, `acceptUnmounted`, `acceptEmptyRoot`) | `library.admin` (human) |
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
- **Where review items live:** `control.db` of the server that holds the project (owner decision 1). **Rejections are not review state:** they are `drop` records in the op log (§8.3), so they are never lost. What a move loses is open items' comments and their not-yet-merged accept marks; open `branchReview` items for active branches are re-derived.
- `branch_policy {projectId, protected: [branchId], requireReview, autoMerge: {enabled, into?, roles?, changedBy?, changedAt?}}`, default `protected: [main]`, `requireReview: false`, `autoMerge.enabled: false` (§8.5). It lives in this server's `control.db`, never in the project folder, so a shared project cannot switch it on for another machine.

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
- **Auto merge (owner decision, agent §20 item 4).** Branches of `auto`-mode agent sessions may be merged into a protected branch, `main` included, **only under a per-project owner policy**:
  - **off by default** (`branch_policy.autoMerge.enabled: false`); turned on or off only by `review.policy` (`auth.admin`, human-only: the console, TTY-gated, or a device), audited as `review.policy` with the old and new value. Turning it on is refused with `AUTO_MERGE_UNAVAILABLE` until sub-project 10's QA gate and critic exist (agent §6.5);
  - **the policy is a server function, not a scope:** no credential carries a right to auto-merge, and no RPC triggers one. The merge is made by the server-internal actor `{kind: policy, policy: autoMerge, enabledBy}` (sub-project 7, agent §6.5) through the same `review.merge {itemId, expect, acceptPending: true}` path, on a `branchReview` item whose **QA gate and critic verdicts both pass for that exact snapshot** (sub-project 10 records them on the item). If any check fails (`reason: policyOff|requireReview|verdictMissing|verdictFailed|snapshotMismatch`), nothing merges and the item stays open for a person. Until sub-project 10 delivers both verdicts no auto merge can happen;
  - `requireReview: true` on the project disables auto merge (a person decides every entry);
  - the policy stage (§4.4 stage 5) checks the policy and the verdicts; the merge carries the protected-branch guard (§4.4 stage 6) and `sourceUpTo` of the judged snapshot, so nothing outside what the critic saw is merged; `review.policy` writes are serialised with the project's in-flight writes, so an auto merge either completes before the policy is turned off or is refused;
  - audited as `review.merge` with `auto: true`, the item, the snapshot, the session, the verdict ids and `enabledBy`; the `review.verdict` entry says `by: policy`, `via: autoMerge`, so a dataset never mistakes it for a person's acceptance.
  Without the policy, agents still reach protected branches only through a person's review.

### 8.6 RPCs and `fin` (5E)

| RPC | Scopes |
|---|---|
| `review.request`, `review.comment`, `review.refresh` | `review.write` |
| `review.list`, `review.get`, `review.watch` (stream: item changes, not re-plans) | `project.read` |
| `review.decide`, `review.merge`, `review.close` | `review.decide` (the `auto` merge policy's actor is server-internal, §8.5) |
| `history.drop` | `project.write`, or `review.decide` on a protected target |
| `review.policy` (protected branches, `requireReview`, `autoMerge`), `review.policy.get` | `auth.admin` (human) / `project.read` |

`fin review request|ls|show|refresh|comment|decide|merge`, `fin drop`, `fin merge --include/--up-to`.

### 8.7 Security, codes, tests (5E)

- **Security:** an agent can open a review and comment, but not decide, drop into or merge into protected branches by default; the one exception is an `auto`-mode merge under an owner-enabled policy with passing QA and critic verdicts for the snapshot (§8.5), which makes a fooled critic the last check before `main` for projects whose owner turned it on; review text is content; the change list is computed by the server.
- **Codes:** 3 `REVIEW_NOT_FOUND`, `REVIEW_UNDECIDED`; 4 `REVIEW_STALE`, `REVIEW_HAS_CONFLICTS`, `REVIEW_CLOSED`; 7 `BRANCH_PROTECTED`; 4 `AUTO_MERGE_UNAVAILABLE` (enabling the policy before sub-project 10); merge reasons `DROPPED`, `notIncluded`.
- **Tests:** `fin-store` properties: a dropped origin is never copied by any merge into the target or its later descendants, whatever the order of drops, undrops, merges and imports; `include`/`sourceUpTo` never copy a record outside them, and a commit that lands between planning and the write transaction is not included; `derived::compute` agrees between a writer and a follower that imported the records in another order; the format goldens of §8.3. Server: `REVIEW_STALE`; pending never means accept outside the snapshot; conflicts open and supersede items; protected `main` refuses an `agent` commit (stage 4 and the in-transaction guard) and accepts `review.merge`; auto merge (the `policy` actor driven by a test hook): nothing merges with the policy off (the default), with `requireReview`, without either verdict, with a verdict for an older snapshot, and when the policy is turned off while the merge is in flight; it merges with the policy on and both verdicts passing (fake verdicts until sub-project 10); no RPC from any credential can make an auto merge; a `review.policy` write from an agent is `HUMAN_ONLY`; `review.verdict` entries; an e2e `review` scenario with an agent credential and the local principal as reviewer, including a project move that keeps the rejection.

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
| Control store, catalogs, FTS | `node:sqlite` (Node built-in; Node 24, the target; checked under Electron 44/45's Node 24.22) | MIT (Node), public domain (SQLite) | 5A, 5D |
| HMAC, ECDSA P-256, AES-GCM, random | `node:crypto`; Rust `hmac` 0.12 + `sha2` 0.10 in `fin` | MIT OR Apache-2.0 | 5A |
| WebSocket server under our protocol | `ws` 8 (already under `@effect/platform-node`; pinned directly) | MIT | 5A |
| Windows pipes, ACLs, Job Objects, process creation, enumeration | `windows-sys` 0.61 (already in the tree; 5C-1 adds `Win32_System_JobObjects`, `Win32_System_Threading`, `Win32_System_Pipes`, `Win32_System_IO`, `Win32_Storage_FileSystem` in `fin-jobhost`, and `Win32_System_Diagnostics_Debug`, `Win32_System_ErrorReporting` in `fin-jobs`) | MIT OR Apache-2.0 | 5A, 5C, 5D |
| Unix `prctl`, `posix_spawn`, `kqueue`, `pidfd_open`, `proc_pidinfo`, `statx`, cgroups | `libc` 0.2 (already in the tree; a direct dependency of `fin-jobhost`, `fin-workers` and `fin-jobs` on Unix) | MIT OR Apache-2.0 | 5C, 5D |
| Scheduler and isolation property tests | `fast-check` 4 (dev-only, exact pin; `pure-rand` MIT) | MIT | 5C-1 |
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
| 23 | 5C owns the one job system and the one `fin-workers <family>` binary; spawning lives in `fin-node` behind a sandbox hook; processes enter their kill tree before they run; stdin-EOF exit everywhere (built; the `setsid` gap with process groups is a deviation, and a Windows spawn broker keeps pipe handles from leaking) | §6.0, §6.2 |
| 24 | Crash isolation targets the in-flight item; resource failures are never bisected; stall detection is separate from liveness | §6.2, §6.5 |
| 24a | Warm workers in an `interactive:<family>` pool serve short jobs back to back, with idle expiry (60 s) and recycling (500 jobs / 30 min) | §6.3 |
| 24b | Workers never start processes; helpers are siblings started by 5C, connected only through the job's mapped files | §6.4 |
| 24c | 5C is split: the engine needs only 5A Tasks 1–2; the RPCs need 5A Tasks 3–6 | §6.8 |
| 24d | `control.db` migrations run in independent per-area chains | §3.1 |
| 34 | A graceful `finisherd` stop spends no attempt of a resumable job; only a crash does | §6.5 |
| 35 | Warm kinds run cold while a sandbox is in force, until 7C grants a warm worker per-job paths | §6.3 |
| 36 | Every process has a leaf kill domain inside its job's group; plugin processes follow a minor-version rule and warm keys; workers reach their host through `call`/`reply` | §6.2, §6.3 |
| 25 | Enumeration never opens a file; placeholders (files and directories, Nextcloud suffixes) are never read | §7.2, §7.3 |
| 26 | Volume identity per library; an unmounted share is offline; spin-up-tolerant timeouts | §7.1, §7.2 |
| 27 | Demand-driven NAS polling: 60 s while in use, 6 h otherwise, freshness pass before answering | §7.2 |
| 28 | Review merges take `include`/`sourceUpTo` checked in the write transaction; pending is never accept outside the snapshot | §8.2, §8.4 |
| 29 | Rejections are `drop` records in the op log (format 3 for those records only; no upcaster) | §8.3 |
| 30 | `review.verdict` is filled in 5E (not 10) | §3.4 |
| 31 | Protected branches per server, default `main`; `local` keeps the scope, agents do not | §8.5 |
| 32 | `auto`-mode branches merge into protected branches only under a per-project, human-only, default-off policy, with passing QA and critic verdicts for the snapshot, made by a server-internal `policy` actor (no scope, no RPC), policy writes serialised with the project's writes, audited | §8.5 |
| 33 | The repository targets Node 24, Electron's embedded Node | §1.6, §3.1 |

---

## 12. Owner decisions (2026-11-07)

The owner answered every question as recommended. The options weighed are kept in revision 3 (`git log` of this file).

1. **Review items live in `control.db` of the server that holds the project.** Open `branchReview` items are re-derived after a move; comments and not-yet-merged accept marks of open items stay on the server that recorded them. Rejections are durable regardless (`drop` records, §8.3). No `.finisher/review/` event files. → §8.1.
2. **Human-only operations are human-only now (5A), with the TTY-gated `finisherd` console as the human interface until the UI.** Until sub-project 9, consent changes (except training on, which waits for onboarding or a hand edit), purges, `project default`, confirmed takeovers and relocations need an interactive terminal on the server (`docker exec -it` on a NAS). Agents and tests lose `fin diagnostics consent|purge|project default`, `fin project takeover --confirm` and `--relocate`. → §4.6, §4.7.
3. **Browsers reach a remote server only over a route whose certificate they trust** (owner-supplied files such as `tailscale cert`, or a reverse proxy) **or over loopback through an SSH tunnel.** SPKI pinning is for the desktop shell and native clients. No ACME client, no clicking through certificate warnings; the v1 web client on a LAN needs one setup step. → §5.5.
4. **The local audit log keeps device names and remote addresses for pairing, revocation and failed authentication only;** local, owner-only, never uploaded, 180 days. It is the one store holding IP addresses (D34 keeps them out of the journal). → §3.3.
5. **`local` keeps `branch.protected` and `review.decide`;** `agent` and `mcp` credentials never get them by default. A hand-run agent using the local key can still write `main` without review until it runs under sub-project 7 or uses an agent credential. → §4.5. (Sub-project 7's `auto` mode reaches protected branches only through the separate, owner-enabled auto-merge policy, §8.5.)
6. **Human-only is advisory against raw-shell agents until sub-project 7's sandbox (7C), and this is documented** in the README's security model (5A Task 13). Agent and MCP credentials are offered as the recommended way to run a harness by hand, not forced: forcing them would buy attribution, not security, and break every script and test that drives `fin` directly. The real fix is sub-project 7 denying agents read access to `<server-data>/auth/`. → §2 principle 3, §4.6, §4.7, §4.10.

**Decisions taken during the 5C-1 build (2026-11-07/08), recorded in §6.11:** 7. A graceful stop spends no attempt for a resumable job (§6.5; Decision 34). 8. Warm kinds run cold under a sandbox until sub-project 7C provides per-job grants (§6.3; Decision 35).

**Cross-cutting decisions of the same day that change this design:** the repository targets **Node 24** (§1.6, §3.1); **`auto` may merge into `main` under a per-project owner policy** (agent §20 item 4; §8.5 here); **third-party code plugins are allowed before 7C** (plugins §20 item 1; §6.2 here); **v1 API models go through one OpenAI-compatible adapter (OpenRouter or a local server), and CLI agents use the user's own sign-in** (agent §20 items 6–7 and F1; §1.6, §5.5 here); the platform floor is **Windows 11 only and Linux kernel ≥ 6.2** (arch D-table; §4.11 here).

---

## 13. Risks

| Risk | Mitigation |
|---|---|
| Effect 4.0.6's RPC client ids or protocol hooks differ from what §4.3 assumes | 5A Task 3 starts by reading the source and recording the answer; the chain needs only a connection id per request |
| The native pipe listener (napi threads, overlapped I/O) is new native code | its own task with Windows-only tests; the handshake still protects a libuv pipe if the listener must be disabled |
| `node:sqlite` blocks the event loop | per-statement budget; no autocheckpoint on the main connection; batched deletes; fallback: the same schema through `rusqlite` in `fin-node` |
| Electron's Node (24) differs from the repository's (26) until the switch to Node 24 lands | the switch is a `TASKS.md` item that verifies `node:sqlite` and every API in use on Node 24; until then only APIs present in both |
| Keychain prompts, missing or locked Secret Service | file store with optional wrapping; never prompt |
| Browsers and self-signed TLS | trusted certificates or an SSH tunnel only (owner decision 3, §5.5) |
| Warm workers leak memory or carry state between jobs | recycling after 500 jobs / 30 minutes; fresh-vs-reused equality tests per family; a crash loses only the worker |
| `fin-sandbox` arrives after 5C | the `Spawner` trait; unsandboxed spawns with the env allow-list (third-party plugin processes included), reported in `server.status` |
| A fooled critic auto-merges into `main` | off by default; human-only, per-project switch; both QA and critic verdicts bound to the snapshot; audited; `requireReview` overrides it (§8.5) |
| NAS scans hang, disks sleep | worker processes, abandonment limits, spin-up-tolerant timeouts, demand polling |
| Sync providers hydrate on access | enumeration without opening; process-wide materialisation off on macOS; placeholder markers on every OS; an `open` counter in tests |
| A `drop` record makes a project unreadable to 4C-era servers | by design read-only (`NEWER_FORMAT`), never wrong; written only when someone rejects |

---

## 14. Findings in the architecture spec

Contradictions and gaps found while designing this; each batch's "as built" pass records the resolution in arch §27.

1. **§21.1 decides trust by transport** (local socket → `fin`, WebSocket → `ui`), but the WebSocket token sits in a file any same-user process can read, and loopback is reachable through SSH tunnels. Resolved by §3.4 and §4.3.
2. **§8.4 "peer-credential checks on the local socket"** are impossible from Node, Landlock (ABI 4) does not mediate `connect()` on pathname sockets, and libuv's Windows pipes take the default DACL. Resolved by key-file proofs and the native pipe listener (§4.2, §4.3).
3. **§8.4's env allow-list** lacked a way to hand `fin` its credential without a secret in the environment. Amended: `FIN_CREDENTIAL` (a path).
4. **§16 pins a self-signed certificate "at pairing"** for every client; browsers cannot pin, and the web client is v1. Resolved by owner decision 3 (§5.5).
5. **§16 DPoP-bound tokens** assume usable client clocks, while §27.9 records clients off by ~10 s for a day. Resolved by server nonces (§5.3).
6. **§21.1 phasing puts human-only operations in sub-project 7** although permissions are sub-project 5, and §24 puts `review.*` verdicts in sub-project 10 while §11.3's per-transaction accept/reject is sub-project 5's. Resolved by owner decision 2 (§4.6) and Decision 30.
7. **§11.3 review markers `open/review/resolved`** do not exist in the 4C model (`Marker` has no status); sub-project 10 needs an op.
8. **§6.4 "conflicts become review items"** presumes per-transaction action, but 4B's merge has no inputs for it, and nothing records a rejection. Resolved by `include`/`sourceUpTo` and `drop` records (§8.3, §8.4).
9. **§5 "single owner"** vs §16's device "role". Resolved: every device is the owner in v1.
10. **§16 "environments … sync"** is undefined. Read as §5.6.
11. **§6.7 fingerprints include mtime,** so copies never match across machines and relinking (§20) falls back to full hashes over the network. Accepted, on demand only (§7.3).
12. **§6.7 change detection** ignores inotify limits, cloud placeholders (any open or read downloads them, and on macOS whole directories can be dataless), unmounted mount points that look like empty libraries, and NAS hibernation. Resolved by §7.1–7.3.
13. **§7 gives TS "its own SQLite"** but forbids synchronous calls on the event loop. Resolved by the budget and rules in §3.1.
14. **§16 audit log vs D34 "never stored raw: host names, IPs"**. Resolved by owner decision 4 (§3.3): the audit log alone, for pairing, revocation and failed authentication.
15. **4A/4B expose "a person's decision"** (`takeover {confirm}`, `open {relocate}`) to `fin`, which D16 makes agent-only. Resolved by §4.6.
16. **The repository used to pin Node 26.12, but Electron 44/45 embed Node 24.22**, so an embedded `finisherd` (arch §4, desktop) ran on a different Node than the one it was tested on. Resolved by the owner's Node 24 decision (§1.6); the repository moved to Node 24.22.2 on 2026-11-08.
17. **The server id is reminted on a move** (4A), so it cannot be an environment id (§3.7).
18. **Tracked files hold personal data,** against the public-repo rule: arch §27.8 names the Apple Developer team's owner and team id, and D3 holds an absolute local path.

---

## 15. Review findings (revision 2) and where they are resolved

| Finding | Resolution |
|---|---|
| **B1** console could opt in to training; console commands scriptable; fixes printed commands | §3.4, §4.6, §4.7 (training on device-only; `CONSENT_UI_ONLY` from the console; TTY + server-issued typed challenge), §3.5 (fix texts) |
| **B2** plan commits not green; D4/D6 missing | plan reordered: the `fin` handshake is Task 4, enforcement switches on in that commit; Tasks 1, 2 and 10 can start now; Tasks 3+ wait for D4 and D6 |
| **B3** `exclude` merged unseen commits; pending meant accept | §8.2, §8.4 (`include`/`sourceUpTo` in the write transaction; pending only within the snapshot, and only on request) |
| **B4** rejections not durable | §8.3 (`drop` records: format 3, no upcaster, derived index); split out of §12 item 1 |
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
| Electron embeds Node 24 | §1.6, §1.5, §14 item 16 (revision 4: the repository targets Node 24) |
| Kill-tree race on Windows | improved: processes are created suspended, assigned to their Job, then resumed (§6.2), instead of assignment by pid after spawn |

---

## 17. Owner decisions applied (revision 4)

| Decision | Where |
|---|---|
| 5.1 review state in `control.db` | §8.1, §12 |
| 5.2 human-only now, TTY console | §4.6, §4.7, §12 |
| 5.3 browsers: trusted certificates or an SSH tunnel | §5.5, §13, §14 item 4 |
| 5.4 audit log: device names and addresses for pairing, revocation and failed authentication only | §3.3, §14 item 14 |
| 5.5 `local` keeps `branch.protected` and `review.decide` | §4.5 |
| 5.6 human-only advisory against raw-shell agents until 7C, documented | §2, §4.6, §4.7, §4.10; 5A Task 13 (README) |
| Node 24 | §1.5, §1.6, §3.1, §10, §11 Decision 33, §13, §14 item 16 |
| `auto` merges under a per-project owner policy (agent 7.4) | §1.6, §3.5, §4.5, §8.1 (`branch_policy.autoMerge`), §8.5, §8.6, §8.7, §11 Decision 32, §13 |
| Third-party code plugins before 7C (plugins 8.1) | §1.6, §6.2, §13 |
| OpenRouter or local OpenAI-compatible API models; CLI sign-in by the user (agent 7.6, 7.7, F1) | §1.6, §5.5 |
| Windows 11 only; Linux kernel ≥ 6.2 | §1.3 (unchanged: already Windows 11), §4.11 |
