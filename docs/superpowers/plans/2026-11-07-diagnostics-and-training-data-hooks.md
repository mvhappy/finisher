# Diagnostics and training-data hooks — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (the event model and its classes, the journal's crash and concurrency behaviour, consent, redaction, purge, blobs, the upload view, and classifying commit outcomes), `sonnet` for plumbing (napi, RPC, settings, the event sources, CLI, e2e, docs).

**Goal:** Finisher can record what it needs to improve the app and, later, to fine-tune an open-weight model on video editing. That covers what belongs in an edit and what does not, and everything that passes through an LLM. Upload, agent traces and the dataset builder come in later sub-projects, as additions with no rewrite. After this plan:
- one versioned event envelope with generated TS types, a closed kind registry, one closed payload enum, and a class (`meta`, `scrubbed` or `content`) on every payload field;
- a local, append-only, capped, crash-safe journal in `<server-data>/events/` that never blocks editing and costs nothing when off, with a content-addressed blob store whose lifecycle follows the entries that reference it;
- consent from day one, in two tiers: **basic diagnostics** (metadata only, on by default everywhere, opt-out) and **training content** (only after an explicit opt-in that an agent cannot give), env overrides, a per-project `collect: never` flag that travels with the project folder, and the consent recorded in every entry;
- the first sources: op-log commits (edits, undo, redo, merges, picks, reverts, checkpoints, branches, recovery) classified from typed results and linked by `txId` and actor, `fin` invocations, RPC and eval timings, font fetches, storage state, errors, panics and sessions;
- the reserved agent kinds that sub-project 7 fills in;
- `fin diagnostics show|consent|export|purge|project`;
- the upload view (`export --as-upload`), which defines exactly what a future uploader may ship. There is no uploader, trait or cursor in this plan.

**Architecture:**
- **`fin-core::events`** (new module, pure, wasm-safe) holds the envelope, the kind registry, the closed `AnyPayload` enum, the payload types and the class newtypes (`Content<T>`, `Scrubbed`, `Hashed`, `IdRef`, `Code`, `Name`, `SrcLoc`, `PathRef`, `BlobRef`). It also holds the static name tables, and the pure functions that list a kind's content paths from its JSON Schema, remove them (`redact`), list the blobs an entry references, and scrub text. It never reads a clock or makes an id: the emitter supplies `id` and `ts`.
- **`fin-journal`** (new crate, I/O, no network) is the only writer:
  - a writer thread behind a data channel bounded by entries and bytes, plus a non-dropping control channel; rotating JSONL files with size and age caps; torn-tail repair;
  - per-minute histograms for high-rate timings;
  - the consent gate and its resolution; the per-install key for keyed hashes; the set of `never` projects;
  - blobs with ref-counted garbage collection; purge, export and the upload view;
  - the cost example.
- **`fin-store`** gains:
  - `collect` in `finisher.project.json`, cached in `collect.rs` and re-read with the project file, with a new maintenance event `{"type":"collect"}`;
  - a write-outcome note (`Store::take_write_outcome()`) that tells a written, a deduped and a planned (dry-run) write apart.

  It emits no journal entries itself.
- **`fin-node`** opens one `Journal` per server data (like the font manager). It classifies every op-log write from typed results in a new `commit_log.rs`, called inside `with_store` from `tasks.rs` and `history.rs` (`recover` included). It feeds the project flags from open and the `collect` event, installs the panic hook, and exposes a napi `Journal` class.
- **`finisherd`:**
  - reads `privacy` from `settings.json` before it opens the journal, keeps watching it (as `fonts.ts` does for `fonts.autoFetch`), and writes it through a new merging settings writer;
  - builds its handler layer once per transport with an injected `Source` (local socket → `fin`, WebSocket → `ui`);
  - publishes `diagnostics.enabled` in `finisherd.json`;
  - times every non-stream RPC into `rpc.stats`;
  - records `session.*` and uncaught errors;
  - serves `diagnostics.*` RPCs: `diagnostics.record` for `fin`, with a kind allow-list per source, a rate limit per source and a size cap; and `diagnostics.consent`, which only a `ui` source may use to turn training on.
- **`fin`:**
  - sends an `actor` (kind, session, call) in the headers of every RPC request;
  - fire-and-forgets one `cli.command` per invocation, unless its own `DO_NOT_TRACK`/`FIN_DIAGNOSTICS=off` says not to or `finisherd.json` says recording is off. It sends the registry name, the flag names, the unkeyed `argsHash` (the server keys it), the duration, the exit code and the txIds;
  - gets `fin diagnostics …`, which can never turn training on.

**Tech Stack:**
- Rust 2024, `serde`/`serde_json`, `schemars` 1 (`extend` attributes for `x-fin-class` and `x-fin-content`), `ulid` 1, `blake3` 1 (keyed mode), `getrandom` 0.3 (MIT OR Apache-2.0, already in the lock file) for the install key, `std::sync::mpsc` (`sync_channel` for data, `channel` for control).
- **No new third-party crates beyond those.** No `erased-serde`: the payload is one closed enum. No compression in this phase. No HTTP: `fin-journal` must not depend on `ureq`, `rustls` or `fin-fonts`, and a test checks it with `cargo tree`.
- Node 24, Effect 4, vitest.

**Spec:** `docs/specs/2026-11-01-finisher-architecture-design.md` §21.1 and D34. Related:
- §6.3–6.4 (txIds, records);
- §6.7 (`finisher.project.json` is rewritten temp-then-rename and synced);
- §8.4 (`fin` runs in the agent sandbox; the env allow-list keeps only `FIN_SERVER` and `FIN_SESSION` of ours);
- §16 (scopes; the audit log is separate from this journal);
- §21 (rows replaced by D34);
- §27.12 (schema direction: Rust → JSON Schema → generated TS).

**Branch:** `sp4c-edit-breadth`. These are Tasks **D1–D7**. They run **after 4C Task 26 and before Task 27**, as batch F′, so 4C's e2e, docs and validation tasks see them:
- 4C Task 28 also records D34 as built;
- 4C Task 29 adds the journal cost row on three OSes.

**Out of scope:**
- any network upload or receiving service, and the `Uploader` trait, batches and cursors (they come with the uploader sub-project; the upload view of Decision 31 is their contract);
- the dataset builder (Decision 34);
- emitters for the reserved agent, UI and review kinds (sub-projects 7, 9, 10), and any emitter of frames or audio (the blob types and their lifecycle ship; nothing writes a frame yet);
- the consent UI and first-run prompt (sub-project 9);
- the agent permission rules that make `diagnostics purge`, `project default` and exporting content human-only (sub-project 7; a TASKS follow-up);
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
  - Near the limit (lines today): `crates/fin-store/src/tick.rs` 590, `apps/finisherd/src/projects.ts` 520, `crates/fin/src/client.rs` 525, `crates/fin-store/src/project_file.rs` 512, `crates/fin/src/backend.rs` 493, `crates/fin/src/main.rs` 489, `crates/fin-store/src/lib.rs` 463.
  - `tick.rs` changes by one line at most (Decision 13). `projects.ts`, `client.rs` and `main.rs` take only call-site lines; their logic goes elsewhere.
  - New logic goes into new files: `apps/finisherd/src/{diagnostics.ts, settings-write.ts}`, `crates/fin/src/diag.rs`, `crates/fin/src/commands/diagnostics.rs`, `crates/fin-store/src/collect.rs`, `crates/fin-node/src/{journal.rs, commit_log.rs}`.
- Errors are `{ code, message, details, fix? }`. New codes go into `crates/fin/src/output.rs` (Decision 33).

**Privacy invariants** (each has a named test)
- **Off means off.** With no purpose on:
  - no file and no directory under `events/` is created;
  - no payload is built (call sites check the gate first);
  - nothing is kept in memory.
- **Nothing before settings.** The journal opens with the consent already resolved from `settings.json` and the env, so no entry is written under the build default when the user said off.
- **No content without training.** No `content` field, and no blob, reaches disk unless `training` was on both when the entry was recorded and when the writer serialises it.
- **Never raw.** No raw path, host name, user name, IP or raw `fin` argument reaches disk in any mode. A test greps the journal and the blobs for the test's temp dir, `$HOME`, `$USER` and the host name.
- **No free names.** A `Name` holds only a `&'static str` from code or a static table; nothing a user typed can become one.
- **The project flag wins.** No entry with `ctx.project` = P is written while P is `never` or unknown, and setting `never` removes P's entries and their blobs.
- **Agents cannot opt in.** No call that `fin` can make turns training on.
- **Never blocks.** No emitter call waits on I/O, except the synchronous panic and shutdown writes, which have a timeout. A full channel, a full disk or an unwritable directory never fails an edit.

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
       pub ctx: Option<Ctx>,       // {project?, branch?, txId?: IdRef, author?: Hashed, actor?: Actor}
       pub payload: serde_json::Value,
   }
   pub struct Actor { pub kind: ActorKind, pub session: Option<IdRef>, pub call: Option<IdRef> }
   pub enum ActorKind { User, Agent, Plugin, System, Unknown }
   ```
   - `session` is always the process run. `ctx.actor.session` is the agent or UI session (`FIN_SESSION` for `fin`). `ctx.actor.call` is the tool-call id an agent harness sets (`FIN_CALL` for `fin`, Decision 23).
   - The payload is `Value` only on the wire and on disk. In memory it is `AnyPayload` (Decision 3), serialised on the writer thread.
3. **The kind registry and the payload enum are closed.**
   - `fin_core::events::kinds()` returns one `KindSpec {name, version, reserved, training_only, sources, schema}` per kind. Names are `domain.verb`, lower camel case within a segment.
   - **`pub enum AnyPayload`** has one variant per kind, reserved ones included, each holding that kind's payload struct (large ones boxed). It has `kind() -> &'static KindSpec`, `approx_bytes() -> usize` (string and vector lengths plus a fixed overhead per variant; no serialisation) and `blobs() -> impl Iterator<Item = &BlobRef>`. `Serialize` writes the payload part only.
   - `validate_payload(spec, &Value) -> Result<AnyPayload, CoreError>` decodes a received payload (`EVENT_INVALID`); it is the only way a `Value` becomes a payload. Rejected: generic `record<P: Payload>` with type erasure (it needs `erased-serde` or serialisation in the caller).
   - The journal refuses an unknown kind or version (`EVENT_KIND_UNKNOWN`). Readers skip and count what they don't know.
   - Version 0 marks a **reserved** kind: its payload may change without a version bump until it gets a real emitter, which then sets version 1.
   - A changed payload is `kv + 1`. Old versions stay readable by the dataset builder. **There are no upcasters:** the journal is not canonical state.
