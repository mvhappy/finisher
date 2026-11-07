# Sub-project 8A: Packages, registries and lifecycle — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (the TUF client and its attack matrix, the strict package reader, manifest validation and permission diffs, plans and approvals, trust decisions, validation), `sonnet` for plumbing (gix transport, publisher CLI, control-store tables, RPCs, console, `fin`, content consumers, e2e, docs).

**Goal:** Finisher can verify, install, update, enable, disable and remove plugins from bundled, local-folder and git registries, with every package checked through TUF metadata and every permission approved by a person. Nothing a plugin ships runs in 8A: content packs become active (font catalogs and caption styles have consumers), code plugins install but cannot be enabled until 8B. After this plan:
- `fin-plugin-api` (pure) defines the manifest, permissions, contributions and licence classes, validates them in Rust, and generates their TypeScript types;
- `.finpkg` packages are read by a strict reader into a staging directory, and packed reproducibly by `fin-plugin pack`;
- `fin-registry` runs the TUF 1.0 client workflow (root chain, timestamp, snapshot, delegated targets) over local-folder and git registries, keeps trusted metadata per registry, refuses rollback, freeze (per freshness policy), mix-and-match, delegation escapes and tampered targets, and reads signed yanks and advisories;
- `fin-plugin registry …` builds and signs registries for teams and tests;
- `finisherd` keeps registries, installed plugins and grants in `control.db`, runs **plan → approve → install**, installs the bundled registry's plugins at start, audits every change and journals `plugin.lifecycle` and `registry.refresh`;
- `finisherd plugin|registry …` (console, owner) and `fin plugin search|info|ls|validate|suggest|install` (agents; install is `HUMAN_ONLY` unless the owner allowed it for a registry).

**Architecture:**
- **`crates/fin-plugin-api`** (new, pure, wasm-safe): `manifest/` (types, ids, versions, runtimes, contributions), `permission.rs` (closed permissions, risk, diff), `licence.rs` (SPDX classification), `validate/` (one module per rule family), `schema.rs` (export). Exported through `crates/fin-store/examples/export_schema.rs` like every model type.
- **`crates/fin-registry`** (new, I/O): `package/` (strict reader, packer, staging), `tuf/` (the trait over the chosen crate, trusted store, freshness, clock check), `transport/` (`fs`, `git_tree`), `registry.rs` (config, refresh, search over signed custom metadata), `advisory.rs`, `store.rs` (store and staging layout, install and remove), `plan.rs` (install plans), `testing.rs` (behind `test-support`: repository builder with throwaway keys).
- **`crates/fin-plugin-cli`** (new binary `fin-plugin`, not shipped): `new`, `validate`, `pack`, `registry init|add|publish|yank|advise|delegate|rotate-key|resign-timestamp`.
- **`fin-node`** gains `registry.rs`: a napi `Registry` class whose long calls (refresh, plan, install, remove) run on the per-engine worker thread (core §3.10 Decision 7), and `validateManifest`; the font manager accepts extra catalogs.
- **`fin-core::events`** gains `plugin.lifecycle` and `registry.refresh` (v1), the `PluginRef` payload type and the `BUNDLED_PLUGINS` and `PLUGIN_ACTIONS` name tables.
- **`finisherd`** gains `src/plugins/` (`registries.ts`, `lifecycle.ts`, `plans.ts`, `content.ts`, `bundled.ts`, `rpc.ts`), migration `control/migrations/m00NN-plugins.ts`, scope rows, console `plugin` and `registry` commands.
- **`fin`** gains `commands/plugin.rs` and `--from <@pub/pack>:<style>` on `style add`.

**Tech Stack:**
- Rust 2024; `tough` 0.24 (MIT OR Apache-2.0; `http` feature off) or, if Task 1's gate fails, `tuf` 0.3.0-beta15 (MIT OR Apache-2.0, `ring`); `tokio` (current-thread runtime, inside `fin-registry` only, if `tough` is kept); `gix` (MIT OR Apache-2.0; minimal features: `blocking-network-client`, an HTTPS transport on rustls, `max-performance-safe` off); `zip` 9 (in the tree); `jsonschema` (MIT); `spdx` (MIT OR Apache-2.0); `semver` (MIT OR Apache-2.0); `ed25519` keys through the TUF crate; `blake3`/`sha2` (in the tree).
- Node 26.12 (`node:sqlite`), Effect 4.0.6, vitest.
- **No other new dependency.** Each task that adds one runs `cargo deny` and the JS licence gate and records the licence in its commit body.

**Spec:** `docs/specs/2026-11-07-sub-project-8-plugins-design.md` §1–6, §11–14, §16–19 (8A parts). Parent: arch §8.3, §14, §15, §16, §18, §21.1. Related: sub-project 5 spec §3.1–3.3, §4.4–4.6 (control store, principals, audit, scopes, human-only, console) on branch `sp5-design`; the 4C plan Decision 30 (font catalogs).

**Branch:** `sp8a-packages`, stacked on **`sp5a-principals` after its Task 12** (8A needs `control.db`, the audit log, the scope table, human-only enforcement and the console). Until 5A exists, Tasks 1–7 (pure Rust: `fin-plugin-api`, `fin-registry`, `fin-plugin`) can be built on `sp4c-edit-breadth` and rebased; Tasks 8–14 cannot.

