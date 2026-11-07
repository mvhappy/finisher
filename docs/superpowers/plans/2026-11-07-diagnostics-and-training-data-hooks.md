# Diagnostics and training-data hooks — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (the event model and its classes, the journal's crash and concurrency behaviour, consent, redaction, purge and the upload boundary), `sonnet` for plumbing (napi, RPC, settings, the event sources, CLI, e2e, docs).

**Goal:** Finisher can record what it needs to improve the app and, later, to fine-tune an open-weight model on video editing. That covers what belongs in an edit and what does not, and everything that passes through an LLM. Upload, agent traces and the dataset builder come in later sub-projects, as additions with no rewrite. After this plan:
- one versioned event envelope with generated TS types, a closed kind registry, and a class (`meta`, `scrubbed` or `content`) on every payload field;
- a local, append-only, capped, crash-safe journal in `<server-data>/events/` that never blocks editing and costs nothing when off;
- consent from day one: `diagnostics` and `training` settings, env overrides, a per-project `collect: never` flag that travels with the project folder, and the consent recorded in every entry;
- the first sources: op-log commits (edits, undo, redo, merges, picks, reverts, branches) linked by `txId`, `fin` invocations, RPC and eval timings, font fetches, storage state, errors, panics and sessions;
- the reserved agent kinds that sub-project 7 fills in;
- `fin diagnostics show|export|purge|project`;
- an `Uploader` boundary with no network implementation.

**Architecture:**
- **`fin-core::events`** (new module, pure, wasm-safe) holds the envelope, the kind registry, the payload types and the class newtypes (`Content<T>`, `Scrubbed`, `Hashed`, `IdStr`, `Code`, `Name`, `PathRef`). It also holds the pure functions that list a kind's content paths from its JSON Schema, remove them (`redact`) and scrub text. It never reads a clock or makes an id: the emitter supplies `id` and `ts`.
- **`fin-journal`** (new crate, I/O, no network) is the only writer:
  - a writer thread behind a bounded channel; rotating JSONL files with size and age caps; torn-tail repair;
  - per-minute histograms for high-rate timings;
  - the consent gate and its resolution; the per-install key for keyed hashes; the set of `never` projects;
  - purge, export and the upload view, with the `Uploader` trait and its cursor;
  - the cost example.
- **`fin-store`** gains `collect` in `finisher.project.json` with `Store::collect_policy()` and `Store::set_collect()`. It emits nothing itself.
- **`fin-node`** opens one `Journal` per server data (like the font manager). It wraps every store call in `tasks.rs` to emit `log.commit` and the error kinds. It feeds the project flags from open and re-read, installs the panic hook, and exposes a napi `Journal` class.
- **`finisherd`:**
  - reads `privacy` from `settings.json` (watched, like `fonts.autoFetch`) and applies it;
  - publishes `diagnostics.enabled` in `finisherd.json`;
  - times every RPC into `rpc.stats`;
  - records `session.*` and uncaught errors;
  - serves `diagnostics.*` RPCs, `diagnostics.record` for `fin` included, with the source taken from the credential, a kind allow-list, a rate limit and a size cap.
- **`fin`** sends one `cli.command` per invocation when `finisherd.json` says recording is on. It sends the registry name, the flag names, the unkeyed `argsHash` (the server keys it), the duration, the exit code and the txIds. It also gets `fin diagnostics …`.

**Tech Stack:**
- Rust 2024, `serde`/`serde_json`, `schemars` 1 (`extend` attributes for `x-fin-class` and `x-fin-content`), `ulid` 1, `blake3` 1 (keyed mode), `getrandom` 0.3 (MIT OR Apache-2.0, already in the lock file) for the install key, `std::sync::mpsc::sync_channel`.
- **No new third-party crates beyond those.** No compression in this phase. No HTTP: `fin-journal` must not depend on `ureq`, `rustls` or `fin-fonts`, and a test checks it with `cargo tree`.
- Node 26, Effect 4, vitest.

**Spec:** `docs/specs/2026-11-01-finisher-architecture-design.md` §21.1 and D34. Related:
- §6.3–6.4 (txIds, records);
- §6.7 (`finisher.project.json` is rewritten temp-then-rename and synced);
- §8.4 (`fin` runs in the agent sandbox; `FIN_SESSION`);
- §16 (scopes; the audit log is separate from this journal);
- §21 (rows replaced by D34);
- §27.12 (schema direction: Rust → JSON Schema → generated TS).

**Branch:** `sp4c-edit-breadth`. These are Tasks **D1–D7**. They run **after 4C Task 26 and before Task 27**, as batch F′, so 4C's e2e, docs and validation tasks see them:
- 4C Task 28 also records D34 as built;
- 4C Task 29 adds the journal cost row on three OSes.

**Out of scope:**
- any network upload or receiving service;
- the dataset builder;
- emitters for the reserved agent, UI and review kinds (sub-projects 7, 9, 10);
- the consent UI and first-run prompt (sub-project 9);
- the agent permission rules that make `diagnostics purge` and `project default` human-only (sub-project 7; a TASKS follow-up);
- a blob store for large content (sub-project 7);
- crash minidumps (with the uploader).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code. `pnpm gates` runs cargo-deny and the JS licence gate.
- **`fin-core` stays pure:** `events/` does no I/O and reads no clock, environment or randomness. It builds for `wasm32-unknown-unknown`, which the existing gate checks.
- **Dependency rules:**
  - `fin-core` never depends on `fin-journal`;
  - `fin-journal` depends on `fin-core` only among our crates, and has no network stack;
  - `fin` does not link `fin-journal`: it sends events to `finisherd` and never writes the journal;
  - `fin-store` does not depend on `fin-journal`.
- **Model types are generated, never hand-written.** Every envelope, payload, status and RPC type derives `JsonSchema` and is added to `crates/fin-store/examples/export_schema.rs` in the task that creates it. Then run `pnpm --filter @finisher/core-types generate`; the drift gate is `scripts/check-generated-types.mjs`.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500.
  - Near the limit: `apps/finisherd/src/projects.ts` 520, `crates/fin/src/main.rs` 489, `crates/fin-store/src/project_file.rs` 469.
  - New logic goes into new files: `apps/finisherd/src/diagnostics.ts`, `crates/fin/src/diag.rs`, `crates/fin/src/commands/diagnostics.rs`, `crates/fin-store/src/collect.rs`, `crates/fin-node/src/journal.rs`.