4. **Field classes are types:**
   - `Content<T>` (`#[serde(transparent)]`; schema `x-fin-class: content`; the field names its kind with `#[schemars(extend("x-fin-content" = "prompt"))]`). The kinds are `userText`, `caption`, `fileName`, `prompt`, `modelInput`, `modelOutput`, `toolArgs`, `toolResult`, `mediaHash`, `txIntent`, `errorMessage`, `frame` and `audio`. `frame` and `audio` always hold a `BlobRef` (Decision 20).
   - `Scrubbed` (built only by `Scrubbed::new(text, &ScrubCtx)`; `x-fin-class: scrubbed`). In this plan it appears only inside `Content` (the panic message, Decision 26): scrubbing still applies to content, because "never raw" holds in every mode.
   - `Hashed` (`^k1:[0-9a-f]{32}$`), `Code` (`^[A-Z][A-Z0-9_]{1,63}$`), `IdRef` (Decision 7), `SrcLoc` (Decision 26) and `Name` (`^[a-z][A-Za-z0-9_./-]{0,63}$`), all `x-fin-class: meta`. The `Name` pattern admits upper case after the first character and `/` because the tables it names need them: RPC methods are camel case (`history.cherryPick`) and media types hold a slash (`image/webp`). It has no space, because every table name is dotted (`item.trim`).
   - **`Name` has no free constructor.** Its field is private. Inside `fin-core` it is built by `Name::from_static(&'static str)`, a crate-private `const fn` that asserts the pattern in every build (at compile time in a `const` or `static`; a test also checks every use takes a literal). Everywhere it is built by `NameTable::get(&self, s: &str) -> Option<Name>` (or `get_or_other`, or `Name::OTHER`) over one of the static tables in `fin_core::events::names`: `CLI_COMMANDS`, `CLI_FLAGS`, `RPC_METHODS`, `MEDIA_EXTS`, `EVAL_KEYS`, `MEDIA_TYPES`. A table is a `&'static [&'static str]`, so a `Name` always holds a `&'static str`; `NameTable::new` is crate-private too and checks every entry against the pattern when the `static` is evaluated. Deserialising a `Name` field goes through its table (`#[serde(deserialize_with = "names::cli_command")]` and the like); a value not in the table becomes `"other"`. There is no `From<String>`, `FromStr`, `Default` or plain `Deserialize` for `Name`.
   - Drift tests keep the tables honest: `fin` checks that its registry's command and flag names are in `CLI_COMMANDS`/`CLI_FLAGS` (D6), and `finisherd` checks `RPC_METHODS` against `FinRpcs` (D4).
   - Numbers, booleans and enums are `meta` implicitly.
   - **A plain `String` anywhere in a payload fails `payload_strings_are_classified`.** The test walks every kind's schema and requires each string leaf to be an enum or to carry `x-fin-class`, and each content field to carry `x-fin-content`. `name_has_no_free_constructor` checks the `Name` rules by a source scan of `events/` and a compile-fail doctest.
5. **Paths and hosts.**
   - A path is stored only as `PathRef {root: project|library|workspace|serverData|other, ext: Option<Name>, hash: Hashed}`. `ext` comes from `MEDIA_EXTS` (lower-cased); any other extension is `null`. The type ships now; the journal has no path helper yet, because no source in this plan stores a path.
   - Platform facts are limited to: OS family, OS major version, arch, logical cores, memory rounded to a power-of-two GiB, app and protocol versions, build profile. No host, user, IP, MAC, serial, CPU or GPU model in this phase. GPU adapter names come in sub-project 6, as metadata, if the owner agrees then.
6. **Keyed hashes.**
   - BLAKE3 keyed with a 32-byte per-install key in `events/key` (0600), created on the first allowed write, truncated to 128 bits, with the prefix `k1:`.
   - `fin` sends its existing unkeyed `argsHash` (`blake3:<hex>`) over the local socket. The server stores `keyed(argsHash)`, so the stored value can't be reversed by dictionary off this machine.
   - `purge --all` deletes the key. A new key makes old and new hashes unlinkable; that is accepted.
7. **Ids and linking to the op log.**
   - **`IdRef`** is a string that is either a canonical ULID (`^[0-9A-HJKMNP-TV-Z]{26}$`), stored raw, or a keyed hash (`k1:…`). Anything that is not a ULID is keyed-hashed before it is stored, whatever its pattern: an idempotency key or a session name may carry a user or host name. The prefix tells the two apart, so there is no `txIdHashed` flag.
   - `ctx.txId`, `ctx.actor.session`, `ctx.actor.call` and every txId in a payload (`links`, `txIds`) are `IdRef`s.
   - `ctx.author` is always keyed-hashed, because `FIN_AUTHOR` is free text.
   - **Store-generated ids are stored raw.** A project id (`ProjectId`, a ULID in lower case, `^[0-7][0-9a-hjkmnp-tv-z]{25}$`, as `fin-store` makes them) and a branch id (`BranchId`, `main` or `br` and any number of digits, as the store accepts) are allocated by the store and never typed, so they are kept as they are and compared with the project flags (Decision 13). A branch *name*, which a user types, is never stored. D3 and D6 lower-case a `--project` given in upper case (or refuse it) before it is compared or recorded.
   - The builder joins on this machine by applying the same rule to op-log txIds and authors with the same key.

### Consent

8. **Settings:** `settings.json` `privacy: {diagnostics?: "on"|"off", training?: "on"|"off", retentionDays?: 1..365, maxMiB?: 16..4096}`.
   - **Effective purpose** = `DO_NOT_TRACK=1` ? off : `FIN_<PURPOSE>` (`on|off`) ?? settings value ?? the build default. The env is `finisherd`'s own, read once at start. The agent sandbox's env allow-list (§8.4) does not pass `FIN_TRAINING` or `FIN_DIAGNOSTICS`, and an agent does not start the server, so these overrides belong to whoever runs `finisherd`.
   - An absent `training` is reported as **`undecided`**, the state before the onboarding question is answered. It records no content, exactly like `off`. Only `undecided` makes the onboarding screen (sub-project 9) ask; an explicit `off` is never asked again.
   - `basis` is `env`, `user` or `default`, after whichever decided. When the two purposes have different bases, the stamp carries the weaker one: `default` < `env` < `user`.
   - **An unusable `privacy` section** (bad JSON, wrong type, out of range) keeps the **last good** privacy settings, with a stderr warning, exactly as `readSettings(serverData, last)` already does for fonts. Only before the first good read in this process does it mean **both off** (fail closed). A bad read changes nothing else: no new effective consent, no `rev` bump and no `consent.changed`.
   - **The settings writer** is new: `apps/finisherd/src/settings-write.ts` `mergeSettings(serverData, patch)`. It reads the file as JSON, sets or removes only the patched keys under `privacy`, preserves every other key (unknown ones and `fonts` included), and writes temp-then-rename, 0600, fsynced. A file that does not parse is not overwritten: the call fails `SETTINGS_INVALID` with a `fix` naming the file, so a hand edit is never lost. Finisherd is the only program that writes `settings.json`; a hand edit racing a write is accepted, and the watch re-reads either result.
9. **Defaults live in one table,** `fin_journal::consent::DEFAULTS`.
   - It holds the owner's decisions (2026-11-07): **diagnostics on everywhere** (desktop, headless and the Docker image alike), **training undecided** (off until an explicit opt-in).
   - The training default is not configurable: `content_needs_explicit_consent` fails if `DEFAULTS` ever turns training on, so content always has `basis: user` or `env`.
   - An operator can still turn diagnostics off for a whole server with `FIN_DIAGNOSTICS=off` or `DO_NOT_TRACK=1`.
   - Changing the diagnostics default is a one-line change plus a test-golden update.
10. **Purposes.**
    - With `training` on, entries keep their content fields.
    - With only `diagnostics` on, content fields are removed by `fin_core::events::redact` before serialisation, and `KindSpec.training_only` kinds are written as their metadata part. An `agent.turn`, for example, keeps its token counts and latency and drops its messages.
    - With only `training` on, entries are written with both parts and `purposes: [training]`. Training covers diagnostics-class fields too: they are context for the examples.
    - **Diagnostics alone is already a training signal.** Its metadata says which ops an edit used, whether it was kept, undone, reverted, rebased or conflicted, and how soon. The privacy notice states that basic diagnostics may train models on this metadata. That is the "collected anyway" subset; the content tier adds what was edited and why.
11. **Off means off. There is no in-memory diagnostic ring.** The gate is an `AtomicU8` of purposes, read with `Relaxed` before any payload is built. Why:
    - it is simple, and provable by a test;
    - "off" then means the same to a user and to a reviewer;
    - a ring would still build every payload.

    Debugging without consent uses the normal stderr logs, which this plan does not change.
12. **Consent changes.**
    - **The journal opens with the resolved consent** (`Journal::open(dir, consent, opts)`). `finisherd` reads `settings.json` and the env first, so nothing is ever written under the build default when the user had said off.
    - The effective consent is persisted with a monotonic `rev` in `events/state.json` (written temp-then-rename, created lazily).
    - **A change goes on the control channel,** which never drops (Decision 18). `apply_settings` stores the new gate first, so callers stop building payloads for a withdrawn purpose at once. It then sends `ConsentChanged {from, to, basis}` on the control channel. The writer drains control messages before data, writes `consent.changed` stamped with the *old* consent when the old or new one allows it, and flushes. A withdrawal is therefore recorded just before it takes effect on disk.
    - **The writer redacts against the stamp ∩ the current gate.** Each entry carries the purposes it was recorded under. When it serialises, the writer intersects them with the gate's value at that moment. Training gone → content removed and no blob written; nothing left → the entry is dropped and counted. The stamp on disk is the intersection, so it never claims more than was in force. Content queued before a withdrawal therefore never reaches disk.
    - Turning a purpose off deletes nothing by itself. `fin diagnostics purge` does, and the upload view re-checks current consent per entry (Decision 31).