**Out of scope:** running code plugins, the protocol, the `plugin` principal's runtime use, SDKs, `fin-plugin-test` (8B); `plugin.pin` ops and WGSL effects (8H); analysis (8C+); providers (8G); the public registry service, HTTPS registries (the transport trait allows them; none is built), Sigstore; the UI dialogs (9); serving skills to agents (`fin guide`, 7); Python runtimes (8H).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code. `pnpm gates` runs cargo-deny and the JS licence gate.
- **Primitive ops live only in `fin-core`.** This plan adds none.
- **`fin-plugin-api` stays pure:** no I/O, clock, environment or randomness; it builds for `wasm32-unknown-unknown` (added to `scripts/check-wasm-deps.mjs`).
- **TypeScript model types are generated, never hand-written.** Manifest, permission, plan, registry status and event payload types derive `JsonSchema`, are added to `crates/fin-store/examples/export_schema.rs` in the task that creates them, and are regenerated with `pnpm --filter @finisher/core-types generate` (drift gate `scripts/check-generated-types.mjs`). Control-plane rows owned by TS are Effect Schemas in `@finisher/contracts`.
- **Dependency rules:** `fin` links neither `fin-registry` nor `gix` nor the TUF crate (it only calls RPCs, except `fin plugin validate`, which links `fin-plugin-api`); `fin-core` does not depend on `fin-plugin-api`; `fin-plugin-api` depends on no crate of ours (it is a leaf); `fin-node` links `fin-registry`; `fin-plugin-cli` is not in the app bundle (a test in `scripts/check-test-hooks.mjs`'s style asserts the packaging list does not name it).
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. New logic goes into new files; `apps/finisherd/src/handlers.ts`, `main.ts` and `crates/fin/src/main.rs`, `registry.rs` take call-site lines only.
- Errors are `{ code, message, details, fix? }`. New codes go into `crates/fin/src/output.rs` with the exits of spec §13 and into `@finisher/contracts`' code list.
- **No key material in tracked files.** Test keys are generated per test; the few fixed vectors (Task 4) use keys whose file names and ids say `test-only` and are never accepted by a non-test build (a test checks that the bundled root hash differs from every fixture root).

**Testing and process**
- Tests never use the network. Git registry tests use `file://` repositories made in a temp dir.
- Clocks are injected (`fin_registry::Clock`) for expiry, freshness and the clock check.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`. Tasks that touch TS or `fin` also run `rtk pnpm e2e` once at the end.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh`.
- **The repo is public:** no host names, IPs, user names or key names in tracked files, fixtures included.
- **Tracker:** each task ticks its line in `TASKS.md` under a new heading "Sub-project 8A: packages, registries and lifecycle", in the same commit.
- **Batches.** Each batch must be green before the next starts:
  - A, types and packages: Tasks 1–3;
  - B, TUF and registries: Tasks 4–7 (5 and 6 may run beside each other after 4);
  - C, server and consumers: Tasks 8–12 (11 may run beside 10);
  - D, e2e, docs and validation: Tasks 13–15.

## Decisions taken here (the spec fixes the shape; these fix the details)

### Types and packages
1. **Manifest file name** `finisher-plugin.json`, at the package root, ≤ 256 KiB (`MANIFEST_TOO_LARGE`), UTF-8, no comments (the spec's `jsonc` is illustration).
2. **Ids:** `@<publisher>/<name>`, publisher `[a-z0-9][a-z0-9-]{1,38}`, name `[a-z0-9][a-z0-9-]{1,63}`, no `--`, no leading or trailing `-`. Reserved publishers (spec §4.1) are refused outside the bundled registry; `@dev` only in developer mode.
3. **Versions:** semver 2.0 without build metadata (`+…` refused: it would make two "different" immutable versions compare equal).
4. **8A validates every contribution kind the spec names**, but only `fontCatalogs` and `captionStyles` have consumers. Code contributions (`commands`, `hooks`, `analyzers`, `providers`, `embedders`, `codecBackends`, `importers`, `matteProducers`, `audioProcessors`, `slots`) are validated structurally (their JSON Schemas compile, ids are unique); their semantics are 8B+.
5. **Licence classes:** `permissive` (every branch of the SPDX expression on the allow-list in `deny.toml`), `weakCopyleft` (MPL-2.0, EPL-2.0 and similar, only as an allowed branch), `copyleft` (GPL, LGPL, AGPL, SSPL, EUPL, CC-BY-SA in any required branch), `nonCommercial` (any NC licence: **refused**), `unknown` (not SPDX: refused in bundled and first-party registries; a warning elsewhere). An `OR` expression takes its most permissive branch; an `AND` its least.
6. **Packages are zip files** (`zip` 9). `fin-plugin pack` writes entries sorted by name, stored or deflate, with the DOS epoch timestamp, modes 0644/0755, and no extra fields, so the same input packs to the same bytes on every OS (a test packs on all three and compares hashes in Task 15).

### TUF and registries
7. **Task 1 decides the crate.** `tough` stays if `aws-lc-sys` builds from the pinned version on macOS arm64, Windows x64 (MSVC) and Linux x64 without network access at build time, and the release `finisherd` bundle grows by ≤ 6 MiB; otherwise `tuf` (rust-tuf). Either way `fin-registry::tuf::TrustedRepo` is the only type the rest of the code sees.
8. **Signing keys:** ed25519 for every role. Thresholds: root ≥ 1 (bundled: 2 of 3, set at the key ceremony of sub-project 11; tests use 1 and 2), targets/snapshot/timestamp 1.
9. **Consistent snapshots on**, target names hash-prefixed; root files kept as `<n>.root.json`.
10. **Freshness:** `strict` for `bundled`, `relaxed` for `local` and `git`; `maxStaleDays` is read from the signed root's `custom.finisher.maxStaleDays` (default 30, at most 365); a registry without it uses 30. Rollback checks are strict for both.
11. **Clock check:** `registries.lastRefreshAt` (wall clock, stored); a refresh at a time more than 24 h before it is `CLOCK_SUSPECT` (exit 5) and changes nothing.
12. **Size caps:** root and targets files ≤ 4 MiB, timestamp ≤ 16 KiB, snapshot ≤ 1 MiB, a delegated role ≤ 4 MiB, ≤ 32 root versions per refresh, ≤ 500 delegations.
13. **Git registries:** a bare repository under `registries/<id>/git`, fetched (depth 1) at the configured ref; files read from the tree object. A fetch that fails leaves the previous tree in use (`REGISTRY_UNREACHABLE`, exit 5, with the trusted set unchanged).
14. **Root at add time:** `--root <file>` or `--root-sha256 <hex>`; otherwise the console prints the root's SHA-256 and key ids and requires the owner to type them back. No RPC adds a registry without one of the three.

### Lifecycle
15. **Plans** live in `control.db` `plugin_plans {id, digest, pluginId, version, registryId, action, by (principal kind and id), human, createdAt, expiresAt}` and in `staging/<planId>/`; expiry 10 minutes, swept every minute and at start. `digest` = BLAKE3 of the canonical plan JSON (permissions, licence, staging tree hash).
16. **Approval** `plugin.install {planId, digest, accept: {copyleft?}}` succeeds only for a human principal with `plugin.install` (owner) **and** a plan made by a human principal; a plan made by an agent is refused with `HUMAN_ONLY` even if a person submits its id (the person must make their own plan).
17. **Agent installs** (`registries.agentInstall = true`, set from the console only): an agent may plan and install when the plan is entirely `low` risk and not a widening update; anything else is `HUMAN_ONLY`. Default off.
18. **Code plugins** install with `enabled = false` and `state = active`; `plugin.enable` on a code plugin answers `PLUGIN_RUNTIME_UNAVAILABLE` until 8B (owner question 1).
19. **Content packs** enable at install. Font catalogs are merged by entry id into the server's font catalog (new ids only; a pack entry that reuses a built-in or another pack's id is refused at plan time, `PLUGIN_MANIFEST_INVALID {path, reason: "catalogIdTaken"}`). Caption styles are resolved by `fin style add --from @pub/pack:<name>`, which copies the style into the project through the existing `style.add` op (no pin: the project holds the copy).
20. **The bundled registry** is a TUF repository in the app's resources (`resources/plugins/registry/`), its root hash in `fin-node` as `BUNDLED_ROOT_SHA256` (built from `resources/plugins/registry/metadata/1.root.json` by `build.rs`). 8A ships it with one content pack, `@finisher/captions-basic` (three caption styles made from the built-in presets), signed with test-only keys **in debug builds only**; release signing is sub-project 11 (a TASKS follow-up).
21. **Remove** refuses bundled plugins (`PLUGIN_BUNDLED`, exit 3). Removing a content pack whose fonts a project pinned keeps the font files in the font cache (fonts are content-addressed and pinned by hash, 4C); its catalog entries disappear for new pins.

### Events and audit
22. `plugin.lifecycle` and `registry.refresh` are emitted by `fin-node` (`source: finNode`) from the registry worker; `PluginRef = {bundled: Name(BUNDLED_PLUGINS)} | {hashed: Hashed}`.
23. Audit kinds `plugin.install|update|remove|enable|disable|revoke`, `plugin.grant`, `registry.add|remove|trust|verifyFail` are added to sub-project 5's `AuditKind` with `details` limited to `{pluginId, version, registryId, permissionsAdded, code}`.

### Scopes
24. New rows in `@finisher/contracts` `scopes.ts`: `plugin.read` (`plugin.list|info|search`, `plugin.plan`, `plugin.content.get`, `registry.list|status`), `plugin.install` (owner: `plugin.install`, `plugin.remove`), `plugin.admin` (owner: `plugin.enable|disable`, `registry.add|remove|set|refresh`), `plugin.suggest` (any principal with `project.read`). Defaults: `local` and `agent` get `plugin.read` and `plugin.suggest`; `registry.refresh` is also allowed with `plugin.read` (it changes nothing a person decided).

## Questions for the owner

This plan implements the recommended answer to spec §19 **Question 1** (no third-party code runs before the sandbox): code plugins install but stay disabled. If the owner answers otherwise, Decision 18 changes to "enabled at install for owner-approved plans" and 8B must ship before any enable can succeed anyway, so nothing in 8A changes beyond the default and one test. Questions 2–6 do not affect 8A.

## Review Focus

1. **Nothing unverified reaches the store:** every byte in `store/` is from a target whose hash and length match signed metadata, unpacked by the strict reader; the staging tree is re-hashed at approval.
   → Task 3 `hostile_archive_corpus`; Task 4 `tampered_target_refused`, `length_cap_enforced`; Task 8 `install_rechecks_staging_hash`.
2. **TUF semantics:** rollback of every role, freeze per policy, mix-and-match, root rotation chains with threshold changes, fast-forward recovery, delegation path confinement, trusted metadata written only after a full chain verifies.
   → Task 4 attack matrix; Task 13 `tuf-conformance` run.
3. **Approval is human and binds the exact plan:** digests, expiry, agent-made plans refused, widening updates need a new approval, agent installs bounded to low risk.
   → Task 9 `approval_matrix`, `agent_plan_refused_for_human_submit`, `widening_update_needs_approval`.
4. **Nothing runs at install:** no process is spawned by plan or install (a test counts child processes), and executables keep only the modes the manifest names.
   → Task 8 `install_spawns_nothing`.
5. **Trust is explicit:** no registry without a pinned or typed-back root; the bundled root hash is compiled in and differs from fixture roots.
   → Task 7 `registry_add_needs_root`; Task 10 `bundled_root_is_not_a_fixture`.
6. **Privacy:** journal entries hold no plugin id of a non-bundled plugin in clear, no registry location, no publisher name.
   → Task 11 `lifecycle_events_hold_no_free_text`.
7. **Cross-platform paths:** reserved Windows names, case-folded duplicates and NFD names in packages; read-only store files on Windows; git over `file://` on Windows.
   → Task 3, Task 15.

---

### Task 1: Gate the TUF and git crates on three OSes — tier: opus

**Files:**
- Create: `archive/spikes/sp8a-tuf/` (git-ignored: a throwaway crate that builds `tough` (no `http`), loads a generated repository with `FilesystemTransport`, and fetches a `file://` repository with `gix`; build logs and sizes).
- Modify: `TASKS.md` (records the decision and the numbers).

- [ ] **Step 1:** Build the spike on macOS arm64, Windows 11 x64 (MSVC) and Linux x64 with `CARGO_NET_OFFLINE=true` after a vendored fetch. Record: whether `aws-lc-sys` needs cmake or NASM, build time, the size added to a release `fin-node` `.node`, `cargo deny check licenses` with the current `deny.toml`.
- [ ] **Step 2:** Same for `gix` with the chosen feature set (HTTPS on rustls with the `ring` provider; record whether any feature pulls `aws-lc-rs` or OpenSSL; reject `curl` transports).
- [ ] **Step 3:** Decide per Decision 7 and write the result (crate, versions, features, sizes) into `TASKS.md` and the commit body. **Commit** `docs: record the TUF and git crate gate for 8A`.

### Task 2: `fin-plugin-api`: manifest, permissions, licences, validation — tier: opus

**Files:**
- Create: `crates/fin-plugin-api/{Cargo.toml, src/lib.rs, src/manifest/{mod.rs, id.rs, runtime.rs, contributes.rs, content.rs, code.rs}, src/permission.rs, src/licence.rs, src/validate/{mod.rs, files.rs, schemas.rs, messages.rs, content.rs}, src/schema.rs}`.
- Test: `crates/fin-plugin-api/tests/{manifest.rs, permission.rs, licence.rs, validate.rs}`, fixtures under `crates/fin-plugin-api/tests/fixtures/` (small JSON manifests only).
- Modify: `Cargo.toml` (member, default member), `crates/fin-store/examples/export_schema.rs`, `packages/core-types` (regenerate), `scripts/check-wasm-deps.mjs`, `TASKS.md`.

**Interfaces:**
```rust
pub struct Manifest { pub manifest_version: u16, pub id: PluginId, pub version: semver::Version, pub kind: PluginKind,
    pub host_api: semver::VersionReq, pub license: String, pub publisher: Publisher, pub messages: Option<String>,
    pub locales: Vec<String>, pub platforms: Vec<Platform>, pub runtime: Option<Runtime>, pub permissions: Vec<Permission>,
    pub activation: Vec<Activation>, pub contributes: Contributes, pub models: Vec<ModelRef>, pub previews: Vec<String>,
    pub homepage: Option<String>, pub repository: Option<String> }
pub struct PluginId { publisher: String, name: String }          // Display "@p/n"; FromStr validates Decision 2
pub enum Permission { ProjectRead, ProjectWrite, MediaRead, MediaFile, JobsSubmit, Gpu,
    Net { origins: Vec<Origin> }, Secrets { names: Vec<String>, mode: SecretMode },
    Fs { scope: FsScope, access: FsAccess }, HooksModify { hooks: Vec<String> } }
pub enum Risk { Low, Medium, High }
impl Permission { pub fn risk(&self) -> Risk; pub fn message_key(&self) -> &'static str; }
pub struct PermissionDiff { pub added: Vec<Permission>, pub widened: Vec<(Permission, Permission)>, pub removed: Vec<Permission> }
pub fn diff(old: &[Permission], new: &[Permission]) -> PermissionDiff;   // spec §4.3 widening rules
pub enum LicenceClass { Permissive, WeakCopyleft, Copyleft, NonCommercial, Unknown }
pub fn classify(spdx: &str) -> LicenceClass;                             // Decision 5
pub trait PackageFiles { fn exists(&self, path: &str) -> bool; fn read(&self, path: &str) -> Option<Vec<u8>>; }
pub fn validate(manifest_json: &[u8], files: &dyn PackageFiles, ctx: &ValidateCtx) -> Result<Manifest, Vec<Invalid>>;
pub struct ValidateCtx { pub host_api: semver::Version, pub platform: Option<Platform>, pub registry: RegistryClass /* bundled|firstParty|other|dev */ }
pub struct Invalid { pub path: String, pub reason: &'static str, pub detail: Option<String> }  // → PLUGIN_MANIFEST_INVALID
```

- [ ] **Step 1: Write failing tests:**
  - `ids_follow_the_grammar`, `reserved_publishers_refused_outside_bundled`, `build_metadata_refused`;
  - `content_pack_with_permissions_is_invalid`, `code_plugin_needs_runtime_and_platforms`, `exec_per_platform_matches_platforms`;
  - `every_referenced_file_exists`, `contribution_schemas_compile`, `message_keys_in_en_catalogue`;
  - `font_catalog_contribution_validates_with_fin_fonts_rules` (the catalog's own rules are applied through a callback in `ValidateCtx` so `fin-plugin-api` stays a leaf), `caption_style_contribution_is_a_fin_core_style` (same: a callback);
  - `permission_diff_widening_cases` (each widening rule, plus reordering is no change);
  - `licence_classes` (MIT, `MIT OR GPL-3.0` → permissive, `MIT AND GPL-3.0` → copyleft, `CC-BY-NC-4.0` → nonCommercial, `LicenseRef-x` → unknown, `MPL-2.0` → weakCopyleft);
  - `unknown_fields_refused` (`deny_unknown_fields` everywhere: a typo never silently drops a permission);
  - `schema_export_has_no_free_strings_in_meta` (not applicable: manifests are not journal payloads; instead `manifest_schema_round_trips`).
- [ ] **Step 2: Implement.** The fin-fonts and fin-core checks are injected (`ValidateCtx::content_checks`) by `fin-node`; `fin-plugin-api` has no crate dependency of ours.
- [ ] **Step 3:** Export the schema, regenerate TS, run the gates → PASS. **Commit** `feat(plugin-api): plugin manifest, permissions and licence classes with Rust validation`.

### Task 3: The strict package reader and the reproducible packer — tier: opus

**Files:**
- Create: `crates/fin-registry/{Cargo.toml, src/lib.rs, src/package/{mod.rs, read.rs, names.rs, limits.rs, pack.rs}}`.
- Test: `crates/fin-registry/tests/package.rs`, `crates/fin-registry/tests/support/hostile.rs` (builds hostile archives in code: never committed binaries).
- Modify: `Cargo.toml`, `TASKS.md`.

**Interfaces:**
```rust
pub struct Limits { pub entries: u32, pub unpacked: u64, pub ratio: u32, pub path_bytes: usize, pub component_bytes: usize } // spec §6.4 defaults
pub struct Unpacked { pub root: PathBuf, pub tree_hash: String /* blake3 over sorted (path, mode, sha256) */, pub manifest: Vec<u8>, pub files: BTreeMap<String, FileMeta> }
pub fn unpack(archive: &Path, dest: &Path, limits: &Limits) -> Result<Unpacked, PackageError>;  // PACKAGE_UNSAFE / PACKAGE_TOO_LARGE
pub fn tree_hash(root: &Path) -> Result<String, PackageError>;                                  // re-hash for approval
pub fn pack(src: &Path, executables: &[String], out: &Path) -> Result<PackInfo, PackageError>;   // Decision 6
```

- [ ] **Step 1: Write failing tests** (`hostile_archive_corpus`, one case each): `..` component, absolute and drive-letter paths, backslashes, NUL, symlink and hard-link entries (external attributes), device entries, `CON`/`aux.txt`/`COM1`, trailing dot and space, case-folded duplicates (`A.txt`/`a.txt`), NFD vs NFC duplicates, > 10,000 entries, > 512 MiB declared, a 1 KiB entry claiming 1 GiB (ratio), declared size smaller than written bytes, a nested `finisher-plugin.json` without one at the root; plus `unpack_sets_modes_only_for_executables`, `pack_is_reproducible` (two packs of the same tree, different mtimes and creation order → same bytes), `tree_hash_changes_on_any_byte_or_mode`.
- [ ] **Step 2: Implement.** Entries are streamed to temp files inside `dest`, then renamed; nothing is written through a path that canonicalises outside `dest`.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(registry): strict plugin package reader and reproducible packer`.

### Task 4: The TUF client, trusted store and attack matrix — tier: opus

**Files:**
- Create: `crates/fin-registry/src/tuf/{mod.rs, trusted.rs, freshness.rs, clock.rs, custom.rs}`, `crates/fin-registry/src/transport/{mod.rs, fs.rs}`, `crates/fin-registry/src/testing.rs` (`test-support` feature: `RepoBuilder` with throwaway ed25519 keys, delegations, custom metadata, rotation helpers, expiry control), `tests/fixtures/tuf/vectors/` (two tiny `test-only` repositories pinning canonical JSON and signatures).
- Test: `crates/fin-registry/tests/{tuf_happy.rs, tuf_attacks.rs, tuf_rotation.rs}`.
- Modify: `crates/fin-registry/Cargo.toml` (the crate chosen in Task 1), `TASKS.md`.

**Interfaces:**
```rust
pub trait Transport: Send + Sync { fn fetch(&self, path: &str, max: u64) -> Result<Vec<u8>, TransportError>; }
pub struct Policy { pub freshness: Freshness /* Strict | Relaxed { max_stale_days } */, pub caps: Caps }
pub struct TrustedRepo { /* dir: registries/<id>/tuf */ }
impl TrustedRepo {
    pub fn init(dir: &Path, root: &[u8], expect_sha256: &str) -> Result<Self, TufError>;   // REGISTRY_UNTRUSTED on mismatch
    pub fn open(dir: &Path) -> Result<Self, TufError>;
    pub fn refresh(&mut self, t: &dyn Transport, policy: &Policy, clock: &dyn Clock) -> Result<RefreshReport, TufError>;
    pub fn targets(&self) -> impl Iterator<Item = TargetInfo>;    // name, length, hashes, custom (spec §6.1)
    pub fn fetch_target(&self, t: &dyn Transport, name: &str, out: &Path) -> Result<(), TufError>;
}
pub struct RefreshReport { pub root_from: u64, pub root_to: u64, pub roles_updated: u32, pub stale: Option<u32 /* days */> }
pub enum TufError { Signature, Threshold, Rollback { role, trusted, got }, Expired { role }, TargetMismatch { what }, Delegation, TooLarge, ClockSuspect, Untrusted, Io(..) }
```

- [ ] **Step 1: Write failing tests:**
  - happy: `refresh_from_root_to_targets`, `delegated_targets_resolve`, `custom_metadata_round_trips`, `fetch_target_checks_length_and_hashes`, `trusted_files_written_only_after_full_chain` (a failure at snapshot leaves the old trusted set byte-identical);
  - attacks (one test each): tampered target bytes; target longer than signed length (`length_cap_enforced`); timestamp, snapshot, targets and delegated role version rollback; expired timestamp under `Strict` (refused) and `Relaxed` (warning within `maxStaleDays`, refused beyond); snapshot that lists an older targets version than the timestamp implies (mix-and-match); timestamp signed by a revoked key; root rotation without the old threshold; `@a` delegated role listing `@b/x` (`TufError::Delegation`); oversized timestamp; 33 root versions in one refresh; `ClockSuspect` per Decision 11;
  - rotation: root 1→2→3 with the threshold going 1→2→1; timestamp-key rotation triggers fast-forward recovery (trusted timestamp and snapshot deleted, next refresh accepts a lower timestamp version signed by the new key); publisher key rotation by re-delegation; revoked publisher's new versions refused;
  - vectors: `fixture_vectors_verify_identically` (the two fixed repositories verify to the same target list on every OS).
- [ ] **Step 2: Implement** over the chosen crate. If `tough`: a current-thread `tokio` runtime owned by `TrustedRepo`, `Transport` adapted to its async transport trait, no `http` feature. Everything stored atomically (temp then rename, `fin-store::fsio` style).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(registry): TUF client with trusted store, freshness policy and attack tests`.

### Task 5: The git-tree transport — tier: sonnet

**Files:**
- Create: `crates/fin-registry/src/transport/git_tree.rs`.
- Test: `crates/fin-registry/tests/git_registry.rs` (repositories made with `gix` in a temp dir, served as `file://`).
- Modify: `crates/fin-registry/Cargo.toml` (`gix` with Task 1's features), `TASKS.md`.

**Interfaces:**
```rust
pub struct GitTree { /* bare repo at registries/<id>/git, ref, fetched commit */ }
impl GitTree {
    pub fn open_or_clone(dir: &Path, url: &str, git_ref: &str, auth: Option<&GitAuth>) -> Result<Self, TransportError>;
    pub fn fetch(&mut self) -> Result<FetchReport, TransportError>;   // depth 1; failure keeps the previous commit
}
impl Transport for GitTree { /* reads blobs at <commit>:<path>, never a checkout */ }
pub enum GitAuth { HttpsToken { secret: String } }               // token from SecretStore (cp §5.5), never logged
```

- [ ] **Step 1: Write failing tests:** `reads_metadata_from_tree_without_checkout` (no working files appear on disk), `force_pushed_rollback_is_caught_by_tuf` (the remote is reset to an older commit: refresh fails `Rollback`, trusted set unchanged), `fetch_failure_keeps_previous_tree`, `symlink_blobs_in_tree_are_refused` (a git symlink entry under `targets/` is `TargetMismatch`), `ref_must_be_branch_or_tag`, Windows: `file_url_with_drive_letter`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(registry): git registries read from fetched tree objects`.

### Task 6: Publisher tooling `fin-plugin` — tier: sonnet

**Files:**
- Create: `crates/fin-plugin-cli/{Cargo.toml, src/main.rs, src/cmd/{new.rs, validate.rs, pack.rs, registry.rs}, src/keys.rs}`.
- Test: `crates/fin-plugin-cli/tests/cli.rs` (runs the binary: init a registry, add a publisher, publish two versions, yank one, advise, rotate the timestamp key, resign the timestamp; then `TrustedRepo` verifies the result).
- Modify: `Cargo.toml` (member, **not** a default member of the app packaging list), `TASKS.md`.

**Interfaces:** `fin-plugin new content|code <dir>`; `fin-plugin validate <dir>`; `fin-plugin pack <dir> -o <file>`; `fin-plugin registry init <dir> --root-keys <k…> --threshold <n> [--max-stale-days <d>]`, `registry delegate <dir> --publisher <p> --key <pub>`, `registry publish <dir> <pkg> --key <publisher key>`, `registry yank <dir> <id>@<v>`, `registry advise <dir> --plugin <id> --versions <range> --action warn|disable --summary <text>`, `registry rotate-key <dir> --role <r> --new <k>`, `registry resign-timestamp <dir> [--expires <days>]`. JSON on stdout, errors as `{code, message, details, fix?}`, exit codes as `fin`'s.

- [ ] **Step 1: Write failing tests** for each subcommand's effect, plus `publish_refuses_existing_version` (immutability), `publish_fills_custom_metadata_from_manifest`, `keys_are_never_printed`.
- [ ] **Step 2: Implement** over `fin-registry::testing`'s builder promoted to a non-test `editor` module (the builder keeps only test conveniences).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(plugin-cli): fin-plugin publisher tooling for local and git registries`.

### Task 7: Registries, search, yanks and advisories — tier: opus

**Files:**
- Create: `crates/fin-registry/src/{registry.rs, advisory.rs, search.rs}`.
- Test: `crates/fin-registry/tests/{registry.rs, advisories.rs}`.
- Modify: `TASKS.md`.

**Interfaces:**
```rust
pub struct RegistryConfig { pub id: String, pub kind: RegistryKind /* Bundled|Local|Git */, pub location: String, pub git_ref: Option<String>,
                            pub root_sha256: String, pub freshness: Freshness, pub agent_install: bool }
pub struct Registry { /* config + TrustedRepo + Transport */ }
impl Registry {
    pub fn add(base: &Path, cfg: RegistryConfig, root: RootSource /* File | Sha256 | Confirmed(sha) */) -> Result<Self, RegistryError>;
    pub fn refresh(&mut self, clock: &dyn Clock) -> Result<RefreshReport, RegistryError>;
    pub fn search(&self, q: &SearchQuery) -> Vec<Listing>;          // over signed custom metadata; yanked hidden unless exact
    pub fn info(&self, id: &PluginId) -> Option<PluginInfo>;         // versions, yanked flags, advisories
    pub fn advisories(&self) -> Vec<Advisory>;                       // publisher + registry-wide, verified
}
```

- [ ] **Step 1: Write failing tests:** `registry_add_needs_root` (no source → `REGISTRY_UNTRUSTED`; wrong hash → refused), `search_reads_custom_metadata_only` (no target downloaded), `yanked_hidden_but_exact_installable`, `advisory_from_other_publisher_ignored`, `registry_wide_advisory_applies`, `advisory_version_ranges`, `bundled_kind_is_strict`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(registry): registries with search, yanks and signed advisories`.

### Task 8: The store, staging and plans in `fin-node` — tier: opus

**Files:**
- Create: `crates/fin-registry/src/{store.rs, plan.rs}`, `crates/fin-node/src/registry.rs`.
- Test: `crates/fin-registry/tests/{plan.rs, store.rs}`, `packages/fin-node/test/registry.test.ts`.
- Modify: `crates/fin-node/src/lib.rs`, `crates/fin-node/Cargo.toml`, `packages/fin-node` (typings), `crates/fin-store/examples/export_schema.rs` (`InstallPlan`, `RegistryStatus`, `InstalledPlugin`), `TASKS.md`.

**Interfaces:**
```rust
pub struct InstallPlan { pub plan_id: String, pub digest: String, pub plugin: PlanPlugin, pub registry: PlanRegistry, pub action: PlanAction,
    pub from: Option<semver::Version>, pub permissions: Vec<PlannedPermission>, pub diff: PermissionDiff, pub licence: PlanLicence,
    pub bytes: u64, pub models: Vec<PlanModel>, pub runtimes: Vec<PlanRuntime>, pub warnings: Vec<PlanWarning>, pub expires_at: u64 }
pub fn make_plan(reg: &Registry, store: &Store, req: &PlanRequest, ctx: &ValidateCtx, clock: &dyn Clock) -> Result<InstallPlan, PlanError>;
impl Store {
    pub fn commit(&self, plan: &InstallPlan) -> Result<InstalledRef, StoreError>;   // re-hash staging (PLAN_MISMATCH), rename into store, read-only
    pub fn remove(&self, id: &PluginId, version: &semver::Version, purge_data: bool) -> Result<(), StoreError>;
    pub fn sweep_staging(&self, now: u64) -> u32;
}
```
```ts
// napi (packages/fin-node): long calls on the worker thread, results are generated types
export class Registry {
  static open(serverData: string): Registry;
  add(cfg: RegistryConfig, root: RootSource): Promise<RegistryStatus>;
  refresh(id: string): Promise<RefreshReport>;
  search(q: SearchQuery): Promise<Listing[]>; info(id: string): Promise<PluginInfo | null>;
  plan(req: PlanRequest): Promise<InstallPlan>;
  commit(planId: string, digest: string): Promise<InstalledRef>;
  remove(id: string, version: string, purgeData: boolean): Promise<void>;
  sweep(): number;
}
export function validateManifest(dir: string): ValidationReport;
```
Decisions 15, 19, 21; spec §5.1, §5.3, §5.4.

- [ ] **Step 1: Write failing tests:** `plan_verifies_unpacks_and_validates`, `plan_manifest_must_match_custom_metadata`, `plan_warns_on_lookalike_id` (edit distance 1 to installed or bundled ids), `plan_flags_copyleft_and_refuses_nonCommercial`, `plan_refuses_incompatible_host_api_and_platform`, `install_rechecks_staging_hash` (a byte changed in staging → `PLAN_MISMATCH`), `install_spawns_nothing` (child-process count before and after), `store_files_are_read_only` (unix modes; Windows read-only attribute), `remove_keeps_data_unless_purged`, `staging_swept_after_expiry`, `catalog_id_taken_refused`, napi: `long_calls_do_not_block_event_loop` (event-loop delay p99 ≤ 5 ms during a 50 MiB plan).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(node): plugin store, staging and install plans`.

### Task 9: Lifecycle in `finisherd`: control store, scopes, RPCs, approvals, audit — tier: opus

**Files:**
- Create: `apps/finisherd/src/plugins/{registries.ts, lifecycle.ts, plans.ts, rpc.ts}`, `apps/finisherd/src/control/migrations/m00NN-plugins.ts`, `packages/contracts/src/plugins.ts`.
- Test: `apps/finisherd/test/plugins-lifecycle.test.ts`, `apps/finisherd/test/plugins-scopes.test.ts`.
- Modify: `packages/contracts/src/{rpc.ts, scopes.ts, index.ts, errors.ts}`, `apps/finisherd/src/handlers.ts` (call sites), `apps/finisherd/src/control/audit.ts` (kinds), `crates/fin/src/output.rs` (codes), `TASKS.md`.

**RPCs:** `registry.list`, `registry.status {id}`, `registry.add {cfg, root}`, `registry.remove {id}`, `registry.set {id, freshness?, agentInstall?}` (console only for `agentInstall`), `registry.refresh {id?}`; `plugin.list`, `plugin.search {query, registry?}`, `plugin.info {id}`, `plugin.plan {id, version?, registry?, update?, allowDowngrade?}`, `plugin.install {planId, digest, accept}`, `plugin.remove {id, purgeData?}`, `plugin.enable {id}`, `plugin.disable {id}`, `plugin.suggest {id, reason, projectId}`, `plugin.content.get {id, kind, name}`. Scopes per Decision 24.

- [ ] **Step 1: Write failing tests:**
  - `approval_matrix`: plan and install for each principal kind (`console`, owner device stub, `local`, `agent`) × plan maker (human, agent) × risk (low, high) × `agentInstall` on/off;
  - `agent_plan_refused_for_human_submit`, `digest_must_match`, `plan_expires`, `widening_update_needs_approval`, `non_widening_content_update_applies_when_autoUpdate`, `code_plugin_installs_disabled` and `enable_code_plugin_is_runtime_unavailable` (Decision 18), `bundled_cannot_be_removed`;
  - `advisory_disable_on_refresh` (state `revoked`, audit `plugin.revoke`), `refresh_failure_keeps_installed_plugins_running`;
  - `every_new_rpc_has_a_scope` (extends 5A's `rpc_scopes_cover_every_rpc`), `install_remove_enable_are_owner_only`;
  - `audit_details_allow_list` (Decision 23);
  - `suggest_opens_plugin_suggestion_item` — if 5E has not landed, the item is recorded in a `plugin_suggestions` table with the same fields and the test checks that; a TASKS follow-up moves it to review items.
- [ ] **Step 2: Implement.** The migration creates `registries`, `plugins`, `plugin_grants`, `plugin_plans` (spec §5.1, Decision 15) with `formatVersion` handling of cp §3.1.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): plugin lifecycle with plans, owner approval, scopes and audit`.

### Task 10: The bundled registry and first-start installation — tier: sonnet

**Files:**
- Create: `resources/plugins/registry/` (debug-signed repository built by a script), `resources/plugins/src/captions-basic/` (the pack's source: manifest, three styles, `en` catalogue), `scripts/build-bundled-registry.mjs` (packs and publishes with `fin-plugin` and test-only keys in debug; refuses to run for release), `crates/fin-node/build.rs` (computes `BUNDLED_ROOT_SHA256`), `apps/finisherd/src/plugins/bundled.ts`.
- Test: `apps/finisherd/test/plugins-bundled.test.ts`, `crates/fin-node/tests/bundled_root.rs`.
- Modify: `apps/finisherd/src/main.ts` (call site after the control store opens), `TASKS.md` (follow-up: release signing of the bundled registry, sub-project 11).

- [ ] **Step 1: Write failing tests:** `bundled_installs_at_first_start`, `bundled_reinstalls_after_app_update` (a newer bundled version replaces the old one without approval: same publisher, no widening, else it stays and is reported), `bundled_root_is_not_a_fixture` (the compiled hash differs from every `tests/fixtures/tuf` root), `tampered_bundled_package_refused_at_start` (server starts; the plugin is `revoked` with `TUF_TARGET_MISMATCH` in status), `release_build_refuses_test_keys` (the script's guard).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): bundled plugin registry verified and installed at start`.

### Task 11: Diagnostics kinds — tier: sonnet

**Files:**
- Create: `crates/fin-core/src/events/payloads/plugin.rs`.
- Modify: `crates/fin-core/src/events/{kinds.rs, names.rs, payloads/mod.rs}`, `crates/fin-node/src/registry.rs` (emitters), `crates/fin-store/examples/export_schema.rs`, `packages/core-types` (regenerate), `TASKS.md`.

**Interfaces:** `PluginLifecycle { action: Name(PLUGIN_ACTIONS), outcome: LifecycleOutcome, code: Option<Code>, plugin: PluginRef, registry: RegistryKind, kind: PluginKindName, permissions_added: u8, risk: RiskName, bytes_log2: u8, micros: u64 }`; `RegistryRefresh { registry: RegistryKind, outcome, code: Option<Code>, roles_updated: u16, targets_log2: u8, stale_days: Option<u16>, micros: u64 }`; `PluginRef = Bundled(Name(BUNDLED_PLUGINS)) | Hashed(Hashed)`. Decision 22.

- [ ] **Step 1: Write failing tests:** `kinds_registered_v1`, `plugin_ref_never_free_text`, `lifecycle_events_hold_no_free_text` (an install from a local registry: the journal has no plugin id, publisher, registry path or URL in clear; greps the entry), `bundled_ref_is_a_name`, `events_off_means_nothing_built` (the gate check before payload construction).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(core): plugin.lifecycle and registry.refresh diagnostics kinds`.

### Task 12: Content consumers, console and `fin` — tier: sonnet

**Files:**
- Create: `apps/finisherd/src/plugins/content.ts`, `apps/finisherd/src/console/plugin.ts`, `crates/fin/src/commands/plugin.rs`.
- Test: `apps/finisherd/test/plugins-content.test.ts`, `apps/finisherd/test/console-plugin.test.ts`, `crates/fin/tests/plugin_cmd.rs`.
- Modify: `crates/fin-node/src/fonts/*` (extra catalogs: `FontManager::set_extra_catalogs(Vec<FontCatalog>)`, merged by entry id, Decision 19), `crates/fin-fonts/src/catalog.rs` (`FontCatalog::merged(&self, extra: &[FontCatalog]) -> Result<FontCatalog, CoreError>`), `crates/fin/src/commands/style.rs` (`--from @pub/pack:<name>` → `plugin.content.get`), `crates/fin/src/registry.rs` (new commands), `TASKS.md`.

**Console** (owner, cp §4.6): `finisherd registry add <id> (--dir <path> | --git <url> [--ref <r>]) [--root <file> | --root-sha256 <hex>]`, `registry ls|status|refresh|rm|set`; `finisherd plugin search|info|ls`, `plugin install <id>[@<v>] [--registry <r>] [--accept-copyleft]` (prints the plan, asks `yes`), `plugin update <id>`, `plugin rm <id> [--purge-data]`, `plugin enable|disable <id>`.
**`fin`:** `fin plugin search <q>`, `fin plugin info <id>`, `fin plugin ls`, `fin plugin validate <dir>` (offline, links `fin-plugin-api`), `fin plugin suggest <id> --reason <text>`, `fin plugin install <id>` (→ exit 7 `HUMAN_ONLY` naming the console, unless Decision 17 applies).

- [ ] **Step 1: Write failing tests:** `pack_fonts_join_the_catalog` (`fin fonts catalog` lists a pack's family; disabling the pack removes it; a pinned font stays usable), `style_from_pack_copies_into_project` (the resulting `style.add` op holds the full style; removing the pack later changes nothing in the project), `console_install_prints_plan_and_needs_yes` (scripted stdin), `fin_plugin_install_is_human_only`, `fin_plugin_validate_offline`, `fin_plugin_search_json_shape`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates and `pnpm e2e` → PASS. **Commit** `feat: content packs for fonts and caption styles; plugin console and fin commands`.

### Task 13: e2e `plugins` scenario and the TUF conformance run — tier: sonnet

**Files:**
- Create: `scripts/e2e/plugins.mjs`, `scripts/tuf-conformance/` (a tiny client binary target `fin-tuf-conformance` in `crates/fin-registry/examples/`, and the CI job definition with the suite pinned by commit SHA).
- Modify: `scripts/e2e.mjs`, `.github/workflows/*` (a Linux-only job; actions pinned to SHAs), `TASKS.md`.

- [ ] **Step 1:** e2e: start a server; `fin-plugin` builds a local registry and a git registry (`file://`) with a content pack and a code plugin; the console adds both (root by hash); `fin plugin search` finds both; `fin plugin install` is exit 7; the console installs the pack (permission plan printed) and the code plugin (installed, disabled); `fin style add --from` works; publish 1.1 with an added `net` permission → console update shows the widening and needs `yes`; a `disable` advisory revokes 1.0; a rollback of the git registry's remote is refused (`TUF_ROLLBACK`) and installed plugins keep working; journal grep for free text; audit rows present.
- [ ] **Step 2:** CI: `tuf-conformance` against `fin-tuf-conformance` on Linux; record which tests are skipped (the suite's HTTP-only cases map to the filesystem transport where possible) in the job's summary.
- [ ] **Step 3: Run** → PASS. **Commit** `test: e2e plugin lifecycle scenario and TUF conformance job`.

### Task 14: Spec "as built", architecture pointer, notices, tracker — tier: sonnet

**Files:**
- Modify: `docs/specs/2026-11-07-sub-project-8-plugins-design.md` (§4–6 rewritten as built, with these decisions), `docs/specs/2026-11-01-finisher-architecture-design.md` (§27 pointer for 8A; §15 and §14.4 cross-references), `THIRD_PARTY_NOTICES.md` (the TUF crate and its crypto library, `gix`, `jsonschema`, `spdx`, `semver`), `README.md` (plugins section: registries and the console), `TASKS.md`.

- [ ] **Step 1:** Write the as-built text; list deviations from the spec in a "Decisions taken by the 8A plan" table, as sub-project 4's §3 does.
- [ ] **Step 2: Run** the gates → PASS. **Commit** `docs: record sub-project 8A as built`.

### Task 15: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

- [ ] **Step 1:** On macOS arm64, Windows 11 x64 and the Linux VM: the full test suite, `pnpm e2e` (plugins scenario), `fin-plugin pack` hash equality across the three OSes for the same source tree, a local registry on the NAS share used by 4A Task 19 and on a synced folder (paths from the environment, never in the repo), a git registry over HTTPS from a public test repository whose URL is supplied by the environment, read-only store attributes on Windows, a package with Windows-reserved names refused on all three.
- [ ] **Step 2:** Record results in the spec's validation subsection and `TASKS.md`. **Commit** `docs: record 8A cross-platform validation`.