- Errors are `{ code, message, details, fix? }`. New codes go into `crates/fin/src/output.rs` (Decision 33).

**Privacy invariants** (each has a named test)
- **Off means off.** With no purpose on:
  - no file and no directory under `events/` is created;
  - no payload is built (call sites check the gate first);
  - nothing is kept in memory.
- **No content without training.** No `content` field reaches disk unless `training` was on when the entry was written.
- **Never raw.** No raw path, host name, user name, IP or raw `fin` argument reaches disk in any mode. A test greps the journal for the test's temp dir, `$HOME`, `$USER` and the host name.
- **The project flag wins.** No entry with `ctx.project` = P is written while P is `never` or unknown.
- **Never blocks.** No emitter call waits on I/O. A full channel, a full disk or an unwritable directory never fails an edit.

**Testing and process**
- **Tests never use the network.**
- Clocks are injected (`fin_journal::Clock`) so rotation, age caps and stats windows are tested without sleeping.
- Crash tests spawn a child process and kill it (`std::process::Command`), as the 4A lease tests do.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`. Tasks that touch TS or `fin` also run `rtk pnpm e2e` once at the end.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh`.
- **The repo is public:** no host names, IPs, user names or key names in tracked files, fixtures included.
- **Tracker:** each task ticks its line in `TASKS.md` under a new heading "Diagnostics and training-data hooks (4C branch)", in the same commit.

## Decisions taken here (the spec fixes the shape; these fix the details)

### Envelope and kinds

1. **The types live in `fin-core::events`.** Events are data, like ops: pure, versioned, schema-exported. Emitters (`fin-journal` through `fin-node`, `finisherd`, `fin`) supply `id` (ULID, canonical upper case) and `ts` (integer milliseconds since the Unix epoch, UTC). Rejected: a separate types crate. It would split the single schema export and the generated-types pipeline for no gain.
2. **The envelope**, exactly:
   ```rust
   pub struct Envelope {
       pub v: u16,                 // ENVELOPE_VERSION = 1
       pub id: String,             // ULID
       pub ts: u64,                // ms since epoch
       pub session: String,        // the emitting process run (ULID)
       pub source: Source,         // fin | finisherd | finNode | ui | agent
       pub kind: String,           // from the registry
       pub kv: u16,                // the kind's payload version
       pub consent: ConsentStamp,  // {rev, purposes: [diagnostics|training], basis: default|user|env}
       pub ctx: Option<Ctx>,       // {project?, branch?, txId?, author?: Hashed, actor?: {kind: user|agent|plugin|system, session?: IdStr}}
       pub payload: serde_json::Value,
   }
   ```
   `ctx.actor.session` is the agent or UI session (`FIN_SESSION` for `fin`); `session` is always the process run.
3. **The kind registry is closed.**
   - `fin_core::events::kinds()` returns one `KindSpec {name, version, reserved, training_only, sources, schema}` per kind. Names are `domain.verb`, lower camel case within a segment.
   - The journal refuses an unknown kind or version (`EVENT_KIND_UNKNOWN`). Readers skip and count what they don't know.
   - Version 0 marks a **reserved** kind: its payload may change without a version bump until it gets a real emitter, which then sets version 1.
   - A changed payload is `kv + 1`. Old versions stay readable by the dataset builder. **There are no upcasters:** the journal is not canonical state.
   - Each payload type implements `trait Payload: Serialize + DeserializeOwned + JsonSchema { const KIND: &str; const VERSION: u16; }`.
4. **Field classes are types:**
   - `Content<T>` (`#[serde(transparent)]`; schema `x-fin-class: content`; the field names its kind with `#[schemars(extend("x-fin-content" = "prompt"))]`). The kinds are `userText`, `caption`, `fileName`, `prompt`, `modelInput`, `modelOutput`, `toolArgs`, `toolResult` and `mediaHash`.
   - `Scrubbed` (built only by `Scrubbed::new(text, &ScrubCtx)`; `x-fin-class: scrubbed`).
   - `Hashed` (`^k1:[0-9a-f]{32}$`), `IdStr` (`^[0-9A-Za-z][0-9A-Za-z_.:$-]{0,127}$`), `Code` (`^[A-Z][A-Z0-9_]{1,63}$`) and `Name` (`^[a-z][a-z0-9_. -]{0,63}$`), all `x-fin-class: meta`.
   - Numbers, booleans and enums are `meta` implicitly.
   - **A plain `String` anywhere in a payload fails `payload_strings_are_classified`.** The test walks every kind's schema and requires each string leaf to be an enum or to carry `x-fin-class`, and each content field to carry `x-fin-content`.
5. **Paths and hosts.**
   - A path is stored only as `PathRef {root: project|library|workspace|serverData|other, ext: Option<Name>, hash: Hashed}`. Extensions over 16 characters or not matching `Name` become `null`.
   - Platform facts are limited to: OS family, OS major version, arch, logical cores, memory rounded to a power-of-two GiB, app and protocol versions, build profile. No host, user, IP, MAC, serial, CPU or GPU model in this phase. GPU adapter names come in sub-project 6, as metadata, if the owner agrees then.
6. **Keyed hashes.**
   - BLAKE3 keyed with a 32-byte per-install key in `events/key` (0600), created on the first allowed write, truncated to 128 bits, with the prefix `k1:`.
   - `fin` sends its existing unkeyed `argsHash` (`blake3:<hex>`) over the local socket. The server stores `keyed(argsHash)`, so the stored value can't be reversed by dictionary off this machine.
   - `purge --all` deletes the key. A new key makes old and new hashes unlinkable; that is accepted.
7. **Linking to the op log.**
   - `ctx.txId` is stored as is when it matches `IdStr`. An idempotency key that doesn't match is stored keyed-hashed (`txIdHashed: true`), and the builder joins by hashing.
   - `ctx.author` is always keyed-hashed, because `FIN_AUTHOR` is free text.
   - The builder joins on this machine by hashing op-log authors with the same key.