13. **The project flag is `finisher.project.json` `collect: "never"`.** Absent means "follow each user's settings"; `"default"` is written as absent.
    - **Why there, not the op log:**
      - it must be project-wide, not per branch;
      - it must not be undone by an undo or reintroduced by a merge;
      - it should not need a new primitive op and a schema bump;
      - the file already syncs with the folder, goes into bundles, is re-read by followers (`tick.rs` `reread_project_file`, from every import pass and from `takeover`) and is kept by older Finishers through `ProjectFile.other`.
    - **`crates/fin-store/src/collect.rs`** owns it:
      - `CollectPolicy {Default, Never}`; `Store::collect_policy()` returns the cached value, read at open;
      - `Store::observe_project_file(&ProjectFile)` sets `file_newer` (what `reread_project_file` does today) and the cached `collect`. When `collect` changed, it queues `StoreEvent::Collect(policy)` on the existing `follow.queued` list, as `queue_rebuilt` does;
      - a file that cannot be read changes nothing, as today;
      - `Store::set_collect(policy)` needs the writer lease (`PROJECT_READ_ONLY` otherwise), writes `collect` only for `never` (it removes the key for `default`), writes the file the usual way, updates the cache and queues the event.
    - **`tick.rs` changes by one line:** `reread_project_file`'s `self.file_newer = f.is_newer();` becomes `self.observe_project_file(&f);`. `lib.rs` gains one cached field.
    - **The event** is `EngineEvent::Collect {collect: "default"|"never"}`, JSON `{"type":"collect","collect":…}` (`wire.rs`, `status/mod.rs`, contracts). `fin-node`'s maintenance thread passes it to the journal (`set_project_policy`) before forwarding it. `ProjectEvent` in `@finisher/contracts` excludes it, like `applied` and `lagged`; `projects.ts` gets one `case "collect": break;` line.
    - **Unknown is not `never`.** A project not open on this server, or one open but not yet read (none in practice: open reads the file), is *unknown*. The journal drops new entries for an unknown project and purges nothing. Only an explicit `never` purges (Decision 29).
    - `fin-node` sets the policy at engine open and from each `collect` event, and sets it back to unknown at close. Every collaborator's server does the same when it reads the flag.
14. **Each entry carries the consent it was written under** (`consent.rev`, `purposes`, `basis`; Decision 12 for the intersection).
    - An entry with content never has `basis: default` (Decision 9). A diagnostics-only entry may.
    - A future uploader can still apply a stricter rule per region, such as dropping `basis: default` entries, without a schema change.
14a. **Setting consent.** `diagnostics.consent {purpose, value: on|off|unset}` (RPC) and `fin diagnostics consent <diagnostics|training> <value>`.
    - It writes `privacy` through the settings writer (Decision 8) and re-applies it. `unset` removes the key, so training goes back to `undecided`.
    - The reply is the new `ConsentState`. If an env override wins over the new value, the reply says so (`env`).
    - The onboarding screen of sub-project 9 is a client of this RPC. It shows one question for content with no answer pre-selected, and a notice about basic diagnostics with a link to turn it off.
14b. **Agents cannot opt users in.** `fin` runs in the agent sandbox, so nothing `fin` can do turns training on:
    - `fin diagnostics consent training` accepts only `off` and `unset`. `on` is a usage error (exit 2). Its message says that opting in is a person's choice, and its `fix` names the three ways in: the Finisher app (sub-project 9), editing `privacy.training` in `<server-data>/settings.json` by hand, or starting `finisherd` with `FIN_TRAINING=on`.
    - `diagnostics.consent {purpose: training, value: on}` is refused `CONSENT_UI_ONLY` (exit 2) unless the connection's source is `ui` (Decision 24). The local socket is `fin`, whatever program is on it.
    - `fin diagnostics consent diagnostics on|off|unset` stays allowed: turning basic diagnostics back on is the documented default, not an opt-in to content.
    - The `ui` source is the bearer-token WebSocket. The token is in `finisherd.json`, so any same-user process that can read server data could claim `ui`. Sub-project 7's sandbox keeps server data unreadable to agents; until then this is a Review Focus item.

### The journal

15. **One writer per server data: `finisherd`, through `fin-node`'s `Journal`.** `finisherd` already holds the server lock (`server_lock.rs`).
    - `fin` sends events over the local socket (`diagnostics.record`). The UI will do the same over the WebSocket.
    - **Offline `fin` records nothing.** That is accepted: agents run with a server.
    - Why not a writer per process: thousands of short `fin` runs would make thousands of files, or need cross-process locking.
16. **Files.**
    - `events/j-<yyyymmdd>-<ulid>.jsonl`; the date is the UTC date of the file's first entry.
    - A file rotates at 8 MiB or at the first entry of a new UTC day.
    - **Caps:** `retentionDays` (default 30) and `maxMiB` (default 128) over all `j-*` files **and** `blobs/`, oldest entries first (a blob goes when its last reference does; Decision 20), applied at open, at each rotation and hourly.
    - Other files: `events/key`, `events/state.json` and `events/blobs/<hex>`. Everything is 0600 and the directories are 0700.
17. **Crash safety.**
    - Every entry is one line ending in `\n`.
    - On open, the newest file's tail after its last `\n` is truncated. Readers skip lines that don't parse, and count them.
    - Closed files are fsynced, and so is the directory (a no-op on Windows, as in `fin-store::fsio`).
    - There is no per-line fsync: losing up to the last ~2 s (two flush intervals, Decision 18) on power loss is accepted.
    - A blob is written temp-then-rename and fsynced before the line that references it is written. A crash can leave an unreferenced blob, never a dangling reference; the GC removes the orphan (Decision 20).
    - `error.panic` and `session.end` are written and flushed synchronously, with a timeout (Decision 18).
18. **Never blocks.**
    - `record(source, ctx, AnyPayload)` serialises nothing in the caller. It stamps `id`, `ts`, `ctx` and the gate's purposes, adds `approx_bytes()` to an in-flight byte counter, and calls `try_send` on the data channel.
    - **The data channel is bounded twice:** 8,192 entries (`sync_channel`) and **8 MiB** of `approx_bytes` in flight (an `AtomicUsize`, decremented by the writer). Either limit drops the event and increments a counter. The writer reports drops as `journal.dropped {count, bytes, since}` at its next flush.
    - **The control channel** (`mpsc::channel`) carries consent changes, project policies, purge, export, flush, stats ticks and synchronous records. It is unbounded but carries only these, a few per minute; the writer drains it before each data batch. Nothing on it is dropped.
    - An I/O error suspends the journal (`status.state = suspended`, with the error code), discards data entries while suspended and retries every 60 s.
    - A panic in the writer thread is caught: the journal suspends and editing is unaffected.
    - **The writer parks until its timer; data does not wake it.** A send that wakes a sleeping receiver costs the sender a futex call and, on a VM, a cross-CPU interrupt (13 µs p50 on the Linux VM, measured in D5: it put diagnostics at +19–27 µs a commit). So the writer drains the data channel with `try_recv` and, with nothing queued, `park_timeout`s until the buffer's flush is due, or one flush interval (1 s, at most) with an empty buffer. A sender unparks it only when 64 KiB of `approx_bytes` are in flight or the channel is full. Every control message (consent, project policy, purge, export, flush, `record_sync`, close) unparks it at once. An idle journal costs no CPU between timer wakes.
    - It flushes when the flush interval has passed since the last flush, or at 64 KiB. **Worst-case latency to disk is ~2 s** (two flush intervals): an entry queued just after a wake waits up to one interval for the writer to take it, then up to one more for its buffer's flush. A synchronous record and an explicit flush (`status`, `export`, `purge`, `close`) are not delayed by the timer.
    - **`record_sync(source, ctx, payload, timeout)`** sends on the control channel and waits for the writer's acknowledgement for at most `timeout` (250 ms for `error.panic`, 1 s for `session.end`). A timeout returns and counts a drop.
    - **The writer-thread guard:** a thread-local marks the writer thread. A panic hook that finds itself on that thread does not call `record_sync` (it would wait on itself); the writer's own `catch_unwind` suspends the journal instead (`writer_panic_suspends_not_crashes`).
19. **High-rate timings are aggregated.**
    - `Journal::observe(metric, key: Name, micros, code)` adds to a per-minute window of log₂ buckets (1 µs to 2⁴⁰ µs) per `(metric, key)`, plus count, error count per code, and max. The window lives on the writer (it arrives over the data channel like an entry, a few bytes each).
    - Each minute writes one `rpc.stats` or `eval.stats` entry per key.
    - A window holds at most 256 keys; extra keys fold into `"other"`.
20. **Sizes and blobs.**
    - A serialised entry over 64 KiB is refused (`EVENT_TOO_LARGE`) and counted as dropped.
    - **Large content goes into blobs.** `BlobRef {hash, bytes, mediaType}`:
      - `hash` is the full 256-bit keyed BLAKE3 of the bytes (`^kb:[0-9a-f]{64}$`), the same key as Decision 6;
      - `bytes` is the length;
      - `mediaType` comes from `MEDIA_TYPES` (`image/webp`, `image/png`, `audio/opus`, `audio/wav`, `application/json`).

      A `BlobRef` may appear only inside a `Content` field, which the schema test checks. The bytes live in `events/blobs/<hex>`.
    - `Journal::attach_blob(bytes: Arc<[u8]>, media: Name) -> Option<BlobRef>` hashes in the caller and queues the bytes on the data channel, where they count against the 8 MiB bound. It returns `None` unless the gate has training. A blob is at most 4 MiB in this phase. **No emitter calls it yet;** a later sub-project adds frame and audio sources, and sub-project 7 the agent kinds' large content.
    - **The lifecycle follows the entries.** The writer keeps a reference count per blob. It is rebuilt at open by scanning the `j-*` files' blob references (`AnyPayload::blobs()`, or the content paths for lines on disk), raised when an entry that references the blob is written, and lowered when a file holding such an entry is deleted or rewritten (retention, `maxMiB`, purge of any selector, `never`). A blob whose count is zero is deleted at the next rotation or purge, so purge, retention, `never` and `maxMiB` all cover blobs. A blob whose referencing entry was dropped (redaction, a full channel) is never written.
21. **Cost budgets,** measured by `cargo run -p fin-journal --release --example journal_cost` and recorded in TASKS.md (Linux VM; 4C Task 29 repeats it on macOS and Windows):
    - disabled gate ≤ 5 ns per call;
    - enabled `record` p99 ≤ 5 µs in the caller, for a diagnostics-only `log.commit` and for one with training on (the committed intent is moved into the payload after the reply is encoded, so it is not cloned; Decision 22). A failed commit under training clones the request tx before the store call; that path's p99 is measured on a 100-op tx and recorded, not budgeted;
    - writer ≥ 100k diagnostics entries/s and ≥ 20 MiB/s of content entries sustained;
    - `record_sync` returns within its timeout with the writer stalled;
    - `fin-node` commit overhead with recording on, against off: p50 ≤ 2 % and ≤ 20 µs on a 1k-item project with diagnostics only; ≤ 5 % and ≤ 50 µs with training on;
    - zero entries and zero files when disabled.

    A budget miss blocks the batch.

### Sources

22. **`log.commit`** is one entry per attempt to append to the op log. It is classified and built in `crates/fin-node/src/commit_log.rs`, called **inside `with_store`**, from typed results, never by re-parsing the reply JSON:
    - **Unparsable requests are not logged.** A request that does not decode (`parse_tx`'s `INVALID_OP`, a history request's `INVALID_OP`, a bad generation, a non-integer count: what a malformed RPC payload gets, and the `INVALID_ARGS`-class refusals before the store is reached) never reaches the store and appends nothing, so it writes no `log.commit`: there is no tx or request to take the txId, author or ops from. It still shows in `rpc.stats` (its error code) and, from `fin`, in `cli.command`. Every request that decodes is logged, refusals included.
    - **Where:** `tasks.rs` `Call::Commit`; `history.rs` `run` for `BranchCreate`, `BranchSet`, `Undo`, `Redo`, `Checkpoint`, `Merge`, `CherryPick` and `Revert`; and `history.rs` `recover` (a long call on the worker) when the request makes or dismisses something. `Branches`, `ResolveBranch`, a listing-only `recover`, `takeover` and project create append no record and log nothing. The test-only commit in `test_hooks.rs` is not logged.
    - **Edit dry runs never reach `fin-node`:** `fin --dry-run` plans an edit on the client. So `dryRun` occurs only for the history, branch and recovery calls, whose requests carry `dryRun`. An edit dry run shows only as `cli.command.dryRun`.
    - **The store's dedupe signal.** `Store::write` (`fin-store/src/write.rs`) is the one place that knows whether a write ran, was a retry of a committed txId, or was rolled back as a plan. All eight call sites go through it: `commit_in`, undo/redo, checkpoint, merge, pick/revert, branch create, branch set and recover. They return five different result types, and only the commit and history results carry a `dryRun` flag, which a deduped retry also reports as `false`. So the signal is a note, not a new return type:
      ```rust
      pub enum WriteOutcome { Written, Deduped, Planned }
      impl Store { pub fn take_write_outcome(&mut self) -> Option<WriteOutcome>; }
      ```
      `write` sets the note at each return: `Ok(None)` → `Deduped`, `Ok(Some)` after rollback → `Planned`, after commit → `Written`. `commit_in`'s early dedupe (a committed txId whose retry fails to normalise) sets `Deduped` too. A retry loop that writes again overwrites it, so the last write wins. `fin-node` calls `take_write_outcome()` before the store call (to clear it) and after, under the same mutex guard, so no other call can interleave. `None` afterwards means no write was reached (a validation error, or a listing).
    - **Payload:**
      - `record`: `edit|undo|redo|merge|pick|revert|checkpoint|branch|branchSet|recover`;
      - `outcome`: `accepted|rebased|deduped|noOp|conflict|refused|dryRun`;
      - `code?`;
      - `ops`: op name → count (counted from the request before the call, for edits; from the committed records otherwise);
      - `baseRev`: the request's `baseRev` (edits only);
      - `rebasedOver`: committed `baseRev` − request `baseRev`, when rebased. There is no `baseLag`: the store plans against the head under the write lock and the stored intent's `baseRev` is that head (`fin-core` `rebase/engine.rs` sets `out.tx.base_rev = last.rev`; revs are contiguous per branch, `seq = head.rev + 1`), so the head lag at entry *is* `rebasedOver`;
      - `conflicts`: codes → count, from `CONFLICT`'s `details.conflicts`;
      - `links`: `IdRef`s, from the typed results: an undo's `HistoryResult.undone`, a merge's `MergeReport.merged`, a pick's or revert's `undone`/link target, and a recovery's committed txIds;
      - `micros`;
      - **training only:** `intent: Option<Content<TxIntent>>`. On success it is the intent as committed: `CommittedTx.tx` for an edit, each record's intent for history calls, at most 64 records. On failure it is the request's normalised tx. A `dryRun`, `deduped` or `noOp` entry has none (the plan was not kept; a retry's intent is on the first attempt's entry). An intent that would push the entry past 32 KiB is left out (`intentOmitted: tooLarge`); the builder then takes a committed intent from the op log by txId. **There is no snapshot:** the state at `baseRev` is rebuilt from the op log, whose compacted packs keep full history (§6).
    - **Outcome, in order:**
      - `Err(e)`: `conflict` for `CONFLICT`, else `refused`, with `code`;
      - note `Planned`: `dryRun`;
      - note `Deduped`: `deduped`;
      - nothing committed (`HistoryResult.noOp`, an empty `MergeReport.committed`): `noOp`, with `code` = the `noOp` reason when there is one;
      - an edit whose committed `tx.baseRev` ≠ the request's: `rebased`;
      - otherwise `accepted`.
    - **`ctx`:** project, branch, `txId` (`IdRef`; the request's, on every path; for a merge or a recovery that is its first record's, `<txId>-1`, what `fin` reports), author (keyed) and **actor**. The actor comes from the request's RPC headers (Decision 23a), passed by `finisherd` to the engine call as an optional `metaJson` argument. Without one it is `{kind: unknown}`. The UI will set it in sub-project 9.
    - **Cost:** nothing is built when `gate()` is `None`. The committed records are moved into the payload after the reply is encoded, so recording clones no tx on success.
    - Undo, redo and revert entries are the **rejection signal**: `links` names the original txIds.
23. **`cli.command`.**
    - **Payload:** `name` (`CLI_COMMANDS`: `fin`'s registry names, dotted, e.g. `item.trim`); `flags` (`CLI_FLAGS`: the flag names parsed, sorted, never values); `argsHash` (keyed, per Decision 6); `micros`; `exit`; `code?`; `online`; `dryRun`; `txIds`.
    - It is sent once, at the end, by `crates/fin/src/diag.rs`, as a **fire-and-forget** request on the connection `fin` already has: `fin` writes the request frame (at most 50 ms for the write) and never reads its reply. Its errors are ignored and never change the exit code or stdout. The server handler enqueues the event before it yields, so closing the connection right after cannot lose it.
    - **Who decides:** `fin` sends nothing when its own env has `DO_NOT_TRACK=1` or `FIN_DIAGNOSTICS=off`. Otherwise, when `fin` found the server through `finisherd.json`, it sends only if `diagnostics.enabled` is true. With `FIN_SERVER` set, `fin` never reads `finisherd.json` (`endpoint.rs`), so it sends and the server's gate decides: with recording off the handler builds nothing and returns.
    - `ctx.actor` is the actor of Decision 23a.
23a. **The actor travels with every request.**
    - `fin` builds `Actor` once per run: `{kind: agent, session: FIN_SESSION, call: FIN_CALL}` when `FIN_SESSION` is set, else `{kind: user}`. Each id is turned into an `IdRef` on the server.
    - **`FIN_CALL`** is reserved here: the tool-call id an agent harness sets for one `fin` run. It becomes `cli.command`'s `ctx.actor.call` and the `actor.call` of every `log.commit` the run causes, so a later `agent.toolCall {callId}` joins on it. Sub-project 7 adds it to the env allow-list (§8.4) and sets it.
    - It goes in the RPC request's headers (the NDJSON `Request` frame's `headers`, empty today in `client.rs`): `fin-actor-kind`, `fin-actor-session`, `fin-actor-call`. `finisherd` reads them in its handlers (`diagnostics.ts` `callMeta(headers)`), validates them (an unknown kind becomes `unknown`; an over-long id is dropped), and passes `metaJson` to the engine calls that write.
24. **Source and trust.**
    - **The handler layer is built per transport** with an injected `Source`: `main.ts` provides `RpcSource` = `fin` to `serveRpc(local)` and `ui` to `serveRpc(ws)`. `HandlersLive` reads `RpcSource`, and each transport gets `Layer.fresh(HandlersLive)`, because layers are memoized by reference (as `serveRpc` already does for the protocol). The shared services (`ProjectRegistry`, `Fonts`, `Diagnostics`) stay provided once. Today one `HandlersLive` serves both transports, so this is a change to `main.ts`, `handlers.ts` and `transport.ts`.
    - `diagnostics.record`: the server sets `source` and `session` from the transport and the connection, never from the payload. Kinds are allow-listed per source (`fin`: `cli.command`, `error.panic`; `ui`: none yet).
    - **Rate limit per source, across connections:** 50 events/s per source, burst 200, in one token bucket per `Source` in the `Diagnostics` service. A thousand short `fin` runs share one bucket. Extra events are dropped and counted.
    - The 64 KiB cap applies.

    Fake events from an injected agent can only appear under the `fin` source, with the actor it claims, which the dataset builder can weigh. This is a Review Focus item.
25. **Sessions.**
    - `session.start {app, protocol, os, osMajor, arch, cores, memGiB, profile}` when the journal opens with any purpose on, or when one turns on.
    - `session.end {uptimeS}` on clean shutdown.
    - `state.json.openSession` holds the run. A start that finds it still set writes `session.unclean {previous}`.