### Consent

8. **Settings:** `settings.json` `privacy: {diagnostics?: "on"|"off", training?: "on"|"off", retentionDays?: 1..365, maxMiB?: 16..4096}`.
   - **Effective purpose** = `DO_NOT_TRACK=1` ? off : `FIN_<PURPOSE>` (`on|off`) ?? settings value ?? the build default.
   - `basis` is `env`, `user` or `default`, after whichever decided. When the two purposes have different bases, the stamp carries the weaker one: `default` < `env` < `user`.
   - An unusable `privacy` section (bad JSON, wrong type, out of range) means **both off** until fixed, with a stderr warning, as `readSettings` does for fonts.
9. **Defaults live in one table,** `fin_journal::consent::DEFAULTS`.
   - It holds the recommended answers to Q1 and Q2: **diagnostics on, training off**.
   - The Docker or headless image sets `FIN_DIAGNOSTICS=off` in its environment, pending Q2.
   - Changing a default is a one-line change plus a test-golden update.
10. **Purposes.**
    - With `training` on, entries keep their content fields.
    - With only `diagnostics` on, content fields are removed by `fin_core::events::redact` before serialisation, and `KindSpec.training_only` kinds are written as their metadata part. An `agent.turn`, for example, keeps its token counts and latency and drops its messages.
    - With only `training` on, entries are written with both parts and `purposes: [training]`. Training covers diagnostics-class fields too: they are context for the examples.
11. **Off means off. There is no in-memory diagnostic ring.** The gate is an `AtomicU8` of purposes, read with `Relaxed` before any payload is built. Why:
    - it is simple, and provable by a test;
    - "off" then means the same to a user and to a reviewer;
    - a ring would still build every payload.

    Debugging without consent uses the normal stderr logs, which this plan does not change.
12. **Consent changes.**
    - The effective consent is persisted with a monotonic `rev` in `events/state.json` (written temp-then-rename, created lazily).
    - A change writes `consent.changed {from, to, basis}` while the *old or new* consent allows it. A withdrawal is therefore recorded under the old consent just before the gate closes, then flushed.
    - Turning a purpose off deletes nothing by itself. `fin diagnostics purge` does, and the uploader re-checks consent per entry (Decision 31).
13. **The project flag is `finisher.project.json` `collect: "never"`.** Absent means "follow each user's settings"; `"default"` is written as absent.
    - **Why there, not the op log:**
      - it must be project-wide, not per branch;
      - it must not be undone by an undo or reintroduced by a merge;
      - it should not need a new primitive op and a schema bump;
      - the file already syncs with the folder, goes into bundles, is re-read by followers (`tick.rs` `reread_project_file`) and is kept by older Finishers through `ProjectFile.other`.
    - `Store::set_collect` needs the writer lease (`PROJECT_READ_ONLY` otherwise) and writes the file the usual way.
    - `fin-node` pushes each open project's policy into the journal at open and after every re-read. **Unknown means `never`:** a project not open on this server, or whose file can't be read.
    - Setting `never` makes the journal purge that project's entries (Decision 29). Every collaborator's server does the same when it reads the flag.
14. **Each entry carries the consent at write time** (`consent.rev`, `purposes`, `basis`). A future uploader that accepts only explicit consent can drop every `basis: default` entry.

### The journal

15. **One writer per server data: `finisherd`, through `fin-node`'s `Journal`.** `finisherd` already holds the server lock (`server_lock.rs`).
    - `fin` sends events over the local socket (`diagnostics.record`). The UI will do the same over RPC.
    - **Offline `fin` records nothing.** That is accepted: agents run with a server.
    - Why not a writer per process: thousands of short `fin` runs would make thousands of files, or need cross-process locking.
16. **Files.**
    - `events/j-<yyyymmdd>-<ulid>.jsonl`; the date is the UTC date of the file's first entry.
    - A file rotates at 8 MiB or at the first entry of a new UTC day.
    - **Caps:** `retentionDays` (default 30) and `maxMiB` (default 128) over all `j-*` files, oldest first, applied at open, at each rotation and hourly.
    - Other files: `events/key`, `events/state.json` and, later, `events/cursor-<uploader>.json`. Everything is 0600 and the directory is 0700.
17. **Crash safety.**
    - Every entry is one line ending in `\n`.
    - On open, the newest file's tail after its last `\n` is truncated. Readers skip lines that don't parse, and count them.
    - Closed files are fsynced, and so is the directory (a no-op on Windows, as in `fin-store::fsio`).
    - There is no per-line fsync: losing up to the last flush interval on power loss is accepted.
    - `error.panic` and `session.end` are written and flushed synchronously.
18. **Never blocks.**
    - `record` serialises nothing in the caller. It builds the payload struct, stamps `id`, `ts` and `ctx`, and calls `try_send` on a `sync_channel(8192)`.
    - A full channel drops the event and increments a counter. The writer reports it as `journal.dropped {count, since}` at its next flush.
    - An I/O error suspends the journal (`status.state = suspended`, with the error code), discards events while suspended and retries every 60 s.
    - A panic in the writer thread is caught: the journal suspends and editing is unaffected.
    - The writer waits in `recv_timeout(1 s)`, so an idle journal costs no CPU between flushes. It flushes every second or 64 KiB.
19. **High-rate timings are aggregated.**
    - `Journal::observe(metric, key, micros, code)` adds to a per-minute window of log₂ buckets (1 µs to 2⁴⁰ µs) per `(metric, key)`, plus count, error count per code, and max.
    - Each minute writes one `rpc.stats` or `eval.stats` entry per key.
    - A window holds at most 256 keys; extra keys fold into `"other"`.