26. **Errors.**
    - `error.internal {call: Name, code, details: keys only}` for `INTERNAL` and the store's unexpected codes, as returned through `fin-node`.
    - `error.panic {where: finNode|finisherd|fin, location: Option<SrcLoc>, thirdParty: bool, messageHash: Hashed, thread: main|worker|tick|writer|other, message: Content<Scrubbed>}`:
      - **Under diagnostics, metadata only:** `location` is `file:line` limited to repo-relative paths, `SrcLoc` = `^(crates|apps)/[a-z0-9_./-]{1,160}:[0-9]{1,7}$`. A location outside our tree (a dependency in the cargo registry, `node_modules`, an absolute path) is `null` with `thirdParty: true`. Rust's `Location::file()` is already workspace-relative for our crates. For JS, the first stack frame under the app root is made relative, and anything else is `null`. `messageHash` is the keyed hash of the scrubbed message, so the same crash groups without its text. `thread` is a closed enum, not a `Name`: `main`, `worker`, `tick`, `writer` or `other`.
      - **The message itself is content** (`errorMessage`, training only), scrubbed first.
      - **No backtrace frames.** Rejected: a `Symbol` class limited to `fin_`-prefixed `::` paths. Release builds are stripped, so frames are unreliable exactly where crashes matter. Demangled generic symbols can still carry type names we do not control, and JS frames are file paths. `location` plus `messageHash` groups crashes well enough until minidumps arrive with the uploader, under the same consent.
      - **Sources:** a panic hook in `fin-node`, chained to the previous hook and guarded against the writer thread (Decision 18), that uses `record_sync`; `uncaughtException`/`unhandledRejection` in `finisherd`; and `fin`'s existing `output::install_panic_hook`, which calls `diag::report_panic(info)` (best effort, the 50 ms budget) before it prints the `INTERNAL` document and exits.
    - **Scrubbing** replaces the home dir, server data, temp dir, the user name, the host name, and any absolute path (`/…`, `C:\…`, `\\…`) with `<home>`, `<data>`, `<tmp>`, `<user>`, `<host>` and `<path>`.
27. **Other sources.**
    - `storage.state {state: readOnly|writable, reason: Code}` on read-only transitions (lease lost, conflict copy, newer format), from engine status events.
    - `fonts.fetch {entry: Hashed, bundled: bool, bytes, micros, outcome: ok|failed|cached, code?}` per fetch. The family is keyed-hashed: a pinned family may be a user's local font, and the catalog is not in `fin-core`'s tables.
    - `rpc.stats` with keys from `RPC_METHODS`. **Stream RPCs are excluded:** `ops.subscribe` and `project.events` (the only `stream: true` RPCs in `packages/contracts/src/rpc.ts` today; `rpc_stats_excludes_streams` checks the list against the contract). Their duration is a subscription's lifetime, not a call's. Timing wraps `guard` in `handlers.ts`, which the stream handlers do not use.
    - `eval.stats` with keys `eval.frame`, `eval.audio` and `fonts.plan`: the three calls in `fin-node/src/eval.rs`.
28. **Reserved kinds (version 0, no emitter):**
    - `agent.session {provider, model, role, mode}`;
    - `agent.turn {turnId, provider, model, inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens, latencyMs, stopReason, costMicros?; messages: Content<modelInput>, output: Content<modelOutput>, systemHash, toolsHash}`;
    - `agent.toolCall {callId: IdRef, turnId, command: Name, argsHash; args: Content<toolArgs>}`;
    - `agent.toolResult {callId: IdRef, ok, code?, micros, txIds; output: Content<toolResult>}`;
    - `agent.editVerdict {txIds, verdict: accepted|rejected|undone|merged|abandoned, by: user|critic|agent}`;
    - `ui.action` and `review.verdict` as empty v0 payloads.

    `agent.turn`, `agent.toolCall` and `agent.toolResult` are `training_only`. Content larger than an entry allows (Decision 20) is a `Content<BlobRef>` field; frames and audio are always blobs. The schema test covers them like the others.

### CLI, RPC, purge and export

29. **Purge** runs on the writer thread (a control message), so one owner touches the files. It rotates the active file, rewrites each affected closed file without the matching entries (temp-then-rename, fsync), deletes files left empty, lowers the blob counts of the removed entries, and deletes the blobs left at zero. Selectors:
    - `all`, which also deletes `key`, `state.json` and every blob;
    - `project <id>` (also run when a project's flag becomes `never`);
    - `before <date>`.

    Retention and `maxMiB` use the same path (whole files, oldest first), so blobs follow there too.
30. **`fin diagnostics`** (server only; `SERVER_UNAVAILABLE` offline):
    - `show` → `{consent {diagnostics, training (on|off|undecided), basis, rev}, env, state, files, bytes, blobs, blobBytes, kinds: {kind: count}, oldest, newest, dropped, projects: {id: never|default|unknown}}`;
    - `export [--since <date>] [--project <id>] [--kind <k>]… [--limit <n>] [--as-upload diagnostics|training]` → JSONL on stdout, newest last; `--limit` keeps the newest `n` (it replaces the dropped `show --recent`). Without `--as-upload` it prints what is stored, content included: it is the user's own data. With it, it prints exactly what an uploader would get now (Decision 31). Blob bytes are never printed; a `BlobRef` is;
    - `purge (--all | --project <id> | --before <date>)` → `{removed, files, blobs}`;
    - `project never|default [--project <id>]` → `{project, collect}`;
    - `consent diagnostics <on|off|unset>`, `consent training <off|unset>` → `ConsentState` (Decisions 14a, 14b).

    **Until sub-project 7 adds permissions, these are not human-only.** An agent's `fin` can read the user's recorded content with `export`, and can `purge --all` or `project default`. Content export is served only to the `fin` and `ui` sources, never to another source a later sub-project adds (`agent`, plugins). The TASKS follow-up makes `export` with content, `purge` (all selectors), `project default` and every consent change human-only. Opting in is already impossible from `fin` (Decision 14b).
31. **The upload view.** `Journal::export(filter, as_upload: Some(purposes))` is the contract a future uploader ships, and its tests are this plan's guarantee:
    - closed files only;
    - an entry is kept only if both its stamped purposes and the **current** consent include one of `purposes`, and its project is not `never` or unknown now;
    - content is removed unless training is both current and stamped;
    - blob references survive only with their content; the uploader fetches the bytes by `hash`.

    The `Uploader` trait, batches, the per-uploader cursor and at-least-once acknowledgement come with the uploader sub-project. They read through this view, so nothing here changes. With the dataset builder (Decision 34), the uploader ships the builder's examples plus the metadata entries of this view.
32. **RPCs** (`@finisher/contracts`, scope `diagnostics`):
    - `diagnostics.status`;
    - `diagnostics.export {filter, cursor?, limit ≤ 1000, asUpload?}` → `{lines, next?}`, paged; `cursor` is an opaque position for paging one export, not an upload cursor;
    - `diagnostics.purge {selector}`, `diagnostics.project {projectId, collect}`, `diagnostics.consent {purpose, value}` (Decisions 14a, 14b);
    - `diagnostics.record {kind, kv, ctx?, payload}` (Decision 24).

    There is no `diagnostics.recent`: `export` with `limit` covers it.
33. **New codes:**
    - exit 2: `CONSENT_UI_ONLY` (Decision 14b), with a `fix` naming the ways in;
    - exit 3: `EVENT_KIND_UNKNOWN`, `EVENT_INVALID`, `EVENT_TOO_LARGE`. These are seen only by `diagnostics.record` callers; `fin` ignores them;
    - exit 5: `JOURNAL_UNAVAILABLE`, for `fin diagnostics …` when the journal is suspended or the server data is unwritable, `SETTINGS_INVALID`, when `settings.json` does not parse (or its merged `privacy` section breaks the journal's rules) and a consent change would overwrite it, and `SETTINGS_WRITE_FAILED`, when the consent change could not be written (the file is unchanged; added by the D4 review). Each `fix` names the reason.

### Later

34. **The dataset builder** (out of scope; spec §21.1) **runs locally,** on the user's machine, over the journal plus the op log. A future uploader ships the examples it built, plus the metadata entries of the upload view. Raw journals and op logs never leave the machine.
    - **examples:** the state at `log.commit.baseRev`, rebuilt from the op log and rendered like `fin view`; the instruction from the `agent.turn` linked through `actor.call`, or the `cli.command` with the same txIds; the intent (`log.commit.intent`, or the op log's by txId);
    - **labels:** kept, undone or reverted (and how soon), merged, abandoned, conflict and its resolution, and verdicts;
    - frames and audio, later, from blobs;
    - only from `purposes ∋ training` entries.

    Nothing in this plan may make that join impossible: `log.commit` carries txIds, links, `baseRev`, the intent and the actor, and `cli.command` carries txIds and the same actor. The e2e checks the join (Task D7).

## Owner decisions (2026-11-07)

1. **Two tiers.**
   - **Basic diagnostics** is metadata only: codes, timings, counts, ids, keyed hashes, and the edit outcomes of Decision 10. It is on by default **everywhere**, with an opt-out. It is disclosed in the terms and the privacy notice, with a first-run notice in the app. It contains no content, paths or host details, so it can rest on legitimate interest (GDPR Art. 6(1)(f)) rather than consent.
   - **Training content** is captions, transcripts, prompts, full LLM conversations, edit intents, error messages, file names, media hashes, and later sampled frames and audio. It is recorded only after an explicit opt-in by a person: the onboarding question (sub-project 9), a hand edit of `privacy.training` in `settings.json`, or `FIN_TRAINING=on` in `finisherd`'s env on the owner's own machines. It is never on by default, and `fin` cannot turn it on (Decision 14b).
2. **Why content is opt-in although some editors default it on.**
   - Cursor's individual plans train on code unless Privacy Mode is on. GitHub Copilot's individual plans moved to training by default in April 2026. Both are criticised for it, and their business plans default off.
   - Zed keeps telemetry on but takes training data only by opt-in. VS Code sends usage telemetry by default with a notice.
   - Finisher follows Zed for content and VS Code for metadata. User footage, faces and voices make content riskier than code under GDPR Art. 9. A persuasive onboarding question ("help improve Finisher for everyone") gets most of the data with a clean basis.
3. **Still to check before the first upload** (a TASKS follow-up): a legal review of the notice wording, ePrivacy Art. 5(3) for storage on the device, and whether some regions need diagnostics consent too. The per-entry `basis` stamp lets the uploader apply a regional rule later without a rewrite.
4. **No network upload in this phase.**

## Review Focus

1. **Off means off, and nothing before settings:** no directory, no file, no payload built, no ring; the journal opens with the resolved consent.
   → D2 `disabled_journal_creates_nothing`; D3 `off_builds_no_payload` (a counting payload builder), `open_with_settings_off_writes_nothing`; D4 `settings_read_before_journal_opens`; D5 `commit_with_journal_off_touches_no_events_dir`.
2. **No content without training; never raw paths, hosts or users; no free names:**
   - the schema walk, `Name`'s constructors and `IdRef`;
   - redaction against the stamp ∩ the current gate, so queued content after a withdrawal never reaches disk;
   - scrubbing, and the panic payload's split (location and hash as meta, the message as content);
   - the e2e grep for temp dir, `$HOME`, `$USER` and the host name, over entries and blobs.

   → D1 `payload_strings_are_classified`, `name_has_no_free_constructor`, `id_ref_keeps_only_ulids_raw`, `src_loc_accepts_only_repo_paths`, `redact_removes_every_content_path`; D3 `diagnostics_only_writes_no_content`, `queued_content_after_withdrawal_never_hits_disk`; D7 leak grep.
3. **Agents cannot opt users in:** `fin … consent training on` is a usage error naming the ways in; the RPC refuses `training: on` from any source but `ui`; the `ui` token's exposure (Decision 14b); the not-yet-human-only list of Decision 30.
   → D4 `consent_rpc_refuses_training_on_from_fin_source`; D6 `consent_training_on_is_usage_naming_ways_in`; D7 step 5.
4. **The project flag:**
   - it is read from a synced file written by another server, and cached in `collect.rs`;
   - unknown drops new entries but purges nothing; only an explicit `never` purges, blobs included;
   - a follower honours it without the lease, through the `collect` event.

   → D3 `unknown_project_drops_but_does_not_purge`, `setting_never_purges_entries_and_blobs`, `follower_reads_collect_after_reread`; D5 `follower_honours_never_from_synced_file`.
5. **Commit outcomes are right:** classified from typed results and the store's write note, never from reply JSON; `rebased` = committed `baseRev` ≠ request `baseRev`; dedupe on every one of the eight write paths; `recover` covered; edit dry runs not expected.
   → D5 `commit_outcomes_*`, `deduped_retry_on_every_write_path`, `recover_create_is_logged`.
6. **Never blocks editing:**
   - full channel, by entries and by bytes;
   - unwritable `events/`;
   - disk-full injection;
   - a slow disk (an injected 2 s write stall leaves commit latency unchanged);
   - a panic in the writer, and a panic hook on the writer thread;
   - `record_sync` with a stalled writer.

   → D2, D5.
7. **Crash safety:** kill -9 mid-write leaves only whole lines after repair, and no dangling blob reference; the age and size caps hold across restarts. → D2 `crash.rs`, D3 `blob_written_before_its_reference`.
8. **The source and trust boundary:** `Source` injected per transport (local socket → `fin`, WebSocket → `ui`), the kind allow-list, the rate limit per source across connections, the size cap, actor headers validated. → D4.
9. **Purity and generated types:** `fin-core::events` builds for wasm32, reads no clock or env, and the TS types are generated. → D1, gates.
10. **Cost budgets** (Decision 21), rechecked for the byte bound, `AnyPayload` and the training-on commit path. → D2 example, D5 commit-overhead example, TASKS.md numbers.
11. **The join:** journal entries join the op log by txId, an undo names the undone tx, and `cli.command` and `log.commit` share the actor's session and call. → D7 e2e.

---

### Task D1: The event model in `fin-core` — tier: opus

**Files:**
- Create: `crates/fin-core/src/events/{mod.rs (Envelope, Source, Ctx, Actor, ActorKind, ConsentStamp, Purpose, Purposes, Basis, ENVELOPE_VERSION), kinds.rs (KindSpec, kinds(), kind(), AnyPayload, validate_payload), class.rs (Content, ContentKind, Scrubbed, ScrubCtx, Hashed, IdRef, Code, Name, SrcLoc, PathRef, PathRoot, BlobRef), names.rs (NameTable and the static tables of Decision 4), redact.rs (content_paths(kind), redact(value, kind, purposes), blob_refs(value, kind)), scrub.rs, payloads/{session.rs, log.rs, cli.rs, stats.rs, errors.rs, fonts.rs, storage.rs, consent.rs, journal.rs, agent.rs, reserved.rs}}`.
- Modify: `crates/fin-core/src/lib.rs` (`pub mod events`), `crates/fin-store/examples/export_schema.rs` (the envelope, `AnyPayload` and every payload type), `packages/core-types/schema/fin-core.schema.json`, `packages/core-types/src/generated.ts` (regenerated).
- Test: `crates/fin-core/tests/events.rs`, `crates/fin-core/tests/events_golden/*.json`.

**Interfaces:** Decisions 1–7, 20, 22–28 (the payload structs exactly as listed there).
- `pub fn kinds() -> &'static [KindSpec]`, `pub fn kind(name: &str, version: u16) -> Option<&'static KindSpec>`.
- `pub enum AnyPayload { … }` with `kind()`, `approx_bytes()`, `blobs()`; `pub fn validate_payload(spec: &KindSpec, payload: &Value) -> Result<AnyPayload, CoreError>` (`EVENT_INVALID`): a typed decode, with no schema engine.
- `pub fn redact(kind: &KindSpec, payload: &mut Value, purposes: Purposes)` removes content paths unless training is set. `content_paths` is computed from the kind's schema (`x-fin-class: content`), so a new content field needs no second list.
- `IdRef::classify(raw: &str) -> Result<IdRef, NeedsHash>`: a ULID is kept, anything else must be hashed by the caller, which has the key (`fin-core` has none).
- `Scrubbed::new(text, &ScrubCtx {home, server_data, tmp, user, host})` and `SrcLoc::parse(&str) -> Option<SrcLoc>`, pure.

- [ ] **Step 1: Write failing tests:**
  - `envelope_json_matches_golden` (camelCase, omitted `ctx`, an actor with session and call);
  - `every_kind_round_trips` and `any_payload_covers_every_kind` (one variant per registry entry, reserved included);
  - `approx_bytes_bounds_serialised_size` (within 2× either way on the samples);
  - `payload_strings_are_classified` (the schema walk, Decision 4), `content_fields_name_their_kind`, `blob_ref_only_inside_content`;
  - `name_has_no_free_constructor` (a source scan of `events/` for `From<String>`, `FromStr`, `Default`, `Deserialize` impls on `Name`, plus a compile-fail doctest), `name_tables_match_pattern`, `unknown_table_value_reads_as_other`;
  - `id_ref_keeps_only_ulids_raw` (an idempotency key, a session name with a user name in it, a lower-case ULID);
  - `src_loc_accepts_only_repo_paths` (`crates/…`, `apps/…` pass; a cargo-registry path, `node_modules`, an absolute or `..` path fail);
  - `log_commit_intent_is_content`, `panic_message_is_content_location_is_meta`;
  - `redact_removes_every_content_path`, `redact_keeps_content_with_training`, `training_only_kinds_keep_metadata_under_diagnostics`;
  - `unknown_kind_and_version_refused`;
  - `oversized_name_or_code_rejected_by_pattern`;
  - `scrub_replaces_home_data_tmp_user_host_and_absolute_paths` (POSIX, `C:\` and UNC cases);
  - `path_ref_never_keeps_the_path` (on the type: no field holds the path; `ext` only from `MEDIA_EXTS`);
  - `events_module_is_pure` (a source grep for `std::time`, `std::env`, `std::fs` and `getrandom` in `events/`).
- [ ] **Step 2: Implement.** Generate the TS types and check that `generated.ts` exports `Envelope` and a payload union keyed by `kind`.
- [ ] **Step 3: Run** the three gates (the wasm32 build of `fin-core` included) → PASS.
- [ ] **Step 4: Commit** `feat(core): versioned event envelope, kind registry and field classes`.

### Task D2: `fin-journal` — writer, files, crash safety, stats, cost — tier: opus

**Files:**
- Create: `crates/fin-journal/{Cargo.toml, src/lib.rs, src/writer.rs, src/control.rs, src/files.rs, src/read.rs, src/stats.rs, src/clock.rs, examples/journal_cost.rs}`. Add the crate to the workspace `default-members`.
- Test: `crates/fin-journal/tests/{journal.rs, crash.rs, caps.rs, stats.rs, bounds.rs}`, plus a `crash_child` helper binary under `tests/support/`.

**Interfaces:**
```rust
pub struct Journal { /* Arc<Inner> */ }
pub struct Consent { pub purposes: Purposes, pub rev: u64, pub basis: Basis }   // D3's resolve() makes it
pub struct JournalOptions { pub max_file_bytes: u64, pub max_total_bytes: u64, pub max_age: Duration,
    pub channel: usize, pub channel_bytes: usize, pub flush_every: Duration, pub flush_bytes: usize,
    pub stats_every: Duration, pub clock: Arc<dyn Clock> }
impl Journal {
    pub fn open(dir: &Path, consent: Consent, opts: JournalOptions) -> Journal;  // never fails; unusable dir → suspended
    pub fn disabled() -> Journal;
    #[inline] pub fn gate(&self) -> Option<Purposes>;             // one Relaxed load
    pub fn record(&self, source: Source, ctx: Option<Ctx>, p: AnyPayload);
    pub fn record_value(&self, source: Source, session: &str, kind: &str, kv: u16,
                        ctx: Option<Ctx>, payload: Value) -> Result<(), CoreError>;   // validate_payload, then record
    pub fn observe(&self, metric: Metric, key: Name, micros: u64, code: Option<Code>);
    pub fn record_sync(&self, source: Source, ctx: Option<Ctx>, p: AnyPayload, timeout: Duration) -> bool;
    pub fn on_writer_thread() -> bool;                            // the panic-hook guard
    pub fn flush(&self) -> Result<(), JournalError>;
    pub fn status(&self) -> JournalStatus;   // {state: off|on|suspended, code?, files, bytes, dropped, oldest?, newest?}
    pub fn read(&self, filter: &Filter) -> impl Iterator<Item = Envelope>;   // flushes first; skips and counts torn lines
    pub fn close(self);
}
```
Decisions 15–19, 21. The directory is created only by the first write. In this task the stamp is the consent passed to `open`. D3 adds `apply_settings`, the stamp ∩ gate rule, projects and blobs.

- [ ] **Step 1: Write failing tests:**
  - `disabled_journal_creates_nothing`;
  - `record_then_read_round_trips`;
  - `rotates_at_size_and_at_utc_day` (injected clock);
  - `age_and_size_caps_delete_oldest` (also `caps_hold_across_reopen`);
  - `torn_tail_is_truncated_on_open`;
  - `reader_skips_bad_lines_and_counts`;
  - `full_channel_drops_and_reports_journal_dropped`, `byte_bound_drops_large_entries_first_and_counts_bytes`;
  - `control_messages_are_never_dropped_when_data_is_full`;
  - `unwritable_dir_suspends_and_retries` (0500 dir on unix; a file in place of the dir on every OS);
  - `writer_panic_suspends_not_crashes`, `panic_hook_on_writer_thread_does_not_wait`;
  - `slow_disk_never_blocks_record` (an injected 2 s stall; `record` p99 stays under the budget);
  - `record_sync_is_on_disk_when_it_returns`, `record_sync_times_out_with_a_stalled_writer`;
  - `stats_window_writes_one_entry_per_key_per_minute`, `stats_overflow_folds_into_other`;
  - `files_are_0600_dir_0700` (unix);
  - `no_network_crates_in_tree` (`cargo tree -p fin-journal` contains no `ureq`, `rustls`, `fin-fonts` or `erased-serde`);
  - `crash.rs::kill_mid_write_leaves_whole_lines` (10 runs, the child killed at random points).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Measure** with `rtk cargo run -p fin-journal --release --example journal_cost`: disabled gate, `record` p50/p99 (diagnostics-only and training `log.commit` samples), writer throughput in entries/s and MiB/s, `record_sync` with a stalled writer, bytes per entry. Check them against Decision 21 and record them in TASKS.md.
- [ ] **Step 4: Run** the gates → PASS. **Commit** `feat(journal): local append-only event journal with caps, crash repair and stats`.

### Task D3: Consent, the project flag, redaction at write, blobs, purge and export — tier: opus

**Files:**
- Create:
  - `crates/fin-journal/src/{consent.rs (PrivacySettings, EnvOverrides, DEFAULTS, resolve, last-good handling, rev in state.json), key.rs (install key, keyed hash, IdRef hashing), projects.rs (policy map: default, never, unknown), blobs.rs (attach, write-before-reference, ref counts, GC), purge.rs, export.rs (Filter, the upload view)}`;
  - `crates/fin-store/src/collect.rs` (`CollectPolicy {Default, Never}`, `Store::collect_policy()`, `Store::observe_project_file()`, `Store::set_collect(policy)`).
- Modify:
  - `crates/fin-journal/src/{lib.rs, writer.rs, control.rs}`: the gate from resolved consent, the stamp ∩ gate rule, redaction before serialising, the project check, blob counts on rotation and delete;
  - `crates/fin-store/src/{project_file.rs (the `collect` field: a few lines, 512 today), lib.rs (one cached field and the re-export), tick.rs (exactly one line, Decision 13), status/mod.rs (`StoreEvent::Collect`), wire.rs (`EngineEvent::Collect`)}`;
  - `crates/fin-node/src/maintenance.rs` (doc list: the `collect` event);
  - `packages/contracts/src/rpc.ts` (the `collect` member of `EngineEvent`, excluded from `ProjectEvent`), `apps/finisherd/src/projects.ts` (one `case "collect": break;`);
  - `crates/fin-store/examples/export_schema.rs` (`PrivacySettings`, `ConsentState`, `JournalStatus`, `CollectPolicy`, `Filter`, `EngineEvent`), generated types.
- Test: `crates/fin-journal/tests/{consent.rs, privacy.rs, purge.rs, blobs.rs, export.rs}`, `crates/fin-store/tests/collect.rs`.

**Interfaces:**
```rust
pub fn resolve(privacy: Option<&Value>, env: &EnvOverrides, last_good: Option<&PrivacySettings>)
    -> (Consent, Option<PrivacySettings>, Option<String>);          // consent, new last good, warning
impl Journal {
    pub fn apply_settings(&self, privacy: Option<&Value>);          // the raw `privacy` section; env read once at open
    pub fn consent(&self) -> ConsentState;                          // {diagnostics, training, basis, rev, env}
    pub fn set_project_policy(&self, project: &str, policy: Option<CollectPolicy>); // None = unknown
    pub fn hash(&self, raw: &str) -> Option<Hashed>;                // None while off (no key is made)
    pub fn id_ref(&self, raw: &str) -> Option<IdRef>;               // Decision 7
    pub fn attach_blob(&self, bytes: Arc<[u8]>, media: Name) -> Option<BlobRef>;   // None unless training
    pub fn purge(&self, sel: PurgeSelector) -> Result<PurgeReport, JournalError>;
    pub fn export(&self, filter: &Filter, as_upload: Option<Purposes>) -> impl Iterator<Item = Envelope>;
}
```
Decisions 6–14, 20, 29, 31.

- [ ] **Step 1: Write failing tests:**
  - `resolve_precedence` (a table test: DO_NOT_TRACK > env > settings > default; off wins; basis);
  - `content_needs_explicit_consent` (`DEFAULTS.training` is never on), `undecided_records_no_content`, `content_entries_never_basis_default`;
  - `open_with_settings_off_writes_nothing`;
  - `unusable_privacy_before_first_good_read_means_off`, `unusable_privacy_after_good_read_keeps_last_good`, `bad_read_does_not_bump_rev`;
  - `consent_rev_is_monotonic_across_reopen`;
  - `withdrawal_is_recorded_then_takes_effect`, `queued_content_after_withdrawal_never_hits_disk` (training on, fill the channel with content entries, withdraw, then let the writer run: no content on disk, stamps are `[diagnostics]`), `consent_change_is_not_dropped_with_full_channel`;
  - `off_builds_no_payload`;
  - `diagnostics_only_writes_no_content`, `training_writes_content`, `stamp_is_recorded_purposes_intersect_gate`;
  - `never_project_entries_dropped`, `unknown_project_drops_but_does_not_purge`, `setting_never_purges_entries_and_blobs`;
  - `blob_written_before_its_reference`, `blob_not_written_without_training`, `blob_gc_on_purge_project_and_before`, `blob_gc_on_retention_and_max_mib`, `shared_blob_survives_until_last_reference`, `orphan_blob_from_crash_is_collected`, `max_mib_counts_blobs`;
  - `purge_all_removes_key_state_and_blobs`, `purge_before_date`, `purge_project_rewrites_files_atomically` (kill during purge leaves the old or the new file, never a mix);
  - `keyed_hash_is_stable_and_not_plain_blake3`, `no_key_created_while_off`, `non_ulid_ids_are_hashed`;
  - `export_as_upload_applies_current_consent` (written with training on, then training off → no content and no blob references in the upload view), `export_as_upload_reads_closed_files_only`, `export_as_upload_skips_never_and_unknown_projects_now`, `export_limit_keeps_newest`;
  - fin-store: `set_collect_needs_lease`, `collect_round_trips_and_older_file_keeps_it` (`other` is preserved), `follower_reads_collect_after_reread` (and the `collect` event is queued once per change), `unreadable_project_file_keeps_cached_collect`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(journal,store): consent per purpose, project never-collect flag, blobs, purge and the upload view`.

### Task D4: `fin-node` and `finisherd` wiring; sources per transport; `diagnostics.*` RPCs — tier: sonnet

**Files:**
- Create:
  - `crates/fin-node/src/journal.rs`: the napi `Journal`, with `open(serverData, privacyJson)` (resolve, then open), `applySettings(json)`, `record(kind, kv, ctxJson, payloadJson, source, session)`, `observe(metric, key, micros, code)`, `status()`, `export(filterJson, cursor, limit, asUpload)`, `purge(selectorJson)`, `setProjectPolicy`, `consent()` and `close()`; plus the chained, writer-guarded panic hook;
  - `apps/finisherd/src/diagnostics.ts`: the `Diagnostics` service and Layer (open after reading settings, the settings watch, sessions, uncaught handlers, RPC timing, the handlers, the `diagnostics.record` guard, rate limits per source, `callMeta(headers)`);
  - `apps/finisherd/src/settings-write.ts` (Decision 8).
- Modify:
  - `crates/fin-node/src/{lib.rs, Cargo.toml}` (adds `fin-journal`);
  - `apps/finisherd/src/config.ts` (`Settings.privacy` validation; `ServerInfo.diagnostics`);
  - `apps/finisherd/src/main.ts` (an `RpcSource` per transport; `Layer.fresh(HandlersLive)` per transport; the `Diagnostics` layer before the transports);
  - `apps/finisherd/src/transport.ts` (`serveRpc(server, source)`);
  - `apps/finisherd/src/handlers.ts` (reads `RpcSource`; `guard(tag, effect)` times non-stream calls; the `diagnostics.*` handlers);
  - `packages/contracts/src/rpc.ts` (Decision 32), `crates/fin/tests/rpc_contract.rs`, `crates/fin-store/examples/export_schema.rs`, generated types.
- Test: `packages/fin-node/test/journal.test.ts`, `apps/finisherd/test/{diagnostics.test.ts, settings-write.test.ts}`.

**Interfaces:** Decisions 8, 12, 14a, 14b, 15, 24–27, 32.
- `finisherd.json` gains `diagnostics: {enabled: boolean}`, rewritten (temp-then-rename) when the gate opens or closes.
- The `privacy` settings are re-read on a `watchFile` poll of their own, beside the one in `fonts.ts`, keeping the last good settings (`readSettings(serverData, last)`'s rule).
- Check during implementation how Effect 4's `RpcServer` hands request headers to a handler; `callMeta` takes whatever it is given. D5 passes the result to the engine calls.

- [ ] **Step 1: Write failing tests:**
  - `settings_read_before_journal_opens` (settings say off → no `events/` even for `session.start`);
  - `settings_privacy_applies_live`, `bad_privacy_section_keeps_last_good_and_warns`, `bad_privacy_before_first_good_read_is_off`;
  - `settings_writer_merges_and_preserves_unknown_keys`, `settings_writer_refuses_unparsable_file`, `settings_writer_is_temp_then_rename`;
  - `env_overrides_settings`, `do_not_track_forces_off`;
  - `server_info_reflects_gate`;
  - `session_start_end_and_unclean`;
  - `uncaught_exception_is_error_panic_meta_only_under_diagnostics` (location repo-relative or null, hash, no message), `uncaught_message_is_content_with_training`;
  - `rpc_timing_reaches_rpc_stats` (an injected clock or a 1 s window under test hooks), `rpc_stats_excludes_streams`, `rpc_methods_table_matches_capabilities`;
  - `source_is_fin_on_socket_and_ui_on_websocket`, `record_rpc_refuses_unlisted_kind`, `record_rate_limited_per_source_across_connections` (two connections share one bucket), `record_rpc_size_cap`;
  - `consent_rpc_writes_settings_and_reports_env_override`, `consent_rpc_refuses_training_on_from_fin_source` (`CONSENT_UI_ONLY`), `consent_rpc_allows_training_on_from_ui`;
  - `diagnostics_status_export_purge_project_rpcs`, `export_pages_with_cursor_and_limit`;
  - `call_meta_from_headers_validates_actor`;
  - `fin_node_panic_hook_chains_previous_and_skips_writer_thread`;
  - RPC contract fixtures.
- [ ] **Step 2: Implement.** **Run** the three gates and `rtk pnpm e2e` → PASS.
- [ ] **Step 3: Commit** `feat(node,server): journal in finisherd, privacy settings, sources per transport and diagnostics RPCs`.

### Task D5: Commit outcomes and edit-path sources in `fin-node` — tier: opus

**Files:**
- Create: `crates/fin-node/src/commit_log.rs` (`classify` from the typed result, the request and the write note; `build` the `log.commit` payload; the actor from `metaJson`).
- Modify:
  - `crates/fin-store/src/{write.rs (WriteOutcome and the note set at each return of `write`; 292 lines today), commit.rs (the early-dedupe path sets `Deduped`), lib.rs (re-export, `take_write_outcome`)}`;
  - `crates/fin-node/src/tasks.rs` (`Call::Commit` gains `meta_json`; inside `with_store`: count ops, clone the request under training, clear the note, commit, read the note, encode the reply, then classify and move the committed tx into the payload);
  - `crates/fin-node/src/history.rs` (the same around each writing call in `run`, and in `recover`; links from `HistoryResult`, `MergeReport` and `RecoverReport`; every writing napi method takes an optional `metaJson`);
  - `crates/fin-node/src/{engine.rs (journal handle at open, project policy at open and unknown at close; `storage.state` from status events), maintenance.rs (the `collect` event to `set_project_policy`), eval.rs (observe `eval.frame`, `eval.audio`, `fonts.plan`), fonts/ (`fonts.fetch`)}`;
  - `apps/finisherd/src/{projects.ts, history.ts}`: pass `metaJson` from `callMeta` through to the engine calls (call-site lines only in `projects.ts`).
- Test: `crates/fin-store/tests/write_outcome.rs`; `packages/fin-node/test/journal-sources.test.ts`; `crates/fin-node/examples/commit_overhead.rs` (runs under test hooks; the budget of Decision 21).

**Interfaces:** Decisions 7, 13, 21, 22, 23a, 26, 27.
- The store's write note is the only store API change in this task (`WriteOutcome`, `Store::take_write_outcome`). The public result types are unchanged.
- `links` come from the typed results: an undo's undone units, a merge's copied txIds, a pick's or revert's target, a recovery's records.

- [ ] **Step 1: Write failing tests:**
  - fin-store: `write_outcome_written_deduped_planned` (each of the eight call sites), `normalise_failure_retry_is_deduped`, `retry_loop_reports_last_write`;
  - `commit_outcomes_accepted_rebased_deduped_conflict_refused` (one each, with codes, `baseRev` and `rebasedOver`; `rebased` exactly when the committed `baseRev` differs from the request's);
  - `history_dry_runs_are_dry_run`, `pick_and_revert_no_op_is_no_op`, `empty_merge_is_no_op`;
  - `deduped_retry_on_every_write_path` (commit, undo, redo, checkpoint, merge, pick, revert, branch create, branch set, recover create);
  - `undo_links_the_undone_tx`, `redo_and_revert_link`, `merge_links_copied_txs`, `pick_links_source`;
  - `checkpoint_and_branch_records`, `recover_create_is_logged`, `recover_listing_is_not_logged`;
  - `intent_is_content_and_committed`, `failed_commit_intent_is_the_request`, `oversized_intent_is_omitted`;
  - `actor_from_meta_and_unknown_without`;
  - `ctx_author_is_hashed`, `non_ulid_txid_is_hashed`;
  - `internal_error_is_error_internal`;
  - `read_only_transition_is_storage_state`;
  - `font_fetch_events`, `eval_stats_keys`;
  - `commit_with_journal_off_touches_no_events_dir`;
  - `follower_honours_never_from_synced_file` (two engines on one folder, the second as follower, through the `collect` event);
  - `journal_stall_does_not_slow_commits`.
- [ ] **Step 2: Implement.** **Measure** with `rtk cargo run -p fin-node --release --example commit_overhead --features test-hooks`: off, diagnostics on, training on, and on with a stalled journal. Check the budgets and record them in TASKS.md.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(node,store): log.commit from typed results with a write-outcome note; history, eval, font and storage events`.

### Task D6: `fin` — actor headers, `cli.command` and `fin diagnostics` — tier: sonnet

**Files:**
- Create: `crates/fin/src/diag.rs` (the env opt-out; `diagnostics.enabled` from the discovery file already loaded, or "send" under `FIN_SERVER`; the `Actor` from `FIN_SESSION`/`FIN_CALL`; build and fire-and-forget `cli.command` within 50 ms; `report_panic`), `crates/fin/src/commands/diagnostics.rs`.
- Modify: `crates/fin/src/{main.rs (one call after dispatch, before exit), client.rs (the actor headers in `request`; a send-only request; call-site lines), output.rs (`install_panic_hook` calls `diag::report_panic` first; Decision 33 codes), registry.rs, map.rs}`, `crates/fin-core/src/events/names.rs` (only if the drift test finds a missing name).
- Test: `crates/fin/tests/cli_diagnostics.rs`.

**Interfaces:** Decisions 14b, 23, 23a, 30. The flag names come from the registry's parse. Values, positional arguments and stdin are never sent.

- [ ] **Step 1: Write failing tests:**
  - `cli_command_sent_when_enabled`, `nothing_sent_when_disabled` (a counting server);
  - `nothing_sent_with_do_not_track_or_fin_diagnostics_off`;
  - `fin_server_sends_and_server_gates` (`FIN_SERVER` set, server recording off: nothing stored);
  - `cli_command_has_no_values` (a run with a unique marker in every argument, then a grep of the journal);
  - `args_hash_is_keyed_on_disk`;
  - `slow_server_never_delays_exit_beyond_budget`;
  - `record_failure_never_changes_exit_or_stdout`;
  - `actor_headers_on_every_request`, `agent_actor_from_fin_session_and_fin_call`;
  - `registry_names_are_in_core_tables`;
  - `panic_reports_then_prints_internal` (the chained hook: one `error.panic`, then the usual exit-6 document);
  - `diagnostics_show`, `diagnostics_export_filters_limit_and_as_upload`, `diagnostics_purge_selectors`, `diagnostics_purge_needs_a_selector_is_usage`, `diagnostics_project_never_then_default`;
  - `diagnostics_consent_diagnostics_on_off_unset`, `diagnostics_consent_training_off_unset` (`show` says `undecided` after `unset`), `consent_training_on_is_usage_naming_ways_in`;
  - `diagnostics_offline_is_server_unavailable`, `journal_unavailable_exit_5`;
  - `map_still_under_budget`.
- [ ] **Step 2: Implement.** **Run** `rtk cargo test -p fin` (`FIN_REQUIRE_SERVER=1`), the gates and `rtk pnpm e2e` → PASS.
- [ ] **Step 3: Commit** `feat(fin): actor headers, record command invocations; fin diagnostics show, consent, export, purge and project`.

### Task D7: e2e, docs and tracker — tier: sonnet

**Files:**
- Create: `scripts/e2e/diagnostics.mjs`.
- Modify: `scripts/e2e.mjs` (`SCENARIOS.diagnostics`), `scripts/e2e.test.mjs`, `README.md` (a "Privacy and diagnostics" section: what is recorded, where, the switches, how to opt in, `fin diagnostics`), `docs/specs/2026-11-01-finisher-architecture-design.md` (§21.1 "as built" notes and any decision that changed), `TASKS.md`.

- [ ] **Step 1: Write the scenario,** driven by `fin` only, with `FIN_SESSION` and `FIN_CALL` set:
  1. Start with `FIN_DIAGNOSTICS=off FIN_TRAINING=off`. Make a project, run 3 edits and an undo. Check: `events/` does not exist, and `fin diagnostics show` says off.
  1a. Restart with no env and no `privacy` settings. Check: `show` says diagnostics `on` (`basis: default`) and training `undecided`, and an edit writes no content.
  2. Restart with diagnostics on. Run 3 edits, an undo, a stale edit that rebases, a retried txId, and a conflicting edit. Then check:
     - `export` holds `session.start`, `log.commit` entries with the right outcomes (`accepted`, `rebased` with `rebasedOver`, `deduped`, `conflict`), and `cli.command` entries whose txIds equal the committed ones;
     - the undo's `links` names the undone tx;
     - `log.commit` and `cli.command` carry the same `actor.session` and `actor.call`;
     - no content fields exist;
     - **join check:** every `log.commit` txId resolves through the `tx.get` RPC (the e2e RPC helper in `scripts/e2e/lib.mjs`).
  3. **Leak grep** over `events/`, blobs included: no temp-dir path, `$HOME`, `$USER`, host name or argument marker.
  4. `fin diagnostics project never`, then one edit. Check: no entries for that project, and the earlier ones are purged. A second server following the folder also writes none for it.
  5. `fin diagnostics consent training on` exits 2 with a usage error naming the ways in. Restart `finisherd` with `FIN_TRAINING=on`, run one edit, and check that its `log.commit` has `intent`. Restart without it and run `fin diagnostics consent training off`. Check:
     - `consent.changed` entries;
     - `export --as-upload training` is empty after the withdrawal;
     - `purge --all` leaves no `key` and no `blobs/`.
- [ ] **Step 2: Docs.**
  - The README section.
  - In the spec, the §21.1 as-built notes: measured costs, any deviation, and the owner decisions above.
  - TASKS follow-ups:
    - the uploader (with its trait, batches and cursor over the upload view) and its notice;
    - the local dataset builder, whose examples the uploader ships;
    - agent kinds, `FIN_CALL` in the env allow-list, and large agent content as blobs (7); frame and audio sources (a later sub-project);
    - UI kinds, the UI's actor on commits, and the consent screens (9);
    - review verdicts (10);
    - human-only `export` with content, `purge` (every selector, `--all` included), `project default` and consent changes (7);
    - keeping server data, and so the `ui` token, unreadable to agents (7);
    - the onboarding consent question and the diagnostics notice (9);
    - GPU facts (6);
    - a legal review before the first upload.
- [ ] **Step 3: Run** `rtk pnpm e2e`, the gates → PASS. **Commit** `test(e2e): diagnostics journal scenario; docs: privacy and diagnostics as built`.