20. **Sizes.** A serialised payload over 64 KiB is refused (`EVENT_TOO_LARGE`) and counted as dropped. Reserved agent kinds will need more; sub-project 7 adds a content-addressed blob store under `events/blobs/`, referenced by `Content<BlobRef>`.
21. **Cost budgets,** measured by `cargo run -p fin-journal --release --example journal_cost` and recorded in TASKS.md (Linux VM; 4C Task 29 repeats it on macOS and Windows):
    - disabled gate ≤ 5 ns per call;
    - enabled `record` p99 ≤ 5 µs in the caller;
    - writer ≥ 100k entries/s sustained;
    - `fin-node` commit overhead with recording on, against off: p50 ≤ 2 % and ≤ 20 µs on a 1k-item project;
    - zero entries and zero files when disabled.

    A budget miss blocks the batch.

### Sources

22. **`log.commit`** is one entry per attempt to append to the op log, emitted in `fin-node/src/tasks.rs` around every store call that commits: commit, undo, redo, merge, pick, revert, checkpoint, branch create and set.
    - **Payload:**
      - `record`: `edit|undo|redo|merge|pick|revert|checkpoint|branch|branchSet`;
      - `outcome`: `accepted|rebased|deduped|conflict|refused|dryRun`;
      - `code?`;
      - `ops`: op name → count;
      - `baseLag`: head − `baseRev` at entry;
      - `rebasedOver`: the number of txs;
      - `conflicts`: codes → count;
      - `links`: txIds, e.g. the tx an undo undid or the txs a merge copied;
      - `micros`.
    - **Outcome mapping:**
      - `accepted`: the result's `baseRev` equals the request's;
      - `rebased`: lower;
      - `deduped`: the idempotent return;
      - `conflict`: `CONFLICT`;
      - `refused`: any other error code;
      - `dryRun`: a dry run, whatever its result.
    - `ctx` = project, branch, txId and author (keyed). The txId is the committed one, or the request's on failure.
    - Undo, redo and revert entries are the **rejection signal**: `links` names the original txIds.
23. **`cli.command`.**
    - **Payload:** `name` (registry command, e.g. `item trim`); `flags` (the flag names parsed, sorted, never values); `argsHash` (keyed, per Decision 6); `micros`; `exit`; `code?`; `online`; `dryRun`; `txIds`.
    - It is sent once, at the end, by `crates/fin/src/diag.rs`, only when `finisherd.json` has `diagnostics.enabled: true`.
    - The call is awaited for at most 50 ms. Its errors are ignored and never change the exit code or stdout.
    - `ctx.actor = {kind: agent, session: FIN_SESSION}` when set, else `{kind: user}`.
24. **The trust boundary for `diagnostics.record`:**
    - the server sets `source` and `session` from the connection, never from the payload;
    - kinds are allow-listed per source (`fin`: `cli.command`, `error.panic`; `ui`: none yet);
    - 50 events/s per connection, burst 200; extra events are dropped and counted;
    - the 64 KiB cap applies.

    Fake events from an injected agent can only appear under the agent's own source and session, which the dataset builder can weigh. This is a Review Focus item.
25. **Sessions.**
    - `session.start {app, protocol, os, osMajor, arch, cores, memGiB, profile}` when the journal opens with any purpose on, or when one turns on.
    - `session.end {uptimeS}` on clean shutdown.
    - `state.json.openSession` holds the run. A start that finds it still set writes `session.unclean {previous}`.
26. **Errors.**
    - `error.internal {call, code, details: keys only}` for `INTERNAL` and the store's unexpected codes, as returned through `fin-node`.
    - `error.panic {where: finNode|finisherd|fin, message: Scrubbed, frames: [Name] (≤ 32, our crates' symbols only), thread}` comes from three places: a chained Rust panic hook in `fin-node`, `uncaughtException`/`unhandledRejection` in `finisherd`, and `fin`'s panic hook (sent best effort).
    - **Scrubbing** replaces the home dir, server data, temp dir, the user name, the host name, and any absolute path (`/…`, `C:\…`, `\\…`) with `<home>`, `<data>`, `<tmp>`, `<user>`, `<host>` and `<path>`.
27. **Other sources.**
    - `storage.state {state: readOnly|writable, reason}` on read-only transitions (lease lost, conflict copy, newer format), from engine status events.
    - `fonts.fetch {entry: Name, bytes, micros, outcome: ok|failed|cached, code?}` per fetch.
    - `rpc.stats` with RPC method names as keys.
    - `eval.stats` with keys `eval.frame`, `eval.audio` and `measure`.
28. **Reserved kinds (version 0, no emitter):**
    - `agent.session {provider, model, role, mode}`;
    - `agent.turn {turnId, provider, model, inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens, latencyMs, stopReason, costMicros?; messages: Content<modelInput>, output: Content<modelOutput>, systemHash, toolsHash}`;
    - `agent.toolCall {callId, turnId, command: Name, argsHash; args: Content<toolArgs>}`;
    - `agent.toolResult {callId, ok, code?, micros, txIds; output: Content<toolResult>}`;
    - `agent.editVerdict {txIds, verdict: accepted|rejected|undone|merged|abandoned, by: user|critic|agent}`;
    - `ui.action` and `review.verdict` as empty v0 payloads.

    `agent.turn`, `agent.toolCall` and `agent.toolResult` are `training_only`.
    The schema test covers them like the others.

### CLI, RPC, purge and export

29. **Purge** runs on the writer thread, so one owner touches the files. It rotates the active file, rewrites each affected closed file without the matching entries (temp-then-rename, fsync), and deletes files left empty. Selectors:
    - `all`, which also deletes `key` and `state.json`;
    - `project <id>`;
    - `before <date>`.
30. **`fin diagnostics`** (server only; `SERVER_UNAVAILABLE` offline):
    - `show [--recent <n>]` → `{consent {diagnostics, training, basis, rev}, env, state, files, bytes, kinds: {kind: count}, oldest, newest, dropped, projects: {id: never|default}}`;
    - `export [--since <date>] [--project <id>] [--kind <k>]… [--as-upload diagnostics|training]` → JSONL on stdout. Without `--as-upload` it prints what is stored, content included: it is the user's own data. With it, it prints exactly what an uploader would get now;
    - `purge (--all | --project <id> | --before <date>)` → `{removed, files}`;
    - `project never|default [--project <id>]` → `{project, collect}`.

    Purge and `project default` become human-only once permissions exist (a TASKS follow-up for sub-project 7).
31. **The upload view and the `Uploader` boundary.**
    ```rust
    pub trait Uploader: Send {
        fn name(&self) -> &str;                 // cursor file key, [a-z0-9-]{1,32}
        fn purposes(&self) -> Purposes;
        fn upload(&mut self, batch: &Batch) -> Result<(), UploadError>;   // ship bytes, nothing else
    }
    pub struct Batch { pub id: String, pub entries: Vec<Envelope>, pub cursor: Cursor }
    impl Journal { pub fn upload_once(&self, u: &mut dyn Uploader, max_entries: usize) -> Result<UploadReport, JournalError>; }
    ```
    - It reads closed files only, from the persisted cursor.
    - It keeps an entry only if both its stamped purposes and the **current** consent include one of the uploader's purposes, and its project is not `never` now.
    - It removes content unless training is current and stamped.
    - It advances the cursor only after `upload` returns `Ok`: at least once, deduplicated by `id` downstream.
    - There is no implementation with a network in this plan. `NoUpload` is the default, and tests use a recording uploader.
32. **RPCs** (`@finisher/contracts`, scope `diagnostics`):
    - `diagnostics.status`, `diagnostics.recent {n}`;
    - `diagnostics.export {filter, cursor?, limit ≤ 1000, asUpload?}` → `{lines, next?}`, paged;
    - `diagnostics.purge {selector}`, `diagnostics.project {projectId, collect}`;
    - `diagnostics.record {kind, kv, ctx?, payload}` (Decision 24).
33. **New codes:**
    - exit 3: `EVENT_KIND_UNKNOWN`, `EVENT_INVALID`, `EVENT_TOO_LARGE`. These are seen only by `diagnostics.record` callers; `fin` ignores them.
    - exit 5: `JOURNAL_UNAVAILABLE`, for `fin diagnostics …` when the journal is suspended or the server data is unwritable. Its `fix` names the reason.

### Later

34. **The dataset builder** (out of scope; spec §21.1) runs locally over the journal and the op log:
    - **examples:** the state at `baseRev` rendered like `fin view`; the instruction from the linked `agent.turn` or the `cli.command`; the ops;
    - **labels:** kept, undone or reverted (and how soon), merged, abandoned, conflict and its resolution, and verdicts;
    - only from `purposes ∋ training` entries.

    Nothing in this plan may make that join impossible: `log.commit` carries txIds and links, and `cli.command` carries txIds. The e2e checks the join (Task D7).

## Questions for the owner

1. **Default for `training`** (it covers user content and full LLM conversations).
   - **Recommended:** **off until the user opts in explicitly** (a first-run prompt in sub-project 9). Training on user content and LLM conversations likely needs explicit consent under GDPR Art. 6(1)(a), and user footage and transcripts can contain special-category data (Art. 9). Opt-out consent for this is weak in the EU and the UK.
   - **Alternative:** on with opt-out, as the request suggests. Then every entry made before a user's explicit choice is stamped `basis: default`, so a later policy can still drop them before upload (Decision 14).

   The plan implements the recommendation until answered; the change is one line.
2. **Default for `diagnostics`** (metadata only: codes, timings, counts, ids, keyed hashes).
   - **Recommended:** **on, with opt-out, on desktop; off by default in the Docker/headless image** (`FIN_DIAGNOSTICS=off` baked into the image env). A shared or NAS server records on behalf of other people, so its operator should opt in. This replaces §21's "telemetry opt-in only".
   - **Alternative:** on everywhere, or off everywhere until the uploader exists. Nothing is uploaded either way in this phase; the first upload will show a notice. A legal check is advised before upload (ePrivacy Art. 5(3) covers reading and storing on the device for analytics).

## Review Focus

1. **Off means off:** no directory, no file, no payload built, no ring.
   → D2 `disabled_journal_creates_nothing`, D3 `off_builds_no_payload` (a counting payload builder), D5 `commit_with_journal_off_touches_no_events_dir`.
2. **No content without training; never raw paths, hosts or users:**
   - the schema walk;
   - redaction per purpose;
   - scrubbing;
   - the e2e grep for temp dir, `$HOME`, `$USER` and the host name.

   → D1 `payload_strings_are_classified`, `redact_removes_every_content_path`; D3 `diagnostics_only_writes_no_content`; D7 leak grep.
3. **The project flag:**
   - it is read from a synced file written by another server;
   - unknown means `never`;
   - setting it purges;
   - a follower honours it without the lease.

   → D3, D5 `follower_honours_never_from_synced_file`.
4. **Never blocks editing:**
   - full channel;
   - unwritable `events/`;
   - disk-full injection;
   - a slow disk (an injected 2 s write stall leaves commit latency unchanged);
   - a panic in the writer.

   → D2, D5.
5. **Crash safety:** kill -9 mid-write leaves only whole lines after repair; the age and size caps hold across restarts. → D2 `crash.rs`.
6. **The `diagnostics.record` trust boundary:** source from the credential, the kind allow-list, the rate limit, the size cap. → D4.
7. **Purity and generated types:** `fin-core::events` builds for wasm32, reads no clock or env, and the TS types are generated. → D1, gates.
8. **Cost budgets** (Decision 21). → D2 example, D5 commit-overhead test, TASKS.md numbers.
9. **The join:** journal entries join the op log by txId, and an undo names the undone tx. → D7 e2e.

---

### Task D1: The event model in `fin-core` — tier: opus

**Files:**
- Create: `crates/fin-core/src/events/{mod.rs (Envelope, Source, Ctx, Actor, ConsentStamp, Purpose, Purposes, Basis, ENVELOPE_VERSION), kinds.rs (KindSpec, kinds(), Payload trait), class.rs (Content, Scrubbed, ScrubCtx, Hashed, IdStr, Code, Name, PathRef, ContentKind), redact.rs (content_paths(kind), redact(value, kind, purposes)), scrub.rs, payloads/{session.rs, log.rs, cli.rs, stats.rs, errors.rs, fonts.rs, storage.rs, consent.rs, agent.rs, reserved.rs}}`.
- Modify: `crates/fin-core/src/lib.rs` (`pub mod events`), `crates/fin-store/examples/export_schema.rs` (the envelope plus every payload type), `packages/core-types/schema/fin-core.schema.json`, `packages/core-types/src/generated.ts` (regenerated).
- Test: `crates/fin-core/tests/events.rs`, `crates/fin-core/tests/events_golden/*.json`.

**Interfaces:** Decisions 1–5, 7, 22–28 (the payload structs exactly as listed there).
- `pub fn kinds() -> &'static [KindSpec]`, `pub fn kind(name: &str, version: u16) -> Option<&'static KindSpec>`.
- `pub fn validate_payload(spec: &KindSpec, payload: &Value) -> Result<(), CoreError>` (`EVENT_INVALID`): a typed round trip through the payload struct, with no schema engine.
- `pub fn redact(kind: &KindSpec, payload: &mut Value, purposes: Purposes)` removes content paths unless training is set. `content_paths` is computed from the kind's schema (`x-fin-class: content`), so a new content field needs no second list.
- `Scrubbed::new(text, &ScrubCtx {home, server_data, tmp, user, host})`, pure.

- [ ] **Step 1: Write failing tests:**
  - `envelope_json_matches_golden` (camelCase, omitted `ctx`);
  - `every_kind_round_trips` (a sample per kind, reserved included);
  - `payload_strings_are_classified` (the schema walk, Decision 4);
  - `content_fields_name_their_kind`;
  - `redact_removes_every_content_path`;
  - `redact_keeps_content_with_training`;
  - `training_only_kinds_keep_metadata_under_diagnostics`;
  - `unknown_kind_and_version_refused`;
  - `oversized_name_or_code_rejected_by_pattern`;
  - `scrub_replaces_home_data_tmp_user_host_and_absolute_paths` (POSIX, `C:\` and UNC cases);
  - `path_ref_never_keeps_the_path`;
  - `events_module_is_pure` (a source grep for `std::time`, `std::env`, `std::fs` and `getrandom` in `events/`).
- [ ] **Step 2: Implement.** Generate the TS types and check that `generated.ts` exports `Envelope` and a payload union keyed by `kind`.
- [ ] **Step 3: Run** the three gates (the wasm32 build of `fin-core` included) → PASS.
- [ ] **Step 4: Commit** `feat(core): versioned event envelope, kind registry and field classes`.

### Task D2: `fin-journal` — writer, files, crash safety, stats, cost — tier: opus

**Files:**
- Create: `crates/fin-journal/{Cargo.toml, src/lib.rs, src/writer.rs, src/files.rs, src/read.rs, src/stats.rs, src/clock.rs, examples/journal_cost.rs}`. Add the crate to the workspace `default-members`.
- Test: `crates/fin-journal/tests/{journal.rs, crash.rs, caps.rs, stats.rs}`, plus a `crash_child` helper binary under `tests/support/`.

**Interfaces:**
```rust
pub struct Journal { /* Arc<Inner> */ }
pub struct JournalOptions { pub max_file_bytes: u64, pub max_total_bytes: u64, pub max_age: Duration,
    pub channel: usize, pub flush_every: Duration, pub flush_bytes: usize, pub stats_every: Duration,
    pub clock: Arc<dyn Clock> }
impl Journal {
    pub fn open(dir: &Path, opts: JournalOptions) -> Journal;     // never fails; unusable dir → suspended
    pub fn disabled() -> Journal;
    #[inline] pub fn gate(&self) -> Option<Purposes>;             // one Relaxed load
    pub fn set_purposes_for_test(&self, p: Purposes);             // D3 replaces with consent
    pub fn record<P: Payload>(&self, source: Source, ctx: Option<Ctx>, p: P);
    pub fn record_value(&self, source: Source, session: &str, kind: &str, kv: u16,
                        ctx: Option<Ctx>, payload: Value) -> Result<(), CoreError>;
    pub fn observe(&self, metric: Metric, key: &str, micros: u64, code: Option<&str>);
    pub fn record_sync<P: Payload>(&self, source: Source, ctx: Option<Ctx>, p: P);   // panic, session.end
    pub fn flush(&self) -> Result<(), JournalError>;
    pub fn status(&self) -> JournalStatus;   // {state: off|on|suspended, code?, files, bytes, dropped, oldest?, newest?}
    pub fn read(&self, filter: &Filter) -> impl Iterator<Item = Envelope>;   // flushes first; skips and counts torn lines
    pub fn close(self);
}
```
Decisions 15–21. The directory is created only by the first write.

- [ ] **Step 1: Write failing tests:**
  - `disabled_journal_creates_nothing`;
  - `record_then_read_round_trips`;
  - `rotates_at_size_and_at_utc_day` (injected clock);
  - `age_and_size_caps_delete_oldest` (also `caps_hold_across_reopen`);
  - `torn_tail_is_truncated_on_open`;
  - `reader_skips_bad_lines_and_counts`;
  - `full_channel_drops_and_reports_journal_dropped`;
  - `unwritable_dir_suspends_and_retries` (0500 dir on unix; a file in place of the dir on every OS);
  - `writer_panic_suspends_not_crashes`;
  - `slow_disk_never_blocks_record` (an injected 2 s stall; `record` p99 stays under the budget);
  - `record_sync_is_on_disk_when_it_returns`;
  - `stats_window_writes_one_entry_per_key_per_minute`, `stats_overflow_folds_into_other`;
  - `files_are_0600_dir_0700` (unix);
  - `no_network_crates_in_tree` (`cargo tree -p fin-journal` contains no `ureq`, `rustls` or `fin-fonts`);
  - `crash.rs::kill_mid_write_leaves_whole_lines` (10 runs, the child killed at random points).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Measure** with `rtk cargo run -p fin-journal --release --example journal_cost`: disabled gate, `record` p50/p99, writer throughput, bytes per entry. Check them against Decision 21 and record them in TASKS.md.
- [ ] **Step 4: Run** the gates → PASS. **Commit** `feat(journal): local append-only event journal with caps, crash repair and stats`.

### Task D3: Consent, the project flag, redaction at write, purge, export and the upload boundary — tier: opus

**Files:**
- Create:
  - `crates/fin-journal/src/{consent.rs (PrivacySettings, EnvOverrides, DEFAULTS, resolve, rev in state.json), key.rs (install key, keyed hash), projects.rs (policy map, unknown = never), purge.rs, export.rs (Filter, AsUpload), upload.rs (Uploader, Batch, Cursor, NoUpload, upload_once)}`;
  - `crates/fin-store/src/collect.rs` (`CollectPolicy {Default, Never}`, `Store::collect_policy()`, `Store::set_collect(policy)`).
- Modify: `crates/fin-journal/src/{lib.rs, writer.rs}` (the gate from resolved consent, the stamp, redaction before serialising, the project check), `crates/fin-store/src/{project_file.rs (collect field), lib.rs}`, `crates/fin-store/examples/export_schema.rs` (`PrivacySettings`, `JournalStatus`, `CollectPolicy`, `Filter`).
- Test: `crates/fin-journal/tests/{consent.rs, privacy.rs, purge.rs, upload.rs}`, `crates/fin-store/tests/collect.rs`.

**Interfaces:**
```rust
impl Journal {
    pub fn apply_settings(&self, privacy: Option<&Value>);         // the raw `privacy` section; env read once at open
    pub fn consent(&self) -> ConsentState;                         // {diagnostics, training, basis, rev, env}
    pub fn set_project_policy(&self, project: &str, policy: Option<CollectPolicy>); // None = unknown
    pub fn hash(&self, raw: &str) -> Option<Hashed>;               // None while off (no key is made)
    pub fn path_ref(&self, root: PathRoot, path: &Path) -> Option<PathRef>;
    pub fn purge(&self, sel: PurgeSelector) -> Result<PurgeReport, JournalError>;
    pub fn export(&self, filter: &Filter, as_upload: Option<Purposes>) -> impl Iterator<Item = Envelope>;
    pub fn upload_once(&self, u: &mut dyn Uploader, max_entries: usize) -> Result<UploadReport, JournalError>;
}
```
Decisions 6–14, 29, 31. `Store::set_collect` is refused `PROJECT_READ_ONLY` without the lease. It writes `collect` only when the value is `never`, and removes the key for `default`.

- [ ] **Step 1: Write failing tests:**
  - `resolve_precedence` (a table test: DO_NOT_TRACK > env > settings > default; off wins; basis);
  - `unusable_privacy_section_means_off`;
  - `consent_rev_is_monotonic_across_reopen`;
  - `withdrawal_is_recorded_then_gate_closes`;
  - `off_builds_no_payload`;
  - `diagnostics_only_writes_no_content`, `training_writes_content`, `stamp_matches_consent_at_write`;
  - `never_project_entries_dropped`, `unknown_project_counts_as_never`, `setting_never_purges_that_project`;
  - `purge_all_removes_key_and_state`, `purge_before_date`, `purge_project_rewrites_files_atomically` (kill during purge leaves the old or the new file, never a mix);
  - `keyed_hash_is_stable_and_not_plain_blake3`, `no_key_created_while_off`;
  - `export_as_upload_applies_current_consent` (written with training on, then training off → no content in the upload view);
  - `upload_cursor_advances_only_on_ok`, `upload_reads_closed_files_only`, `upload_skips_never_projects_now`;
  - fin-store: `set_collect_needs_lease`, `collect_round_trips_and_older_file_keeps_it` (`other` is preserved), `follower_reads_collect_after_reread`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(journal,store): consent per purpose, project never-collect flag, purge, export and upload boundary`.

### Task D4: `fin-node` and `finisherd` wiring; `diagnostics.*` RPCs — tier: sonnet

**Files:**
- Create: `crates/fin-node/src/journal.rs` (napi `Journal`: `open(serverData)`, `applySettings(json)`, `record(kind, kv, ctxJson, payloadJson, source, session)`, `observe(metric, key, micros, code)`, `status()`, `recent(n)`, `export(filterJson, cursor, limit, asUpload)`, `purge(selectorJson)`, `setProjectPolicy`, `close()`; the chained panic hook), `apps/finisherd/src/diagnostics.ts` (Layer: open, settings watch, sessions, uncaught handlers, RPC timing, handlers, `diagnostics.record` guard).
- Modify:
  - `crates/fin-node/src/{lib.rs, Cargo.toml}` (adds `fin-journal`);
  - `apps/finisherd/src/{config.ts (Settings.privacy validation; ServerInfo.diagnostics), main.ts, transport.ts (timing middleware → observe), handlers.ts}`;
  - `packages/contracts/src/rpc.ts` (Decision 32), `crates/fin/tests/rpc_contract.rs`, `crates/fin-store/examples/export_schema.rs`, generated types.
- Test: `packages/fin-node/test/journal.test.ts`, `apps/finisherd/test/diagnostics.test.ts`.

**Interfaces:** Decisions 8, 15, 24–26, 32.
- `finisherd.json` gains `diagnostics: {enabled: boolean}`, rewritten (temp-then-rename) when the gate opens or closes.
- The `privacy` settings are re-read on the existing `watchFile` poll.

- [ ] **Step 1: Write failing tests:**
  - `settings_privacy_applies_live`, `bad_privacy_section_turns_off_and_warns`;
  - `env_overrides_settings`, `do_not_track_forces_off`;
  - `server_info_reflects_gate`;
  - `session_start_end_and_unclean`;
  - `uncaught_exception_is_error_panic_scrubbed`;
  - `rpc_timing_reaches_rpc_stats` (an injected clock or a 1 s window under test hooks);
  - `record_rpc_sets_source_from_credential`, `record_rpc_refuses_unlisted_kind`, `record_rpc_rate_limited`, `record_rpc_size_cap`;
  - `diagnostics_status_export_purge_project_rpcs`, `export_pages_with_cursor`;
  - RPC contract fixtures.
- [ ] **Step 2: Implement.** **Run** the three gates and `rtk pnpm e2e` → PASS.
- [ ] **Step 3: Commit** `feat(node,server): journal in finisherd, privacy settings, sessions, RPC stats and diagnostics RPCs`.

### Task D5: Edit-path sources in `fin-node` — tier: sonnet

**Files:**
- Modify:
  - `crates/fin-node/src/tasks.rs` (one wrapper around the dispatch: time the call, classify the outcome, build `log.commit` only when `gate()` is `Some`);
  - `crates/fin-node/src/{engine.rs (project policy at open and after each re-read; storage.state from status events), history.rs (record kinds and links from `HistoryResult`/`MergeReport`), eval.rs (observe `eval.frame`, `eval.audio`, `measure`), fonts/ (fonts.fetch)}`.
- Test: `packages/fin-node/test/journal-sources.test.ts`; `crates/fin-node/examples/commit_overhead.rs` (runs under test hooks; the budget of Decision 21).

**Interfaces:** Decisions 7, 13, 22, 26, 27.
- No store API changes beyond D3's `collect_policy()`.
- `links` come from the existing results: an undo's undone txIds, a merge's copied txIds, a pick's source tx.

- [ ] **Step 1: Write failing tests:**
  - `commit_outcomes_accepted_rebased_deduped_conflict_refused_dryrun` (one each, with codes, `baseLag` and `rebasedOver`);
  - `undo_links_the_undone_tx`, `redo_and_revert_link`, `merge_links_copied_txs`, `pick_links_source`;
  - `checkpoint_and_branch_records`;
  - `ctx_author_is_hashed`, `idem_key_txid_that_is_not_an_id_is_hashed`;
  - `internal_error_is_error_internal`;
  - `read_only_transition_is_storage_state`;
  - `font_fetch_events`, `eval_stats_keys`;
  - `commit_with_journal_off_touches_no_events_dir`;
  - `follower_honours_never_from_synced_file` (two engines on one folder, the second as follower);
  - `journal_stall_does_not_slow_commits`.
- [ ] **Step 2: Implement.** **Measure** with `rtk cargo run -p fin-node --release --example commit_overhead --features test-hooks`: on against off, and on with a stalled journal. Check the budget and record it in TASKS.md.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(node): op-log, history, eval, font and storage events linked by txId`.

### Task D6: `fin` — `cli.command` and `fin diagnostics` — tier: sonnet

**Files:**
- Create: `crates/fin/src/diag.rs` (read `diagnostics.enabled` from the discovery file already loaded; build and send `cli.command` with a 50 ms budget; the panic hook), `crates/fin/src/commands/diagnostics.rs`.
- Modify: `crates/fin/src/{main.rs (one call after dispatch, before exit), registry.rs, map.rs, output.rs (Decision 33 codes), client.rs or a new client_diag.rs}`.
- Test: `crates/fin/tests/cli_diagnostics.rs`.

**Interfaces:** Decisions 23 and 30. The flag names come from the registry's parse. Values, positional arguments and stdin are never sent.

- [ ] **Step 1: Write failing tests:**
  - `cli_command_sent_when_enabled`, `nothing_sent_when_disabled` (a counting server);
  - `cli_command_has_no_values` (a run with a unique marker in every argument, then a grep of the journal);
  - `args_hash_is_keyed_on_disk`;
  - `slow_server_never_delays_exit_beyond_budget`;
  - `record_failure_never_changes_exit_or_stdout`;
  - `agent_actor_from_fin_session`;
  - `diagnostics_show`, `diagnostics_show_recent`, `diagnostics_export_filters_and_as_upload`, `diagnostics_purge_selectors`, `diagnostics_purge_needs_a_selector_is_usage`, `diagnostics_project_never_then_default`, `diagnostics_offline_is_server_unavailable`, `journal_unavailable_exit_5`;
  - `map_still_under_budget`.
- [ ] **Step 2: Implement.** **Run** `rtk cargo test -p fin` (`FIN_REQUIRE_SERVER=1`), the gates and `rtk pnpm e2e` → PASS.
- [ ] **Step 3: Commit** `feat(fin): record command invocations; fin diagnostics show, export, purge and project`.

### Task D7: e2e, docs and tracker — tier: sonnet

**Files:**
- Create: `scripts/e2e/diagnostics.mjs`.
- Modify: `scripts/e2e.mjs` (`SCENARIOS.diagnostics`), `scripts/e2e.test.mjs`, `README.md` (a "Privacy and diagnostics" section: what is recorded, where, the switches, `fin diagnostics`), `docs/specs/2026-11-01-finisher-architecture-design.md` (§21.1 "as built" notes and any decision that changed), `TASKS.md`.

- [ ] **Step 1: Write the scenario,** driven by `fin` only:
  1. Start with `FIN_DIAGNOSTICS=off FIN_TRAINING=off`. Make a project, run 3 edits and an undo. Check: `events/` does not exist, and `fin diagnostics show` says off.
  2. Restart with diagnostics on. Run 3 edits, an undo, a stale edit that rebases, and a conflicting one. Then check:
     - `export` holds `session.start`, `log.commit` entries with the right outcomes, and `cli.command` entries whose txIds equal the committed ones;
     - the undo's `links` names the undone tx;
     - no content fields exist;
     - **join check:** every `log.commit` txId resolves through the `tx.get` RPC (the e2e RPC helper in `scripts/e2e/lib.mjs`).
  3. **Leak grep** over `events/`: no temp-dir path, `$HOME`, `$USER`, host name or argument marker.
  4. `fin diagnostics project never`, then one edit. Check: no entries for that project, and the earlier ones are purged. A second server following the folder also writes none for it.
  5. Turn training on, then off. Check:
     - `consent.changed` entries;
     - `export --as-upload training` is empty after the withdrawal;
     - `purge --all` leaves no `key`.
- [ ] **Step 2: Docs.**
  - The README section.
  - In the spec, the §21.1 as-built notes: measured costs, any deviation, and the owner's answers to Q1 and Q2 when given.
  - TASKS follow-ups:
    - the uploader and its notice;
    - agent kinds and the blob store (7);
    - UI kinds and the consent screens (9);
    - review verdicts (10);
    - human-only `purge` and `project default` (7);
    - the dataset builder;
    - GPU facts (6);
    - a legal review before the first upload.
- [ ] **Step 3: Run** `rtk pnpm e2e`, the gates → PASS. **Commit** `test(e2e): diagnostics journal scenario; docs: privacy and diagnostics as built`.
