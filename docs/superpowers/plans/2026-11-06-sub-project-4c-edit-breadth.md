# Sub-project 4C: Edit breadth — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (model and upcasters, ops with their inverses, footprints, shift and content maps, rebase, evaluation, captions, text measurement, font pinning, fetching and caching, `fin` composition), `sonnet` for plumbing (fixtures, probes, bindings, RPC, CLI, docs).

**Goal:** The model covers everything v1 edits, and evaluation tells the media engine exactly what to draw and play:
- Several timelines, bins and markers. Video, audio and caption tracks. Items of kind `clip`, `caption`, `graphic`, `image`, `adjustment` and `nested`, each with transform, crop, opacity, blend, effects, mask, audio params, keyframes and link groups. Transitions at cuts.
- A settings registry drives generic `param.set`, `keyframe.*` and `effect.*` edits.
- Structural edits with explicit modes (`ripple`, `overwrite`, `lift`, `roll`), each with an exact inverse, a footprint, a shift map and a content map for item-relative times, so 4B's rebase, merge and undo work on all of them.
- `fin-core::eval` produces deterministic frame and audio plans.
- Caption layouts come from the new `fin-text` crate:
  - **projects pin fonts by SHA-256**, so every machine measures with identical bytes;
  - fonts come from a pinned **font catalog** and are **fetched on demand** into a shared, content-addressed cache. No fonts are bundled;
  - imported font assets pin by hash too.
- New asset kinds are probed. `fin view`, `fin status`, `fin fonts`, and a `fin` command for every edit.
- Every pre-4C project (sub-project 2, 4A, 4B) opens with the same state in the new model.

Everything is driven and tested through `fin` → `finisherd`. There is no UI work.

**Architecture:**
- **`fin-core`** gains:
  - the v2 model (`model/`), including **a `fonts` registry of pins** (key → exact file by SHA-256) and the project's fallback order; schema version 2; and frozen upcasters for snapshots, records and stored diffs (`upgrade/`);
  - **per-op diffs:** every change and shift carries its op index, and start-edge changes carry their content offset;
  - about 45 primitive ops, one module each, `font.pin`, `font.unpin` and `font.repin` included. `clip.*` stay as client aliases;
  - `params/`, `eval/`, `captions/`, and `layout` (the `TextMeasurer` contract, the face resolution from pins, script detection); `compose/` (pure multi-op builders that `fin` uses);
  - a measurer threaded through the edit path (`ApplyEnv`), the rebase engine and the history planners. **Apply and replay never fetch, and never read font bytes.**
- **`fin-text`** (new) is pure and builds for wasm32. It shapes and measures over font bytes looked up by SHA-256 through a `FontBytes` provider. It is the one source of font family and script-coverage information (`font_info`).
- **`fin-fonts`** (new, server side) owns the font catalog (a versioned, pinned JSON manifest with a licence gate) and the shared cache `<server-data>/fonts/<sha256>`. That cache is content-addressed, written temp-then-rename, verified before use, safe across processes, resumable, works offline and respects proxies. A fetcher over `ureq` + `rustls` fills it, and the pin planner turns a tx's font needs into `font.pin` ops.
- **`fin-store`** gains:
  - DB migration 0003 (a version bump only);
  - stored v1 diffs upcast on read;
  - alias normalisation at commit;
  - an injected measurer;
  - a project file at a newer schema opens read-only. This is a cherry-pickable fix that lets 4B degrade too.
- **`fin-probe`** probes images, SVG, dotLottie, fonts (with a whole-file SHA-256), `.cube` LUTs and audio-only files.
- **`fin-node`, `@finisher/contracts`, `finisherd`:**
  - the font manager (fetch triggers, progress, status warnings), a `FontBytes` provider over the cache and font assets, one shared measurer;
  - the `eval.frame`, `eval.audio` and `fonts.*` RPCs. Edits keep going through `tx.commit`.
- **`fin`** gets:
  - a command for every edit (`--dry-run`, echoed mode and resolved values, font pins planned through the server). `fin clip insert` of an A/V asset adds a linked audio item by default;
  - `fin view`, `fin status`, `fin fonts`, `fin schema param` and `fin eval`.

**Tech Stack:**
- Rust 2024, rusqlite 0.37 bundled, `blake3`, `proptest` (dev). `serde_json` gains `float_roundtrip` in `fin-core` (feature unification makes it workspace-wide).
- New crates, all permissive (Task 4 and Task 5 check each one with `cargo deny`):
  - **`fin-core`:** `unicode-script` (MIT OR Apache-2.0), pure, wasm-safe.
  - **`fin-text`:** `cosmic-text` 0.19 (MIT OR Apache-2.0), `default-features = false, features = ["no_std"]`. It brings:
    - `fontdb` 0.23 (MIT), `harfrust` 0.5 (MIT), `skrifa` 0.40 / `read-fonts` (MIT OR Apache-2.0);
    - `unicode-bidi`, `unicode-segmentation` (MIT OR Apache-2.0), `unicode-linebreak` (Apache-2.0), `libm` (MIT OR Apache-2.0);
    - `self_cell` (Apache-2.0 OR GPL-2.0-only, **used under Apache-2.0**; recorded in `THIRD_PARTY_NOTICES.md`).

    No `swash` in 4C.
  - **`fin-fonts`:**
    - `ureq` 3.5 (MIT OR Apache-2.0) with `default-features = false, features = ["rustls-no-provider", "platform-verifier"]`, plus `win-system-proxy` on Windows;
    - `rustls` 0.23 (Apache-2.0 OR ISC OR MIT) with the **`ring`** provider (0.17, Apache-2.0 AND ISC; both allowed);
    - `rustls-platform-verifier` 0.7 (MIT OR Apache-2.0; OS trust stores, so corporate CAs work; on Linux `rustls-native-certs` and `rustls-webpki` (ISC));
    - `sha2` (MIT OR Apache-2.0), `zip` (archive members, as in `fin-probe`).
    - Not used: `webpki-roots` (CDLA-Permissive-2.0, not in `deny.toml`) and `aws-lc-rs` (a C/cmake build). `rustls-platform-verifier` pulls `webpki-root-certs` only on wasm32/Android, so `deny.toml` `[graph]` gains `targets` = the shipped triples (x86_64/aarch64 for Apple, Linux and Windows), keeping those out of the licence graph.
  - **`fin-probe`:** `imagesize` 0.15 (MIT), `zip` 9 (MIT; `default-features = false, features = ["deflate-flate2-zlib-rs"]`), `roxmltree` 0.21 (MIT OR Apache-2.0), `sha2`, `fin-text` (font names and coverage).
  - **`fin-testfonts`** (new, dev only, `publish = false`): `write-fonts` 0.55 (MIT OR Apache-2.0) generates test fonts at test time.
  - **`fin-core` dev:** `blake3`.
  - **CI only:** `wasmtime` (Apache-2.0 WITH LLVM-exception).
- **No font files are in the repository or in any package.** The initial catalog (Decision 30) lists upstream OFL-1.1 files at pinned commits or release tags.
- Node 26, Effect 4, vitest.

**Spec:** `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` §5 (4C). The plan also depends on:
- §1–2;
- §3.1–3.4 (versioning, frozen migrations, `finisher.project.json`);
- §4.3 (rebase, footprints, shift maps), §4.4–4.6, §4.8;
- §6 and §8.

Parent: `docs/specs/2026-11-01-finisher-architecture-design.md` §6.2–6.4, §7, §9, §12.3–12.7 and §27.10 (S11b `fin-text`).

**Branch:** `sp4c-edit-breadth` (the plan was committed as `6251bc7` on `36fe7e0`). 4A Task 19 and 4B Task 23 still gate their merges. Task 29 here runs 4C's validation.

**Out of scope** (spec §1): pixels and sound (compositor, mixer, rasterising text, sanitising SVG), transcription, `.otio`, `fin apply` / `fin batch` changes / `fin tx`, review items, `fin-wasm` replay tests, and UI.

**Also moved:**
- the edits that make `nested` items and masks go to sub-project 6 (Decision 48);
- catalogs supplied by the registry, and signed catalogs, go to sub-project 8 (Decision 30).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code. `pnpm gates` runs cargo-deny and the JS licence gate. Task 5 adds the **font catalog licence gate**.
- **`fin-core` is pure:** no I/O, clocks or randomness. It builds for `wasm32-unknown-unknown`, and so does `fin-text` (Task 4 adds it to `pnpm gates`).
- **Dependency rules:**
  - `fin-core` never depends on `fin-text` or `fin-fonts`;
  - `fin-text` has no I/O;
  - **`fin` links neither `fin-text`, `fin-fonts` nor `fin-probe`**, so it carries no HTTP or TLS stack and no shaper.
- **Primitive ops live only in `fin-core`, one module per op** under `crates/fin-core/src/ops/`. Each module holds that op's validate, apply, inverse, `footprint`, `positions_mut`, `offsets_mut` and `refs_mut`. Shared helpers live in `ops/util/`. Multi-op builders live in `compose/` and are never ops.
- **TypeScript model types are generated, never hand-written.**
  - Every new wire type derives `JsonSchema` and is reachable from `fin_core::json_schema` or added in `crates/fin-store/examples/export_schema.rs`, in the task that creates it.
  - Then run `pnpm --filter @finisher/core-types generate`. The drift gate is `scripts/check-generated-types.mjs`.
- **File sizes:** non-test source files ≤ 600 lines, tests ≤ 1,500.
  - Near the limit at `36fe7e0`:
    - `fin-store`: `lease/clock.rs` 600, `tick.rs` 587, `fsio.rs` 582, `derived.rs` 519, `project_file.rs` 469;
    - `fin-store-xhost`: `node.rs` 560;
    - `fin`: `client.rs` 525, `commands/mod.rs` 487 (Task 1 splits it);
    - `fin-core`: `history/merge.rs` 478.
  - **`fin-core/tests/props_history.rs` is at 1,483 lines**, so 4C property code goes into new files.
- Errors are `{ code, message, details, fix? }`. Every new code goes into `crates/fin/src/output.rs` with the exit code Decision 45 gives.
- **No media and no font files are committed.** Test media is generated in code (`fin-probe` `fixtures`). Test fonts are generated at test time (`fin-testfonts`).

**Formats and migrations**
- **Migrations are frozen once shipped.** `m0001_records.rs` and `m0002_history.rs` and their pins never change. m0003 writes literal values only.
- **Upcasters are frozen the same way:** `serde_json::Value` steps with literal names and golden tests. Until 4C merges, a step may still be edited together with its golden.
- **Stored bytes are never re-encoded.** Old records, stored diffs and snapshot rows are upgraded in memory on read. The record and segment formats do not change.
- **Replay applies; it never re-judges** (spec §2). Replay never needs a measurer, a catalog or font bytes. **Nothing measured or fetched is ever written to a record, a diff or a snapshot.** Pins are plain data in ops.

**Determinism** (spec §5.4, §5.6)
- **Eval** (`fin-core/src/eval/`, `params/`, `captions/`, `layout.rs`) uses only:
  - `f64` `+ − × ÷`, `sqrt`, comparisons, `abs`, `min`/`max`, `floor`;
  - conversions from integers.

  It never uses `sin`, `cos`, `tan`, `pow*`, `exp`, `ln`, `log*`, `hypot`, `atan*`, `mul_add` or any `libm::` function. Task 17 adds a gate.
- **Text:**
  - the bytes are exactly the project's pinned files (by SHA-256);
  - the fallback order is the project's, never the platform's;
  - the locale is fixed; float functions go through `libm` (`no_std`);
  - output is in 1/64 px integers.
- Plan and layout hashes are BLAKE3 of compact JSON, compared across macOS, Windows 11, Linux and wasm32 (Tasks 4, 17, 29).

**Network and caches**
- **Tests never use the internet.** The fetcher is tested against a local HTTP server started by the test, with fault injection.
- Tests that need **real** catalog fonts read them from a cache warmed beforehand (`FIN_FONT_CACHE`, filled by `cargo run -p fin-fonts --example warm_cache`). CI warms and caches it and sets `FIN_REQUIRE_FONTS=1`, so those tests fail rather than skip there. Locally they skip with a printed notice when the cache is absent.

**Testing and process**
- **Property tests run inside `cargo test`.** All properties (4B's included) take under 30 s in a debug build on the Linux VM. Each `proptest!` sets `ProptestConfig { cases, .. }` explicitly and honours `PROPTEST_CASES`.
- **Every edit gets golden tests:** apply, inverse, shift map and `inverse_shifts_mirror` (Decision 23). **Every op with nested ids gets `ids_cover_every_id_in_json`** (Decision 24).
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`. Tasks that touch the TS side or `fin` also run `rtk pnpm e2e` once at the end.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh` (Node 26).
- **The repo is public:** no host names, IPs, user names or key names in tracked files, fixtures included. Cross-host details stay in `archive/hosts.md`. Spikes and scratch work go in `archive/`.
- **Tracker:** each task ticks its line in `TASKS.md` under "Sub-project 4C: edit breadth", in the same commit.
- **Batches** (spec §8). Each batch must be green before the next starts:
  - A, foundations, model and structural edits: Tasks 1–10;
  - B, params, structure ops and eval: Tasks 11–17;
  - C, captions, fonts in the edit path, assets and server: Tasks 18–22;
  - D, `fin`: Tasks 23–25;
  - E, properties, e2e, docs and validation: Tasks 26–29.

  Tasks 4 and 5 may run beside Tasks 3 and 6. Task 21 may run beside Tasks 15–20.

## Decisions taken here (the spec leaves them open; Task 28 records them in the spec)

### Versions, migrations and upgrade

1. **One schema version for ops and model: `SCHEMA_VERSION` 1 → 2.**
   - Every record 4C writes is version 2. A 4B (or 4A) follower reads one as `NEWER_FORMAT` and goes read-only. **This is accepted:** the spec and the README upgrade notes say so.
   - The segment, record and `.fsnap` framing do not change; `.fsnap` headers say `modelSchemaVersion: 2`.
2. **`finisher.project.json` `schemaVersion`:**
   - new projects write 2;
   - an existing file is never bumped;
   - reads accept 1 and 2;
   - **a higher value opens read-only `NEWER_FORMAT`**: `ProjectFile::is_newer()` counts it, and `read_project_file_once` no longer refuses it. Today `project_file.rs` fails `SCHEMA_VERSION` at open; only the poll in `tick.rs` `reread_project_file` treats it as newer.

   Task 3 lands this as its own commit, which also applies to `sp4b-history` unchanged, for the coordinator to cherry-pick there. Otherwise Task 28 documents that 4B refuses 4C-created projects.
3. **DB schema v3 (`m0003_version`, frozen) is a version bump only:** `user_version` 3, `formatVersion`/`minReaderVersion` `"3"`, no table or data changes. A 4B binary therefore refuses a 4C database (`DB_TOO_NEW`).
   - Stored v1 diffs are upgraded on read (Decision 5).
   - Old `snapshots` rows are upcast on every read and never rewritten. `diffs::fill` only inserts the missing snapshots at multiples of 200 (`diffs.rs:70`).
4. **Per-op diffs:**
   - `Change { kind, id, change, op, edge?, path?, by? }` and `Shift { track, from, by, op }`. `op` is the op's index in its transaction;
   - `by` on a change is the **content offset** of a start-edge move;
   - an op may record any number of shifts and changes, and `split_diff` splits by `op`;
   - **ripple never records shifted items as changes**;
   - entity kinds: `asset, track, timeline, item, transition, marker, bin, effect, link, style, font`. `item` replaces 4B's `clip`.
5. **Upcasters v1 → v2** (frozen):
   - **Record step.** In `intent.ops`, `resolved` and `inverse`:
     - `clip.insert|trim|delete|remove|restore` → `item.*` (ripple or lift; **a missing `ripple` on `clip.restore` means `true`**);
     - `asset.add` gains `kind: "media"`.
   - **Snapshot step:** the v2 model with Decision 9's defaults and an empty `fonts` registry.
   - **Diff step:** a stored diff is v1-shaped when no entry has `op` (an empty diff reads the same either way; a mix is `INVALID_FORMAT`).
     - 4B's v1 ops each recorded exactly one change, and each clip op exactly one shift, so entries get op indexes in order.
     - A `clip.trim` change gains `edge`, and on the start edge `by = −delta`; `clip` becomes `item`.
     - Its output must equal 4C's replay of the upcast record (`verify` `diffs_match` on all fixtures).
   - **`new_project(genesis)` equals `upcast_snapshot(<v1 rev-0 JSON>)`** (pinned by a test).
6. **Aliases:**
   - `clip.insert|trim|delete` are accepted from clients for schema version 2 only, and so is a v1 client tx holding only v1 op kinds;
   - `fin_core::normalize` rewrites them at the commit entry (adding `kind: "media"` to a v1 `asset.add`);
   - an alias in `Internal`/`Replay` mode is `INVALID_OP`;
   - dropping the aliases at schema 3 is a follow-up.
7. **`argsHash` is stable across the upgrade:** new `fin` arguments are left out of the hashed input when absent, and golden tests pin the 4B hashes. A retry across the upgrade is answered from the log. Pins planned by the server (Decision 35) are ops, not arguments, so they never change the hash.

### Model

8. **Layout** (spec §5.1). `Project { id, name, schemaVersion, rate, timelines: Vec<Timeline>, bins, assets, styles, fonts: Fonts }`.
   - **`fonts` is stored, as spec §5.1 originally said.** It reverses this plan's earlier "fonts derived" choice, because only stored pins make every machine measure with the same bytes (Decision 29).
   - `rate` stays: the genesis rate and the default for new timelines.
9. **Rev-0 and upcast defaults:**
   - timeline `tl1` `"Timeline 1"`, the project rate, **1080×1920** (owner), frozen in the upcaster;
   - tracks named by kind in track order (`V1`, `A1`, `C1`…), flags false, gain 0 dB;
   - track order is compositing order, bottom first;
   - `fonts` empty.
10. **Ids: one prefix per item kind** (owner):
    - `c` clip, `cap` caption, `g` graphic, `img` image, `adj` adjustment, `nst` nested;
    - `m` marker, `tr` transition, `b` bin, `fx` effect, `lk` link, `st` style;
    - 4B's `a`, `t`, `tl`, `br` are unchanged.

    Font pins are keyed by face, not by id (Decision 29). Effect ids come from the global counter and are addressed as `(item, effect)`.
11. **Defaults are omitted in JSON.**
12. **Value types:**
    - times are `Rational`;
    - parameter values are finite `f64` (round-tripped exactly through record bytes by `float_roundtrip`), `bool`, `[f64; 2]` or strings;
    - **gains are dB**, so `fin-core` needs no `pow`;
    - plans carry colours as `[f64; 4]`, interpolated per sRGB component; `fin view` prints hex with `floor(c × 255 + 0.5)`.
13. **Item-relative times are content-anchored.** Keyframe `at`, caption word times and a split's `offset` are relative to the item start.
    - A start-edge move by `d` (start trim, a start-edge roll, a slide's neighbour) shifts them by `−d` and records `by = −d`.
    - A split rebases the right part. A slip moves none.
    - Out-of-range values are kept, so inverses stay exact; eval clamps.
14. **Speed:** source time = `sourceIn + (t − start) × speed`. The source extent is policy.

### Edits and rebase

15. **Positions are in the frame before the op** (`Assoc::After`), `item.move`'s `at` included. Lengths are never mapped.
16. **Item-relative values are mapped through content offsets** (chosen over "make them conflict").
    - `StepMap.content` holds `(item, by)` from the steps' changes; `Mapping::map_content` adds them, and an inverse map negates them.
    - Ops expose `offsets_mut()`: the `item.split` `offset` and the keyframe `at`.
    - *Why:* a start-edge move is a pure translation of that item's relative times, so translations compose by addition and an undo cancels its unit exactly. Conflicting instead would refuse common edits and make two keyframe edits conflict with each other. Keyframe identity survives: the trim moved every keyframe of the item by the same `by`.
17. **At most one id allocation per op:**
    - `caption.import` expands in `fin`;
    - an overwrite never splits (`OVERWRITE_SPLITS_ITEM`; `compose::overwrite` splits first);
    - a split of a linked item splits that item only (`compose::split_linked`);
    - `transition.set` creates with `$` and updates with a literal id;
    - font pins allocate no id.
18. **Modes are required:**
    - `item.insert`/`item.move`: `ripple | overwrite`;
    - `item.delete`: `ripple | lift`;
    - `item.trim`: `ripple | roll | overwrite`, and **internal `strict`** (no ripple, no carving, an overlap is refused `CLIP_OVERLAP` in every mode).

    The inverses of overwrite, split and caption merge use `strict`, so an undo meets later edits as a conflict. `fin` defaults to ripple for clips, overwrite (lift for delete) otherwise, and echoes `resolved.mode`.
19. **Link groups:** move, trim and delete act on every member (only the named item changes track); slip, slide and split act on the named item. Members are recorded in the diff.
20. **Footprints:**
    - 4B rules for what an op names, plus `Footprint::with_changes(own_diff)`: `Removed` → removes, `Added` → creates, `edge` → `Edge`, `path` → `Param`, else `Exists`;
    - sub-entities are `"<item>/<fx>"`, `"<track>/<transition>"` and `"font/<key>"`;
    - slip touches `Param("sourceIn")`; split touches `Exists` and creates the right part; slide touches `Param("start")` and its neighbours' edges;
    - `Param` keys: the registry path, `"<path>@<at>"` for keyframes, `"effects"`, `"caption.words"`, `"caption.style"`, `"transition.out"`/`"transition.in"`, and `"sha256"` for `font.repin`.
21. **Linked audio by default** (owner): `fin clip insert` of an A/V asset on a video track sends the video clip, an audio clip item on `--audio-track` (default: the first audio track) and `item.link` (`compose::clip_with_audio`). With no audio track it is `USAGE` with a fix. `--no-audio` opts out.
22. **Transitions** are centred on the cut.
    - Clip handles are policy (`TRANSITION_NO_HANDLES`).
    - A generic post-op pass prunes transitions whose items no longer touch, appending the restore to the op's inverse; it runs in every mode.
    - `TRANSITION_INVALID` is structural.
    - `compose::transition_with_links` adds a `crossfade` between touching link partners (`--no-linked` opts out).
23. **Inverse shift order** (blocker B5): an op's inverse records the op's shifts inverted **in exact reverse order** (`Mapping::set_mirror`, `rebase/map.rs`). It is checked in every golden and in property 1, with a seeded bug in Task 26.
24. **Nested ids** (blocker B6): `Op::ids()`/`refs_mut()` list every id an op carries, nested ones included: restores, the literal adds, `effect.restore`, `style.add`, and a `font.pin` of an asset. `ids_cover_every_id_in_json` checks it per op and in property 1.
25. **Structural checks for replay** (blocker B1):
    - the track check runs after each op on **every track its diff touched**: shift tracks, tracks holding any item a change names, and tracks named by track or transition changes;
    - `TRANSITION_INVALID`;
    - value shapes.

    Everything else is policy (font availability and overflow included).
26. **Markers never ripple** (owner). **Track lock** is policy (`TRACK_LOCKED`).
27. **Effects:** `effect.add` allocates `$fx`. The built-ins are `colorAdjust`, `lut` and `blur`. Params are a sparse map validated by specs (plugin-ready). `fin` resolves easing presets to `bezier`.
28. **Styles:**
    - built-in presets (`preset:<name>`) plus project styles `st<n>`, through `style.add|set|remove`;
    - safe zones are named data with their sources;
    - a style names a font **family, weight and italic**, which resolve to a pin (Decision 29);
    - a font asset a pin uses, or a pin a style uses, is in use (`ASSET_IN_USE`, `FONT_IN_USE`).

### Fonts

29. **Pins in the model.**
    ```rust
    pub struct Fonts { pub pins: BTreeMap<String /* FontKey "<family>|<weight>|<normal|italic>" */, FontPin>,
                       pub fallback: Vec<String> /* keys, in fallback order */ }
    pub struct FontPin { pub family: String, pub weight: u16, pub italic: bool, pub face: u32,
        pub sha256: String, pub size: u64, pub format: FontFormat /* ttf|otf|ttc */,
        pub scripts: Vec<String> /* ISO 15924 coverage */, pub source: FontSource }
    pub enum FontSource { Catalog { entry: String, style: String, catalog: String /* catalog version */ },
                          Asset { asset: String } }
    ```
    - **Ops:**
      - `font.pin {key, pin, fallback: Option<u32>}` adds a pin (and optionally places it in the fallback order). A pin of a key already held **with the same `sha256`** is a no-op (empty diff, empty inverse), so two edits pinning the same face merge cleanly. With another hash it is `FONT_PINNED`: policy on edits, structural on replay, because valid history cannot produce it.
      - `font.unpin {key}`: refused while a style or caption uses the key (`FONT_IN_USE`).
      - `font.repin {key, pin}`: the user changes the file, for example to a newer catalog version.
      - Inverses are exact (`font.unpin` ↔ `font.pin` with its fallback index; `font.repin` with the old pin).
    - **A catalog update never changes an existing project's layout**: pins name exact bytes, and only `font.repin` moves them.
    - Pins are data. Apply never fetches and never reads font bytes; replay needs neither.
30. **The catalog** (`crates/fin-fonts/catalog/catalog.json`, `catalogVersion` such as `"2026.11.1"`), typed by `fin_fonts::FontCatalog` (`JsonSchema`):
    ```json
    { "catalogVersion": "2026.11.1",
      "fallback": ["noto-sans", "noto-sans-arabic", "noto-sans-hebrew", "noto-sans-cjk", "noto-color-emoji"],
      "entries": [{ "id": "noto-sans", "family": "Noto Sans", "licence": { "spdx": "OFL-1.1", "url": "…" },
        "homepage": "…", "scripts": ["Latn", "Cyrl", "Grek"],
        "styles": [{ "id": "regular", "weight": 400, "italic": false,
          "file": { "sha256": "…", "size": 0, "format": "ttf", "faces": [{ "index": 0, "family": "Noto Sans", "weight": 400, "italic": false }] },
          "sources": [{ "url": "https://…/<commit>/…/NotoSans-Regular.ttf" },
                      { "url": "https://…/releases/download/<tag>/…zip", "sha256": "…", "size": 0,
                        "archive": { "format": "zip", "member": "…/Inter-Regular.ttf" } }] }] }] }
    ```
    - **Initial entries:**
      - Inter (static TTFs from the `rsms/inter` release zip, as archive members);
      - Noto Sans, Noto Sans Arabic, Noto Sans Hebrew (static TTFs from `notofonts` at a pinned commit), Regular and Bold;
      - Noto Sans CJK Regular and Bold (`notofonts/noto-cjk` `Sans/OTC/*.ttc`, 19.5 and 20.1 MB; faces SC, TC, JP, KR, HK);
      - Noto Color Emoji (`googlefonts/noto-emoji` `2D/fonts/NotoColorEmoji.ttf`, CBDT, 10.7 MB; the font is OFL-1.1, while the repository's tools and images are Apache-2.0 and are not used).
    - **Extensible:** more entries and styles can be added, catalogs merge by entry id, and a newer `catalogVersion` adds files without touching pins. **Sub-project 8's registry can supply further catalogs** (and signatures); 4C reads only the built-in one, plus `FIN_FONT_CATALOG` in `test-hooks` builds.
31. **The catalog gate** (`scripts/check-font-catalog.mjs`, and the Rust validation in `FontCatalog::validate`, which share one rule list):
    - `licence.spdx` is in the permissive allow list (`OFL-1.1`, `Apache-2.0`, `MIT`, `BSD-3-Clause`, `CC0-1.0`). No NC, ND, share-alike corpus licences, or GPL font exceptions;
    - every URL is `https` and immutable: it contains a 40-hex commit or a `/releases/download/<tag>/` path;
    - `sha256` is 64 hex; `size` is between 1 byte and 64 MiB; `format` is `ttf`, `otf` or `ttc`;
    - ids, style ids and face keys are unique, and every `fallback` id exists;
    - archive members say their archive's `sha256` and `size` too.
32. **The cache** (`fin-fonts::cache`, `<server-data>/fonts/`):
    - **content-addressed** `<sha256>` files;
    - written as `.~fin-<ulid>.part` → SHA-256 verified → renamed **without replacing** (an existing file is verified, then kept);
    - **verified before every use** (SHA-256, memoised by length and mtime);
    - **resumable:** `<sha256>.part` plus `<sha256>.part.json` `{url, received}`, continued with an HTTP `Range` request when the server answers 206, else restarted;
    - **concurrency-safe across processes:** one OS lock (`File::lock`, as `fin-store`'s `lease.lock` uses) on `<sha256>.lock` while fetching. Others wait (bounded), then re-check the final file;
    - **size limits:** the download must match the catalog `size` and is aborted past it (archives: their own size, then the member's);
    - a stray temp file is swept after 24 h. No total-size limit in 4C (`fin fonts gc` is a follow-up).
33. **The fetcher** (`fin-fonts::fetch`):
    - `ureq` with `rustls` + `ring` and the platform verifier;
    - proxies from `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY`/`NO_PROXY`, and the Windows system proxy;
    - connect timeout 10 s, idle read timeout 30 s;
    - per source, 3 attempts with backoff (1 s, 4 s, 16 s), then the next source. Total failure is `FONT_FETCH_FAILED {key, sha256, reason, retryAt}`, retried automatically after at most 10 min, or at once on `fin fonts fetch`;
    - at most 2 downloads at a time per process;
    - **offline-friendly:** `FIN_FONTS_OFFLINE=1` or the finisherd setting `fonts.autoFetch: false` never touches the network; cache hits work either way;
    - plain `http` only for loopback in `test-hooks` builds.
34. **When fetching happens** (owner question 1, default **automatic**):
    - when a project opens or imports, and after a commit, for pins whose bytes are not cached;
    - when a measurement reports missing bytes;
    - on `fonts.plan` (for the files it pins);
    - on `fin fonts fetch`.

    Fonts are server-level, so `fin fonts status` and the `fonts.status` RPC show every file's state (`cached | queued | fetching {bytes, total} | failed {reason, retryAt}`). Project status lists what its pins lack (`fonts {missing, fetching, failed}`) and warns `FONT_MISSING` or `FONT_FETCH_FAILED` with fix text. A change is published as a `status` event on `project.events`; no new event type.
35. **Pins are planned by the server, sent by the client.**
    - `fin`'s `Builder`, when online, calls `fonts.plan {projectId, branch, ops}` before it commits. The server applies the ops to a copy of the head without a measurer and reads `layout::font_needs`:
      - faces = the styles' `(family, weight, italic)`;
      - scripts = `unicode-script` of the caption text.
    - It returns `font.pin` ops for faces with no pin (from the catalog's matching entry style) and for scripts no fallback pin covers (from the catalog `fallback` order). `fin` prepends them to its own tx and echoes `resolved.fonts`.
    - **The server never alters a client's tx** (spec §2.3: intent as submitted).
    - A text whose faces or scripts have no pin (another client that did not plan) is unmeasured, with warning `FONT_UNPINNED` and `fix: fin fonts pin`.
    - Offline `fin` plans no pins (`measured: false`).
36. **Measurement uses pinned bytes; missing bytes mean "unmeasured", never "blocked".**
    - `layout::faces_for(project, style, words)` resolves the request's faces: the style's pin first, then every `fonts.fallback` pin whose `scripts` meet the text's scripts, in fallback order.
    - `TextRequest.faces` carries `FaceRef {sha256, face, family, weight, italic}`. `fin-text` looks the bytes up by SHA-256 through a `FontBytes` provider and answers `Measured::Layout` or `Measured::Unavailable {missing}`.
    - **One shared measurer serves every project and branch.** Determinism comes from the request, which names exact bytes.
    - Unavailable (not fetched, offline, failed, hash mismatch, no pin):
      - overflow is not checked for that item;
      - eval reports `CaptionPlan.layout: null` and `unmeasured: {reason, keys}`;
      - status warns.

      Once the bytes arrive, the next measurement is measured; nothing is re-written.
    - **Nothing measured enters the log:** the overflow check only accepts or refuses an edit; layouts live only in eval plans, computed on demand. *Known consequence (spec §5.6, wider here):* an edit accepted while a font was missing can conflict `CAPTION_OVERFLOW` when a machine that has it later rebases or merges it.
37. **Font assets** (imported files):
    - `asset.add` of kind `font` records `font: FontFile {sha256, size, format, faces: [{index, family, weight, italic, scripts}]}`, from `fin_text::font_info` and a whole-file SHA-256;
    - `fin fonts pin --asset <a>` pins a face with `source: Asset`;
    - the provider reads the asset's file and verifies its SHA-256. A changed file is a hash mismatch: unmeasured, warning `FONT_MISSING` (reason `hashMismatch`).
38. **`fin-text` engine rules:**
    - `cosmic-text` `no_std` (`libm` everywhere); always our own `Fallback`, built from the request's face list (`PlatformFallback` is never constructed); locale `"en-US"`;
    - **one process-wide name interner** (each distinct family name leaked once);
    - `FontSystem`s cached by the request's face list (LRU, 8);
    - output in 1/64 px at the timeline resolution;
    - `fingerprint()` = the `fin-text`, `cosmic-text`, `harfrust` and `skrifa` versions (from `Cargo.lock` via `build.rs`). A plan's caption layout also lists its face hashes.
    - `fin_text::font_info(bytes)` is the one source of family, weight, italic and script coverage, used by `fin-probe` and catalog verification.
39. **Test fonts:**
    - unit and integration tests use fonts **generated at test time** by `fin-testfonts` (`write-fonts`). They are deterministic (fixed timestamps), with chosen cmaps, advances and coverage (`TestSans` Latin+Cyrillic, `TestRtl` Hebrew letters, `TestHan` a few Han code points, `TestBig` for size limits), and need no licence or network;
    - the test-only CJK subset font of the earlier plan is dropped;
    - **real-font shaping** (Arabic joining, bidi, kerning, CJK, emoji) is pinned by a corpus over the warmed real catalog files (`FIN_FONT_CACHE`), required in CI and on the Task 29 hosts;
    - the `pnpm fonts:fetch` script is gone; `warm_cache` is only a CI cache warmer.

### Evaluation, interfaces and CLI

40. **Eval runs on the server.** `fin eval frame|audio` needs `finisherd`.
41. **Plans:**
    - layers come bottom-up; a nested item is a group layer; an adjustment layer carries effects only; a transition yields A (`out`) then B (`in`) with `progress`;
    - a muted video or caption track is hidden; only audio tracks produce audio; `solo` silences the rest;
    - `AudioPlan` gain segments carry dB ramps with `interp` and fade or crossfade `shapes`.
42. **Bézier:** CSS `cubic-bezier`, solved by 8 Newton steps (accepted at `|x − u| < 1e-7` with `|x'| > 1e-6`), else 32 bisection steps.
43. **RPCs.** No per-edit RPCs: edits go through `tx.commit`. New: `eval.frame`, `eval.audio`, `fonts.catalog`, `fonts.status`, `fonts.fetch`, `fonts.plan`.
44. **`fin` commands:**
    - the clip, item, image, graphic, transition, track, timeline, marker, bin, asset, param, keyframe, effect, caption, style, schema and eval groups;
    - **`fin fonts ls [--catalog | --cached | --project]`, `fonts fetch <key | entry | --pinned>`, `fonts status`, `fonts pin <family> [--weight] [--italic] [--asset <a>] [--fallback <n>]`, `fonts repin <key> [--catalog-version <v>]`, `fonts unpin <key>`**;
    - the map lists command names per group and stays under 1.5k tokens;
    - multi-op behaviour comes from `fin_core::compose`;
    - no `fin batch` changes.
45. **New codes** (exit 3 unless noted):
    - `OVERWRITE_SPLITS_ITEM`, `NOT_ADJACENT`, `TRANSITION_NO_HANDLES`, `TRANSITION_EXISTS`, `TRANSITION_INVALID`, `TRACK_LOCKED`;
    - `TRACK_NOT_EMPTY`, `TIMELINE_NOT_EMPTY`, `TIMELINE_NESTED`, `BIN_NOT_EMPTY`, `BIN_CYCLE`, `ALREADY_LINKED`, `SLIDE_BLOCKED`, `MOVE_INTO_ITSELF`;
    - `PARAM_UNKNOWN`, `PARAM_TYPE`, `PARAM_NOT_ANIMATABLE`, `PARAM_ANIMATED`, `SLOT_UNKNOWN`, `STYLE_IN_USE`, `CAPTION_OVERFLOW`, `SRT_INVALID`;
    - `FONT_PINNED`, `FONT_IN_USE`, `FONT_NOT_IN_CATALOG`; `FONT_FETCH_FAILED` (exit 5 as a `fin fonts fetch` error, otherwise a warning); `NESTED_CYCLE` (eval only).

    Warnings: `FONT_MISSING`, `FONT_FETCH_FAILED`, `FONT_UNPINNED`. `INVALID_RANGE` gains `{path, min, max, unit}`, `ASSET_IN_USE` gains `items`, and `INSERT_INSIDE_CLIP` keeps its name and gains `details.item`.
46. **`fin view`:** screenplay JSON `{rev, timeline, lines, elided, tokens}`, with the token estimate `ceil(chars / 4)` and fixed degradation steps.
47. **Upgrade notes** (README and spec):
    - 4C turns 4B followers read-only at its first edit;
    - a 4C-created project opens read-only on a patched 4B, or not at all on an unpatched one;
    - a 4C DB is refused by 4B;
    - old projects pin no fonts until their first caption edit.
48. **Last-batch kinds:** adjustment and blend stay in 4C. **The nested-insert and mask edits move to sub-project 6** (the model, registry paths, eval and guards stay). It is a TASKS follow-up.

## Questions for the owner (answered 2026-11-06)

Both defaults accepted by the owner: automatic fetching with the off switch, and upstream sources at pinned commits/tags with optional mirrors.

1. **May `finisherd` fetch fonts without being asked?** Default **yes** (Decision 34): it fetches a project's pinned or planned fonts as soon as they are needed, so captions measure without a manual step. Off switch: `fonts.autoFetch: false` or `FIN_FONTS_OFFLINE=1`, after which only `fin fonts fetch` downloads. The alternative is fetch-only-on-command, or a first-use prompt in the UI later.
2. **Catalog sources.** Default: the upstream GitHub repositories at pinned commits and release tags (`notofonts`, `googlefonts/noto-emoji`, `rsms/inter` release zips), each entry allowing further mirror URLs. The alternative is a Finisher-hosted mirror from the start (it needs hosting; sub-project 8's registry is the natural home). This choice decides availability and who is contacted on first use.

Earlier answers are recorded in Decisions 9, 10, 21 and 26. The bundling answer is superseded by Decisions 29–39.

## Review Focus

1. **The upgrade path:**
   - fixtures open with states equal to hand-checked v2 goldens, and their upcast v1 diffs equal a fresh replay;
   - `new_project` equals the upcast genesis;
   - undo, merge and pick of pre-4C records work;
   - a patched 4B opens a 4C project read-only;
   - → Tasks 3, 6, 7; Task 29 mixed-version row.
2. **Footprints from the op's own diff.** → Tasks 8–10.
3. **Content offsets** (B3). → Tasks 8, 14, property 3.
4. **Exact inverses** in reverse shift order (B5); `strict` refuses overlap (B4). → Task 26, Task 8.
5. **Replay's track check** covers no-shift ops (B1). → Task 7, property 6.
6. **Numbers round-trip** (B2). → Task 6.
7. **Nested ids** (B6). → Tasks 1, 7, 12, 14, 18.
8. **Fonts:**
   - pins make layouts identical on machines with the same bytes;
   - a catalog update changes nothing pinned;
   - a missing, failing or mismatched font never blocks an edit and never puts anything into the log;
   - concurrent and resumed fetches are safe;
   - the licence gate holds;
   - → Tasks 5, 18, 20, 22: `same_pins_same_layout_across_providers`, `catalog_update_keeps_pinned_layout`, `missing_font_never_blocks_and_logs_nothing`, `concurrent_fetch_one_download`, `resume_after_partial`, `catalog_gate_refuses_nc_licence`.
9. **Determinism** across OSes and wasm32, with generated and real fonts. → Tasks 4, 17, 29.
10. **Measurer threading.** Replay never calls it; edits re-check overflow once per tx. → Task 20.
11. **Frozen code.** → Tasks 3, 6, 7.

---

### Task 1: Make room (pure refactor) — tier: sonnet

**Files:**
- `crates/fin-core/src/model.rs` → `model/mod.rs` (no change).
- `crates/fin-core/src/ops/mod.rs`:
  - move the `Op` enum and `Edge` into `ops/defs.rs`;
  - move each op's arm of `name`, `is_internal`, `refs_mut`, `allocates` and `records_shift` into its own module (`pub(crate) const NAME`, `INTERNAL`, `fn refs_mut`, `fn allocates`);
  - dispatch through one `ops_table!` macro listing `Variant => module` once, generating `name`, `is_internal`, `refs_mut`, `ids`, `allocates`, `positions_mut`, `apply` and `footprint`.
- `crates/fin-core/src/ops/util.rs` → `ops/util/{mod.rs, lookup.rs, ripple.rs, check.rs}`.
- `crates/fin/src/commands/mod.rs` → move `Builder` into `commands/builder.rs`. `Builder::push` substitutes working ids through `Op::refs_mut()` instead of per-op matches.
- Create: `crates/fin-core/tests/common/ids_json.rs` (`ids_cover_every_id_in_json`, Decision 24) and use it in `ops_table_lists_every_variant`.

**Interfaces:** none new. All items keep their names and visibility.

- [x] **Step 1:** Move the code with no logic changes. Doc comments move with their items. Add:
  - `ops_table_lists_every_variant` (every variant's `name()` equals its serde tag, round-tripped through JSON, and passes `ids_cover_every_id_in_json`);
  - `ids_json_helper_flags_an_unlisted_id` (a hand-made op JSON with an extra `"c9"` in an id field fails the helper).
- [x] **Step 2:** `rtk wc -l` the touched files: each must be ≤ 450.
- [x] **Step 3: Run** `rtk cargo test --workspace`, `rtk pnpm test`, `rtk pnpm gates`. Expected: PASS with the same test count plus two (record it in the commit body).
- [x] **Step 4: Commit** `refactor(core): one op table, split ops and model modules ahead of 4C`.

### Task 2: Scrubbed 4B fixture, generated from 4B code — tier: sonnet

**Files:**
- Create: `crates/fin-store/tests/fixtures/4b/folder/finisher.project.json`, `.../folder/.finisher/segments/{main,br1}/*.fseg`, `crates/fin-store/tests/fixtures/4b/project.fdb`, `.../4b/expected.json` (`{branches: {main: {rev, project}, br1: {rev, project}}, stacks, argsHashes}`), `.../4b/README.md`.
- Modify: `crates/fin-store/tests/integration_support/mod.rs` (`fixture_4b`). Create: `crates/fin-store/tests/fixture_4b.rs`.
- The generator lives in a 4B worktree under `archive/`, never committed.

- [ ] **Step 1: Generate from 4B code.**
  - `rtk git worktree add archive/wt-4b 36fe7e0`.
  - Write `crates/fin-store/examples/make_4b_fixture.rs`:
    1. create a project; `asset.add`;
    2. on `main`, 4 inserts on `t1`/`t2`, 2 trims (one per edge), 1 delete;
    3. `branch new feature` with 2 edits; checkpoint `v1`;
    4. one more `main` edit; `merge feature`; `undo`, `redo`; a cherry-pick from `feature`;
    5. `tick` until exported; `close`; `PRAGMA wal_checkpoint(TRUNCATE)`.
  - Record the `argsHash` 4B's `fin` computes for three fixed `clip insert|trim|delete` inputs (Decision 7).
- [ ] **Step 2: Scrub** as 4B Task 2 did:
  - copy only `finisher.project.json`, the `.fseg` files and `project.fdb`;
  - `DELETE FROM meta WHERE key IN ('projectDir','serverIds','createKey'); DELETE FROM lease_state; VACUUM;`;
  - write the states, stacks and hashes.
- [ ] **Step 3: Leak check:** `rtk proxy bash -c 'grep -rIl -e "$(hostname)" -e "$USER" crates/fin-store/tests/fixtures/4b || echo clean'`, plus `strings project.fdb` with the same patterns.
- [ ] **Step 4:** Add `fixture_4b(tmp)` and `fixture_4b_opens_with_relocate` (today's code: the states equal `expected.json`).
- [ ] **Step 5: Run** `rtk cargo test -p fin-store`, `rtk pnpm gates`; `rtk git worktree remove archive/wt-4b`.
- [ ] **Step 6: Commit** `test(fin-store): scrubbed 4B fixture for 4C upgrade tests`.

### Task 3: Newer project files read-only; per-op diffs; the v1 diff upcaster; migration 0003 — tier: opus

**Files:**
- **Commit A** (cherry-pickable onto `sp4b-history`): `crates/fin-store/src/project_file.rs` (`is_newer()` counts `schemaVersion > SCHEMA_VERSION`; `read_project_file_once` stops refusing it), tests, `crates/fin-store/tests/store.rs`.
- **Commit B:**
  - `crates/fin-core/src/{tx.rs (Change, Shift, EntityKind), apply.rs}`, `ops/util/ripple.rs` (`OpDiff`), every op module;
  - `rebase/{footprint.rs (split_diff, with_changes), steps.rs}`;
  - create `crates/fin-core/src/upgrade/diff_v1.rs` (`upgrade.rs` → `upgrade/mod.rs`);
  - create `crates/fin-store/src/migrations/m0003_version.rs`; modify `migrations/mod.rs` (`DB_VERSION = 3`, `MIN_READER_VERSION = 3`), `db/mod.rs`;
  - one helper `db::records::stored_diff(record_value, diff_json)` used by every reader of `txs.diff`: `db/records.rs` `decode`, `history_ops/mod.rs` (the `Diff` parse near line 210) and `verify/replay.rs` (the `diffs_match` compare);
  - the RPC fixtures under `crates/fin/tests/` and the TS tests that carry diffs.
- Test: `crates/fin-core/tests/{diff_split.rs, upgrade_golden.rs}`, `crates/fin-store/tests/{migrations.rs, fixture_4b.rs}`.

**Interfaces:**
```rust
pub struct Change { pub kind: EntityKind, pub id: String, pub change: ChangeKind, pub op: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub edge: Option<Edge>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub by: Option<Rational> }
pub struct Shift { pub track: String, pub from: Rational, pub by: Rational, pub op: u32 }
pub enum EntityKind { Asset, Track, Timeline, Item, Transition, Marker, Bin, Effect, Link, Style, Font }
pub(crate) struct OpDiff<'a> { diff: &'a mut Diff, op: u32 } // record, record_edge(.., by), record_path, shift
pub fn split_diff(ops: &[Op], diff: &Diff) -> Result<Vec<Diff>, CoreError>;
impl Footprint { pub(crate) fn with_changes(self, own: &Diff) -> Footprint; }
pub fn upcast_diff(diff: Value, v1_ops: &[Value]) -> Result<Diff, CoreError>; // Decision 5, frozen
```

- [x] **Step 1: Commit A.**
  - Tests: `newer_schema_version_is_newer`, `newer_project_file_opens_read_only`, `older_schema_version_still_opens`.
  - Commit `fix(store): a project file at a newer schemaVersion opens read-only`.
  - Check that it cherry-picks cleanly onto a scratch checkout of `sp4b-history` with passing `rtk cargo test -p fin-store`, then discard the checkout. Report the hash to the coordinator.
- [x] **Step 2: Failing tests for commit B:**
  - `split_diff_by_op_index`, `split_diff_rejects_out_of_range_or_unordered_op`, `ops_record_their_own_index` (the start trim's change has `edge` and `by`), `with_changes_maps_removed_added_edge_path`;
  - `upcast_v1_diff_golden`, `mixed_diff_is_invalid_format`, `empty_diff_reads_the_same`, `diff_upcast_equals_replay_on_fixtures` (SP2, 4A, 4B; `verify` ok);
  - `migration_0003_is_frozen`, `fresh_and_migrated_schemas_match`, `db_v3_is_refused_by_a_v2_reader`;
  - all 4B rebase and history tests unchanged.
- [x] **Step 3: Run** `rtk cargo test -p fin-core --test diff_split --test upgrade_golden` and `rtk cargo test -p fin-store --test migrations --test fixture_4b` → FAIL.
- [x] **Step 4: Implement.** m0003 runs `UPDATE meta SET value = '3' WHERE key IN ('formatVersion', 'minReaderVersion');` (the runner sets `user_version`). `stored_diff` passes the raw v1 `intent.ops` to `upcast_diff`.
- [x] **Step 5:** Regenerate types; update the fixtures and TS tests.
- [x] **Step 6: Run** the three gates → PASS. **Commit** `feat(core,store): per-op diffs; v1 diffs upcast on read; DB v3`.

### Task 4: The `TextMeasurer` contract and `fin-text` (pure, bytes by hash); generated test fonts — tier: opus

The riskiest unknown goes first: deterministic shaping across OSes. `fin-text` is written fresh (the S11b spike code is not available).

**Files:**
- Create: `crates/fin-core/src/layout.rs` (the contract). Modify `crates/fin-core/src/lib.rs`.
- Create: `crates/fin-testfonts/{Cargo.toml, src/lib.rs}` (`publish = false`; dev-dependency of the crates below).
- Create: `crates/fin-text/{Cargo.toml, build.rs}`, `crates/fin-text/src/{lib.rs, system.rs (FontSystem from a face list), fallback.rs, intern.rs, info.rs, measure.rs}`.
- Create: `crates/fin-text/tests/{measure.rs (generated fonts), corpus_real.rs (warmed real fonts)}`, `crates/fin-text/tests/corpus/{generated.json, real.json, expected.json, layouts/*.json}`.
- Modify: `scripts/gates.mjs` (`cargo build -p fin-text --target wasm32-unknown-unknown`), `THIRD_PARTY_NOTICES.md`.

**Interfaces:**
```rust
// fin_core::layout (re-exported)
pub struct TextStyle { pub family: String, pub weight: u16, pub italic: bool, pub size: f64, pub line_height: f64,
    pub letter_spacing: f64, pub align: Align }
pub struct FaceRef { pub sha256: String, pub face: u32, pub family: String, pub weight: u16, pub italic: bool }
pub struct TextRequest { pub words: Vec<String>, pub style: TextStyle,
    pub faces: Vec<FaceRef> /* primary first, then fallbacks in order */, pub max_width: Option<f64>, pub max_lines: Option<u32> }
pub struct TextLayout { pub lines: Vec<LineBox>, pub words: Vec<WordBox>, pub width: i32, pub height: i32,
    pub overflow: bool, pub missing: Vec<u32> /* code points no face has */, pub faces: Vec<String> /* sha256s used */ }
pub enum Measured { Layout(TextLayout), Unavailable { missing: Vec<String> /* sha256s without bytes */ } }
pub trait TextMeasurer: Send + Sync {
    fn measure(&self, req: &TextRequest) -> Result<Measured, CoreError>;
    fn fingerprint(&self) -> String; // Decision 38
}
// fin_text
pub trait FontBytes: Send + Sync { fn get(&self, sha256: &str) -> Option<std::sync::Arc<[u8]>>; }
pub struct FaceInfo { pub index: u32, pub family: String, pub weight: u16, pub italic: bool, pub scripts: Vec<String> }
pub fn font_info(bytes: &[u8]) -> Result<Vec<FaceInfo>, CoreError>;
pub struct Measurer { /* provider + LRU of FontSystems keyed by face list */ }
impl Measurer { pub fn new(bytes: std::sync::Arc<dyn FontBytes>) -> Measurer; }
impl fin_core::layout::TextMeasurer for Measurer {}
// fin_testfonts
pub struct Spec { pub family: &'static str, pub weight: u16, pub italic: bool, pub chars: &'static str, pub advance: fn(char) -> u16 }
pub fn build(spec: &Spec) -> Vec<u8>;            // deterministic TTF (glyf, cmap, hmtx, name, OS/2, …)
pub fn test_sans() -> Vec<u8>; pub fn test_rtl() -> Vec<u8>; pub fn test_han() -> Vec<u8>; pub fn test_big(size: usize) -> Vec<u8>;
```

- [x] **Step 1: Licences.** Add `cosmic-text = { version = "0.19", default-features = false, features = ["no_std"] }` and `write-fonts` (dev). Run `rtk cargo deny check licenses`, and record the transitive licences in the commit body.
- [x] **Step 2: Write failing tests:**
  - `measure.rs` (generated fonts, always run):
    - `same_bytes_same_layout` (two `Measurer`s, two providers);
    - `fallback_follows_face_order` (`TestSans` then `TestHan`; swapping the order changes which face is used);
    - `missing_bytes_are_unavailable` (lists the sha256);
    - `rtl_run_is_rtl`, `wrap_and_overflow`, `letter_spacing_and_alignment`;
    - `platform_fallback_never_used`, `interner_leaks_each_name_once`, `fingerprint_names_versions`;
    - `font_info_reads_faces_and_scripts` (TTC too, built from two generated faces).
  - `corpus_real.rs` (warmed real fonts, by sha256; skips with a notice unless `FIN_FONT_CACHE` is set; fails when `FIN_REQUIRE_FONTS=1` and a file is missing):
    - Latin kerning, Cyrillic, Greek;
    - Arabic joining, Hebrew, mixed bidi;
    - Simplified Chinese, Japanese kana, Korean, emoji;
    - an unassigned code point (`missing`);
    - Bold; wrapping; `max_lines`.

  Hashes go in `expected.json`; the layouts in `layouts/` are for review.
- [x] **Step 3: Run** `rtk cargo test -p fin-text --test measure` → FAIL.
- [x] **Step 4: Implement.**
  - `FontSystem::new_with_locale_and_db_and_fallback("en-US", db, FaceListFallback)`, with the faces loaded in request order into an empty `fontdb::Database`;
  - `Buffer` with `Shaping::Advanced` and `Wrap::Word`; `libm::roundf` to 1/64 px;
  - files < 400 lines.
- [x] **Step 5: Goldens.** *(Generated goldens done in Task 4; the real corpus with Task 5: `real.json` pins the catalog files (`noto-sans-cjk:regular` is face 2, SC, of `NotoSansCJK-Regular.ttc`), the 14 real hashes are blessed from a `warm_cache` dir on Linux and the layouts reviewed by eye: RTL word order for Arabic and Hebrew, `123` before the Arabic word in the LTR mixed line (UBA W2), CJK at exactly 1 em per ideograph and kana, emoji at the CBDT advance (1.245 em), U+0378 missing, wrap lines ≤ 600 px.)* Once Task 5's `warm_cache` exists (or by a one-off download into `archive/fonts/` checked against the catalog hashes), fill `expected.json` for the real corpus and review `layouts/` by eye.
- [x] **Step 6: Early cross-OS signal** (controller; generated corpus byte-identical on Linux, Windows 11, macOS arm64 and wasm32-wasip1; real fonts with Task 5): run both test files on the macOS and Windows hosts with a warmed cache, and on wasm32-wasip1 locally (`wasmtime --dir`). Record the results in `archive/sp4c-text/`. Task 29 repeats it.
- [x] **Step 7: Run** the three gates → PASS. **Commit** `feat(text): fin-text measurement over fonts by hash; TextMeasurer contract; generated test fonts`.

### Task 5: The font catalog, the cache and the fetcher (`fin-fonts`) — tier: opus

**Files:**
- Create: `crates/fin-fonts/{Cargo.toml, catalog/catalog.json}`, `crates/fin-fonts/src/{lib.rs, catalog.rs (FontCatalog, validate), cache.rs, fetch.rs, http.rs (ureq agent: TLS, proxies, timeouts), archive.rs, lock.rs}`.
- Create: `crates/fin-fonts/examples/warm_cache.rs` (fetch every catalog file into a dir: the CI cache warmer, and the catalog check that compares `font_info` faces and scripts with the catalog's), `crates/fin-fonts/tests/{catalog.rs, cache.rs, fetch.rs, support/http_server.rs}`.
- Create: `scripts/check-font-catalog.mjs`, `scripts/check-font-catalog.test.mjs`. Modify: `scripts/gates.mjs`, `deny.toml` (`[graph] targets`), `.github/workflows/ci.yml` (cache the warmed dir keyed by the catalog's hash with a SHA-pinned `actions/cache`, run `warm_cache` on a miss, set `FIN_FONT_CACHE` and `FIN_REQUIRE_FONTS=1`).

**Interfaces:**
```rust
pub struct FontCatalog { pub catalog_version: String, pub fallback: Vec<String>, pub entries: Vec<CatalogEntry> }
impl FontCatalog {
    pub fn builtin() -> &'static FontCatalog;              // include_str!("../catalog/catalog.json")
    pub fn from_json(s: &str) -> Result<FontCatalog, CoreError>; // + validate (Decision 31)
    pub fn find_face(&self, family: &str, weight: u16, italic: bool) -> Option<(&CatalogEntry, &CatalogStyle, &CatalogFace)>;
    pub fn by_sha(&self, sha256: &str) -> Option<(&CatalogEntry, &CatalogStyle)>;
    pub fn fallback_for(&self, script: &str) -> Option<(&CatalogEntry, &CatalogStyle)>;
}
pub struct FontCache { /* <server-data>/fonts */ }
impl FontCache {
    pub fn open(dir: &Path) -> Result<FontCache, CoreError>;
    pub fn get(&self, sha256: &str) -> Option<Arc<[u8]>>;   // verified (Decision 32)
    pub fn has(&self, sha256: &str) -> bool;
}
impl fin_text::FontBytes for FontCache {}
pub struct Fetcher { /* agent, queue (2 workers), backoff state */ }
pub struct FetchOptions { pub offline: bool, pub allow_loopback_http: bool /* test-hooks only */ }
pub enum FileState { Cached, Queued, Fetching { bytes: u64, total: u64 }, Failed { reason: String, retry_at_ms: i64 } }
impl Fetcher {
    pub fn new(cache: Arc<FontCache>, catalog: Arc<FontCatalog>, opts: FetchOptions) -> Fetcher;
    pub fn request(&self, sha256: &str);                    // enqueue unless cached, running or backing off
    pub fn fetch_now(&self, sha256: &str) -> Result<(), CoreError>; // blocking (fin fonts fetch); FONT_FETCH_FAILED
    pub fn state(&self, sha256: &str) -> FileState;
    pub fn subscribe(&self) -> std::sync::mpsc::Receiver<(String, FileState)>;
}
```

- [x] **Step 1: Licences.** Add `ureq`, `rustls` (`ring`), `rustls-platform-verifier`, `sha2` and `zip` as the Tech Stack says. Set `deny.toml` `[graph] targets`, then run `rtk cargo deny check licenses sources bans`; record the tree's licences in the commit body.
- [x] **Step 2: Write the catalog.**
  - Pin every initial file (Decision 30) at a commit or release tag, and fill `sha256`, `size`, `format`, faces and scripts from a one-off `warm_cache` run.
  - For Inter, the source is the release zip, and the archive member's own `sha256` and `size` are recorded.
  - Confirm each licence text by hand (OFL-1.1) and record the URL.
- [x] **Step 3: Write failing tests** (all against `support/http_server.rs`: an in-process HTTP/1.1 server on loopback that serves generated fonts and zips, with injectable faults: wrong bytes, a truncated body then a dropped connection, `Range` support on or off, HTTP 500, slow bodies):
  - catalog:
    - `builtin_catalog_validates`;
    - `catalog_gate_refuses_nc_licence` (and GPL font exception, mutable URL, non-https, duplicate keys, oversize, unknown fallback id). The same fixtures run through the JS gate test;
    - `find_face_and_fallback_for_script`;
  - cache:
    - `write_is_temp_then_rename`, `existing_file_is_verified_and_kept`, `tampered_cache_file_is_rejected` (`get` → `None`; the state is `Failed {hashMismatch}`);
    - `stray_temp_swept_after_24h` (with a fake clock);
  - fetch:
    - `fetch_verifies_hash` (`hash_mismatch_never_lands`);
    - `resume_after_partial` (a `206` continues from `received`; without `Range` it restarts);
    - `size_limit_aborts`;
    - `retries_with_backoff_then_next_source`, `fetch_failed_state_and_retry_at`;
    - `concurrent_fetch_one_download` (two `Fetcher`s in two threads and a child process share the dir: one download, both see `Cached`);
    - `cache_reuse_no_network`;
    - `offline_never_connects`;
    - `archive_member_extracted_and_verified`;
    - `proxy_env_is_used` (the test server acting as an HTTP `CONNECT` proxy records the request);
    - `https_required_outside_test_hooks`.
- [x] **Step 4: Run** `rtk cargo test -p fin-fonts` → FAIL. **Implement** (files < 400 lines; the HTTP code stays in `http.rs`).
- [x] **Step 5: CI.** Run `warm_cache` once locally into `archive/fonts/`, and check that it reports every face and script equal to the catalog.
- [x] **Step 6: Run** the three gates → PASS. **Commit** `feat(fonts): pinned font catalog, content-addressed cache and fetcher`.

**As built** (for Tasks 6, 18, 20 and 22):
- Catalog `2026.11.1`, 11 styles in 10 files, all OFL-1.1: Inter 4.1 Regular/Bold (one variable file, `docs/font-files/InterVariable.ttf` at tag `v4.1` = `e3a3d4c`, 879,708 bytes, `wght` 100–900; coordinator decision, replacing the 33.7 MB release-zip members); Noto Sans, Arabic and Hebrew Regular/Bold (unhinted TTFs, `notofonts/notofonts.github.io` at `86ce02c`); Noto Sans CJK Regular/Bold OTCs (`notofonts/noto-cjk` at `f8d1575`; 10 faces each, JP KR SC TC HK and the Mono five; `defaultFace` 2 = SC); Noto Color Emoji (`googlefonts/noto-emoji` at `99701e0`). Each GitHub-repository file has a jsDelivr mirror at the same commit (same bytes), tried only after the upstream fails. jsDelivr answers `Range` over another representation, so a `206` whose `Content-Range` total is not the catalog size restarts the download.
- Faces carry `scripts` (exactly `font_info`'s, which `warm_cache` checks); the entry's `scripts` are what `fallback_for` matches; `CatalogStyle.default_face` names the face a fallback or an entry-level pin uses. `FontFormat` is defined in `fin-fonts` for now: Task 6 moves it to `fin_core::model` and `fin-fonts` re-uses it.
- `FileState` gains `Missing` (not cached, not queued). `FetchOptions` also holds `proxy_from_env`, `backoff` (its length is the attempts per source), `retry_after` and `lock_wait`; `FontCatalog::active()` reads `FIN_FONT_CATALOG` in `test-hooks` builds. **`offline` stops background fetching only; `fetch_now` (`fin fonts fetch`) still downloads** (owner answer 1); Task 22 maps both `FIN_FONTS_OFFLINE=1` and `fonts.autoFetch: false` to it.
- Review follow-ups: source URLs must also be on `rules.json` `sourceHosts` (`github.com`, `raw.githubusercontent.com`, `cdn.jsdelivr.net`; rule `sourceHost`); a download slot is taken only after the file's (and archive's) lock, so waiting never holds one; a sibling archive member re-checks the cache after taking the archive's lock (one zip download for Inter Regular + Bold, also across processes); `Fetcher::set_offline` changes the switch at run time; TLS, certificate and redirect errors fail over to the next source at once; the cache never reads a file over 64 MiB and its verify memo includes the file identity (device + inode; creation time on Windows). Proxies: `ALL_PROXY` wins over `HTTPS_PROXY`; the Windows system proxy is used only as one `host:port` (a per-protocol `ProxyServer`, PAC and WPAD go direct). `scripts/check-wasm-deps.mjs` fails if fin-core or fin-text pull a package on wasm32 that no shipped triple has (cargo-deny never sees wasm32).
- Variable files (coordinator decision on Inter): `CatalogFile.variable {wght: [min, max]}`; one file may serve several styles only when it is variable, declared identically in each, and its range covers each style's weight (rule `variable`, both gate halves, 4 shared cases); `warm_cache` reads the `fvar` `wght` axis and compares. The face is `font_info`'s default instance (`Inter Variable`, 400); the request's weight drives the axis, so a pin of Inter Bold names face 0 with weight 700 (the real corpus' `inter-bold` is wider than `latin-kerning` word by word; Regular's geometry equals the static TTF's). `find_face` matches a face's family (a variable face by its style's weight), else the entry's family (`"Inter"` → `Inter Variable`; `"Noto Sans CJK"` → SC). Archive sources stay supported and tested; no built-in entry uses one now. Both Inter sources (raw.githubusercontent and jsDelivr at `e3a3d4c`) serve identical bytes, and jsDelivr's `Range` is correct for this file.
- `FONT_FETCH_FAILED.details`: `{key (the default face's FontKey), entry, style, sha256, reason, retryAt}`, fix text names `HTTPS_PROXY` and `fin fonts fetch <entry>`; reasons `hashMismatch | sizeMismatch | tooLarge | truncated | badRange | http<status> | network | tls | redirect | badUrl | insecureUrl | archive | lockTimeout | io`. New codes in `output.rs`: `FONT_NOT_IN_CATALOG`, `FONT_CATALOG_INVALID` (exit 3), `FONT_FETCH_FAILED` (exit 5).
- `FontCatalog` and `FileState` derive `JsonSchema` but are not yet exported to TypeScript: Task 22 adds them to `export_schema.rs` with the `fonts.*` RPCs.

### Task 6: Model v2 (with the `fonts` registry), the snapshot upcaster, new id kinds — tier: opus

The existing 7 ops are ported onto the v2 model unchanged in meaning (`clip.*` still apply, now to `Item` of kind `clip` on any timeline). Task 7 renames them.

**Files:**
- Create: `crates/fin-core/src/model/{timeline.rs, item.rs, asset.rs, caption.rs, fonts.rs}`. Modify: `model/mod.rs` (`Project`, `new_project`, `SCHEMA_VERSION = 2`).
- Create: `crates/fin-core/src/upgrade/snapshot_v1.rs` (frozen step) and `upgrade/record_v1.rs` (an identity step for now; Task 7 fills it).
- Modify:
  - `crates/fin-core/Cargo.toml` (`serde_json` `float_roundtrip`);
  - `crates/fin-core/src/ids.rs` (Decision 10 kinds; `MemIds::after` walks every entity);
  - `crates/fin-core/src/ops/util/lookup.rs` (item, track and timeline lookup across timelines; `exists` for every kind);
  - every op module (now on `Item`), `crates/fin-core/src/time.rs` (`Rational::to_f64`).
- Modify the readers of `project.timeline`:
  - `crates/fin-store/src/{counters.rs (+ kind names), recover/remap.rs, branch/lineage_tests.rs}`;
  - `crates/fin/src/{addr.rs, commands/view.rs}`;
  - `crates/fin-store-xhost/src/{history.rs, ops.rs, hashes.rs}`;
  - `scripts/e2e/history-lib.mjs`, `scripts/e2e.test.mjs`, `apps/finisherd/test/server.test.ts`, `packages/fin-node/test/{engine,history}.test.ts`.
- Test: `crates/fin-core/tests/model.rs`, `crates/fin-core/tests/upgrade_golden.rs` with `tests/upgrade_golden/{snapshot_v1.json, snapshot_v2.json}`, `crates/fin-core/tests/record.rs`, and `crates/fin-store/tests/{migrations.rs, fixture_4a.rs, fixture_4b.rs}`.
- Create: `crates/fin-store/tests/fixtures/{sp2,4a,4b}/expected_v2.json` (generated once, reviewed by hand, then frozen).

**Interfaces** (`fin_core::model`; `#[serde(rename_all = "camelCase")]`; defaults omitted per Decision 11):
```rust
pub const SCHEMA_VERSION: u32 = 2;
pub struct Project { pub id: String, pub name: String, pub schema_version: u32, pub rate: Rational,
    pub timelines: Vec<Timeline>, pub bins: BTreeMap<String, Bin>, pub assets: BTreeMap<String, Asset>,
    pub styles: BTreeMap<String, CaptionStyle> /* Task 18 fills the struct; empty until then */,
    pub fonts: Fonts /* Decision 29; empty until a caption edit pins */ }
pub struct Timeline { pub id: String, pub name: String, pub rate: Rational, pub resolution: Resolution,
    pub markers: Vec<Marker>, pub tracks: Vec<Track> }
pub struct Resolution { pub width: u32, pub height: u32 }
pub struct Marker { pub id: String, pub at: Rational, pub duration: Option<Rational>, pub label: String, pub color: Option<String> }
pub enum TrackKind { Video, Audio, Caption }
pub struct Track { pub id: String, pub kind: TrackKind, pub name: String, pub mute: bool, pub solo: bool,
    pub lock: bool, pub gain: f64 /* dB */, pub items: Vec<Item>, pub transitions: Vec<Transition> }
pub struct Transition { pub id: String, pub between: [String; 2], pub kind: TransitionKind /* crossfade|dissolve */,
    pub duration: Rational, pub curve: Curve /* linear (default) | equalPower */ }
pub struct Item { pub id: String, pub start: Rational, pub duration: Rational,
    #[serde(flatten)] pub body: ItemBody, #[serde(flatten)] pub common: Common }
#[serde(tag = "kind")]
pub enum ItemBody {
    Clip { asset: String, source_in: Rational, speed: Rational /* default 1 */ },
    Caption(CaptionBlock),
    Graphic { asset: String, slots: BTreeMap<String, ParamValue> },
    Image { asset: String },
    Adjustment {},
    Nested { timeline: String, source_in: Rational },
}
pub struct Common { pub transform: Transform, pub crop: Crop, pub opacity: f64 /* 1 */, pub blend: BlendMode,
    pub effects: Vec<Effect>, pub mask: Option<Mask>, pub audio: Option<AudioParams>,
    pub keyframes: BTreeMap<String, Vec<Keyframe>>, pub link: Option<String> }
pub struct Transform { pub position: [f64; 2] /* px from frame centre */, pub scale: [f64; 2] /* 1,1 */,
    pub rotation: f64 /* degrees */, pub anchor: [f64; 2] /* item-normalised, 0.5,0.5 */ }
pub struct Crop { pub left: f64, pub right: f64, pub top: f64, pub bottom: f64 } // fractions
pub enum BlendMode { Normal, Multiply, Screen, Overlay, Add, Darken, Lighten }
pub struct Effect { pub id: String, pub kind: String, pub enabled: bool /* true */, pub params: BTreeMap<String, ParamValue> }
pub struct Mask { pub matte: String /* artifact ref */, pub feather: f64, pub choke: f64, pub invert: bool }
pub struct AudioParams { pub gain: f64 /* dB */, pub pan: f64, pub fade_in: Rational, pub fade_out: Rational }
pub struct Keyframe { pub at: Rational, pub value: ParamValue, pub interp: Interp }
#[serde(tag = "type")] pub enum Interp { Hold, Linear, Bezier { x1: f64, y1: f64, x2: f64, y2: f64 } }
#[serde(untagged)] pub enum ParamValue { Bool(bool), Number(f64), Vec2([f64; 2]), Text(String) }
pub struct CaptionBlock { pub words: Vec<Word>, pub style: String, pub overrides: BTreeMap<String, ParamValue>,
    pub source: Option<CaptionSource> }
pub struct Word { pub text: String, pub start: Rational, pub end: Rational } // item-relative
pub struct CaptionSource { pub artifact: String, pub word_range: [u32; 2] }
pub struct Bin { pub id: String, pub name: String, pub parent: Option<String> }
pub enum AssetKind { Media, Image, Svg, LottieTemplate, Font, Lut }
pub struct Asset { pub id: String, pub kind: AssetKind, pub path: String, pub fingerprint: String,
    pub bin: Option<String>, pub duration: Option<Rational>, pub video: Option<VideoInfo>, pub audio: Option<AudioInfo>,
    pub image: Option<ImageInfo>, pub lottie: Option<LottieInfo>, pub font: Option<FontFile>, pub lut: Option<LutInfo> }
// ImageInfo {width, height}; LottieInfo {width, height, rate, duration, slots: Vec<SlotSpec{id, kind}>, fonts: Vec<String>};
// LutInfo {size}
pub struct FontFile { pub sha256: String, pub size: u64, pub format: FontFormat /* ttf|otf|ttc */, pub faces: Vec<FontFace> }
pub struct FontFace { pub index: u32, pub family: String, pub weight: u16, pub italic: bool, pub scripts: Vec<String> }
pub struct Fonts { pub pins: BTreeMap<String /* FontKey */, FontPin>, pub fallback: Vec<String> }
pub struct FontPin { pub family: String, pub weight: u16, pub italic: bool, pub face: u32, pub sha256: String,
    pub size: u64, pub format: FontFormat, pub scripts: Vec<String>, pub source: FontSource }
pub enum FontSource { Catalog { entry: String, style: String, catalog: String }, Asset { asset: String } }
pub fn font_key(family: &str, weight: u16, italic: bool) -> String; // "<family>|<weight>|<normal|italic>"
pub fn new_project(id: String, name: String, rate: Rational) -> Result<Project, CoreError>; // Decision 9
// fin_core::ids: IdKind gains Caption, Graphic, Image, Adjustment, Nested, Marker, Transition, Bin,
// Effect, Link, Style; ID_KINDS lists all; fin-store `counters::kind_name` names them.
```

- [x] **Step 1: Write failing tests:**
  - `upcast_v1_snapshot_golden` (frozen `snapshot_v1.json` → `snapshot_v2.json`, compared as decoded `Project` and as re-encoded JSON);
  - `new_project_equals_upcast_genesis` (rates 30, 30000/1001, 25, 60; resolution 1080×1920);
  - `item_json_round_trips_every_kind`, `fonts_registry_round_trips`, `defaults_are_omitted` (a rev-0 project, with an empty `fonts`, encodes to the golden bytes);
  - `font_key_is_canonical` (family as written, weight digits, `normal`/`italic`);
  - `param_values_round_trip_through_record_bytes` (B2; a prop, 256 cases: random finite `f64`s, subnormals and `±0.0` included, in `ParamValue`s inside ops → `encode_record` → `decode_record` → bit-identical);
  - `ids_after_counts_every_kind`, `new_id_prefixes_are_unambiguous`;
  - every existing `ops_*`, `rebase*`, `undo`, `merge`, `pick` and props test passes on the v2 model;
  - `sp2_4a_4b_fixtures_open_equal_v2_goldens` (fin-store: the states equal `expected_v2.json`, and `upcast_snapshot(expected.project)` equals it too).
- [x] **Step 2: Run** `rtk cargo test -p fin-core --test upgrade_golden --test model --test record`. Expected: FAIL.
- [x] **Step 3: Implement.** The snapshot step is written on `Value` with literal names:
  - it reproduces `new_project`'s defaults (track names by kind order, `"Timeline 1"`, 1080×1920);
  - sets `kind: "media"` on assets;
  - moves `timeline` into `timelines[0]` with the project rate;
  - adds nothing for `fonts` (empty is the omitted default).

  Tests and `fin` send version 2; client v1 txs come in Task 7 through `normalize`.
- [x] **Step 4: Golden review.** Generate `expected_v2.json` for the three fixtures, read each against `expected.json` by hand (ids, times, assets equal; only shape and defaults differ), and commit them.
- [x] **Step 5: Run** the three gates and `rtk pnpm e2e` (scripts read `timelines[0]`) → PASS. **Commit** `feat(core): v2 model with timelines, item kinds, common fields and font pins; snapshot upcaster`.

**As built** (for Tasks 7, 11, 14, 18, 19 and 25):
- **Op payloads stay v1-shaped** (the record step is an identity until Task 7): `clip.*` still carry `Clip`, now 4B's shape plus an optional `speed` (default 1) and the flattened `Common` (all omitted at their defaults), so a v1 record decodes unchanged and an inverse restores a clip item exactly. `Op::ClipRestore.clip` is `Box<Clip>` (clippy `large_enum_variant`). `Item::to_clip()`/`From<Clip> for Item` convert; `clip.*` find only clip items (`NOT_FOUND` "clip" otherwise). `asset.add` builds `kind: media` with `duration: Some(..)`; `asset.remove`'s inverse needs a duration (`INVALID_OP` for a durationless asset until Task 7 gives `asset.add` its kinds). `EntityKind` keeps `clip` (Task 7 renames it together with `diff_v1` and its golden). `clip.trim` still assumes speed 1 (only a hand-made `clip.restore` can carry another speed; `item.trim` handles it).
- **Wire shapes the interface left open:** `ItemBody::Caption { #[serde(flatten)] block: CaptionBlock }` (a newtype variant made json-schema-to-typescript drop the `kind` tag); `FontSource` is tagged `type` (`{"type":"catalog",entry,style,catalog}` / `{"type":"asset",asset}`); `Interp` is tagged `type`; `SlotSpec.kind` is a `String` (Task 11 may narrow it); `CaptionStyle {}` is empty until Task 18. The generated TS is `Item = {id, start, duration, …common} & Item1` (the flattened body union gets the generator's name `Item1`).
- **Defaults omitted** (Decision 11): every new field at its default, floats compared bit for bit (`-0.0` is written and reads back as itself); the collections 4B already had (`assets`, `tracks`, `items`) are always written. Golden in `tests/model.rs` `defaults_are_omitted`.
- **Lookups span every timeline:** `ops::util::TrackAt {timeline, track}` (`get`/`get_mut`), `tracks()`, `items()`; `exists` covers every `IdKind` (item kinds: any item with the id). `Project::entity_ids()` lists every id (timelines, markers, tracks, transitions, items, effects, links, bins, assets, styles); `MemIds::after` raises, for each, the one kind whose prefix it carries, and `fin-store` `recover::remap::entity_ids` uses it. `counters::kind_name` names the 11 new kinds (`caption` … `style`; rows keyed by name, no migration).
- **Also added:** `DEFAULT_RESOLUTION` (1080×1920), `DEFAULT_TIMELINE_NAME`, `Track::new`, `TrackKind::letter`, `IdKind::of_item`/`is_item`, `Item::{end, asset, source_in, kind_name}`, `Rational::to_f64` and `Default` (zero). `check_kind` refuses media on caption tracks. `FontFormat` lives in `fin_core::model`; `fin-fonts` re-exports it.
- **Fixtures:** `tests/fixtures/{sp2,4a,4b}/expected_v2.json` (`{rev, project}`, per branch for 4B), generated once by `upcast_snapshot` of `expected.json`, reviewed (ids, times, assets equal; `kind`s, names, `timelines[0]` and the dropped `null` streams differ), frozen; every state test reads them. `crates/fin/tests/fixtures/rpc/*` recaptured (`schemaVersion: 2`). `float_roundtrip` is checked to matter: `param_values_round_trip_through_record_bytes` fails without it.
- `fin`: `addr` resolves tracks and clips across all timelines; `fin view`/`status` still show `timelines[0]` (Task 25).
- **For Task 7:** fill `record_v1.rs` (and freeze it), rename `EntityKind::Clip` → `Item` in `diff_v1` + golden, replace `Clip` and `Box<Clip>` with `Item` in `item.restore`, and move the float-roundtrip property onto `item.restore`/`item.insert` (it now rides in `clip.restore`'s common fields).


### Task 7: `item.*` ops replace `clip.*`; aliases; record upcaster; asset kinds; replay track check — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/{item_insert.rs, item_delete.rs, item_trim.rs, item_remove.rs, item_restore.rs, alias.rs}`, `crates/fin-core/src/ops/util/{content.rs (Decision 13), lock.rs (TRACK_LOCKED), touched.rs (Decision 25)}`.
- Delete: `ops/{clip_insert, clip_trim, clip_delete, clip_remove, clip_restore}.rs` (their logic moves into the `item_*` modules).
- Modify:
  - `ops/asset_add.rs` (kinds), `ops/defs.rs`, `upgrade/{record_v1.rs, diff_v1.rs}` (`clip` → `item` in the diff step);
  - `apply.rs` (aliases only through `normalize`; the replay track check over `touched` tracks), `tx.rs` (`normalize`);
  - `history/*` and `rebase/*` where `Op::ClipRestore`/`ClipRemove` are named;
  - `crates/fin-store/src/{commit.rs (normalize), recover/remap.rs, history_ops/stack_rows_tests.rs, write_tests.rs, branch/lineage_tests.rs}`;
  - `crates/fin-store-xhost/src/{history.rs, ops.rs}`;
  - `crates/fin/src/commands/clip.rs` (builds `item.*`; linked audio comes in Task 23), `crates/fin/src/output.rs`.
- Test: `crates/fin-core/tests/{ops_item.rs (new; replaces ops_insert.rs and ops_trim_delete.rs), upgrade_golden.rs, normalize.rs (new)}`, `crates/fin-store/tests/{fixture_4b.rs, records.rs, recover.rs}`, `crates/fin/src/commands/tests.rs`.

**Interfaces** (new ops are newtype variants, `Op::ItemInsert(ItemInsert)`, wire tag `"item.insert"`):
```rust
pub enum InsertMode { Ripple, Overwrite }   pub enum DeleteMode { Ripple, Lift }
pub enum TrimMode { Ripple, Roll, Overwrite, Strict /* internal */ }
pub struct ItemInsert { pub id: String /* $ */, pub track: String, pub at: Rational, pub duration: Rational,
    pub mode: InsertMode, #[serde(flatten)] pub body: ItemBody }
pub struct ItemDelete { pub item: String, pub mode: DeleteMode }
pub struct ItemTrim { pub item: String, pub edge: Edge /* start|end */, pub delta: Rational, pub mode: TrimMode }
pub struct ItemRemove { pub id: String, pub mode: DeleteMode }                    // internal
pub struct ItemRestore { pub item: Item, pub track: String, pub mode: DeleteMode } // internal; $ id allocates
pub struct AssetAdd { pub id: String, pub kind: AssetKind, pub path: String, pub fingerprint: String,
    pub duration: Option<Rational>, pub video: Option<VideoInfo>, pub audio: Option<AudioInfo>, pub image: Option<ImageInfo>,
    pub lottie: Option<LottieInfo>, pub font: Option<FontFile>, pub lut: Option<LutInfo>, pub bin: Option<String> }
// ops::alias: `Op::ClipInsert {..}`, `ClipTrim {..}`, `ClipDelete {..}` stay (struct variants, client only)
pub fn normalize(tx: Tx) -> Result<Tx, CoreError>; // Decision 6; re-exported from fin_core
// ops::util::touched
pub(crate) fn touched_tracks(project: &Project, own: &Diff) -> Vec<usize>; // Decision 25
```
Insert validation per kind (policy unless noted):
- `clip` on a video or audio track needs a `media` asset with that stream (`TRACK_KIND_MISMATCH`, structural as today);
- `image` (`image`/`svg` assets), `graphic` (`lottieTemplate`; slots checked in Task 11) and `adjustment` go on video tracks (Decision 48);
- `caption` goes on caption tracks (word rules in Task 19);
- `nested` is `INVALID_OP` with `fix` "nested items arrive in sub-project 6".

`item.trim` on the start edge records `by` and rebases keyframes and words (Decision 13). `strict` is `INVALID_OP` from clients. Every op on a locked track's items is `TRACK_LOCKED`.

- [ ] **Step 1: Write failing tests:**
  - goldens for `item.insert` (each allowed kind), `item.delete` (`ripple`, `lift`), `item.trim` (`ripple` on both edges, `strict`, speed ≠ 1), and the internal remove and restore. Each checks apply, inverse, shifts and `inverse_shifts_mirror`, and `ids_cover_every_id_in_json` for `item.restore` of an item with effects, a link, keyframes and a style;
  - `start_trim_rebases_keyframes_and_words` (exact inverse, `by` recorded); `strict_trim_refuses_overlap_in_every_mode`;
  - `replay_checks_tracks_touched_by_changes` (B1: a hand-made `lift` record whose restore overlaps is `REPLAY_FAILED` `CLIP_OVERLAP` though it records no shift);
  - `asset_add_validates_each_kind`;
  - `upcast_v1_record_golden_per_op` (each of the 7 v1 op shapes in `intent`, `resolved` and `inverse`; `clip.restore` without `ripple` → `ripple`), `upcast_v1_record_golden_whole`;
  - `normalize_maps_aliases`, `normalize_adds_media_kind_to_v1_asset_add`, `alias_outside_client_mode_is_invalid_op`, `v1_client_tx_is_accepted_and_upgraded`;
  - `track_locked_refuses_item_ops` (policy; replay applies);
  - fin-store:
    - `undo_of_4b_record_after_upgrade`, `merge_of_4b_branch_after_upgrade`, `cherry_pick_of_4b_record_after_upgrade` (4B fixture);
    - `replay_of_upcast_records_equals_v2_golden` (all three fixtures, also through a rebuild from the folder alone);
    - `diff_upcast_equals_replay_on_fixtures` again with `item`;
    - `counters_see_ids_inside_restores`, `recovery_remaps_nested_ids` (B6);
  - `fin`: `args_hash_of_4b_inputs_is_unchanged` (the pinned hashes from Task 2, Decision 7).
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_item --test upgrade_golden --test normalize`. Expected: FAIL.
- [ ] **Step 3: Implement.** The record step maps:
  - `clip.insert {id, track, at, asset, sourceIn, duration}` → `item.insert {id, track, at, duration, mode: "ripple", kind: "clip", asset, sourceIn}`;
  - `clip.trim {clip, edge, delta}` → `item.trim {item, edge, delta, mode: "ripple"}`;
  - `clip.delete {clip}` → `item.delete {item, mode: "ripple"}`;
  - `clip.remove {id, ripple}` → `item.remove {id, mode}`;
  - `clip.restore {clip, track, ripple = true}` → `item.restore {item: {id, start, duration, kind: "clip", asset, sourceIn}, track, mode}`;
  - `asset.add` + `kind: "media"`.

  It never touches ids, timestamps or bindings.
- [ ] **Step 4: Run** the three gates and `rtk pnpm e2e` → PASS. **Commit** `feat(core): item.* ops with modes; clip.* as client aliases; record upcaster v1→v2`.

### Task 8: Overwrite, roll, slip, split; content offsets in the rebase engine; `compose::overwrite` — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/util/carve.rs`, `crates/fin-core/src/ops/{item_slip.rs, item_split.rs}`, `crates/fin-core/src/compose/{mod.rs, overwrite.rs}`.
- Modify: `ops/item_insert.rs` (overwrite), `ops/item_trim.rs` (`roll`, `overwrite`), `ops/defs.rs` (`offsets_mut` in the op table), `crates/fin-core/src/rebase/{map.rs (content maps), engine.rs (map offsets), steps.rs}`.
- Test: `crates/fin-core/tests/{ops_modes.rs, rebase_modes.rs, content_map.rs}`.

**Interfaces:**
```rust
pub struct ItemSlip { pub item: String, pub delta: Rational }                        // clip, nested
pub struct ItemSplit { pub item: String, pub offset: Rational, pub id: String /* $ right part */ }
// ops::util::carve
/// Items fully inside [s, e) are removed (inverse: item.restore lift); one overlapping s is trimmed at
/// its end, one overlapping e at its start (`Updated` with `edge`, and `by` on a start edge); one
/// strictly containing [s, e) is OVERWRITE_SPLITS_ITEM. Returns the inverse ops (strict trims back,
/// then restores).
pub(crate) fn carve(track: &mut Track, s: Rational, e: Rational, d: &mut OpDiff<'_>) -> Result<Vec<Op>, CoreError>;
// rebase::map
impl StepMap { pub fn content(&self) -> &[(String, Rational)]; } // from the diff's `by` changes; invert negates
impl Mapping { pub fn map_content(&self, start: usize, item: &str, v: Rational) -> Result<Rational, Unmapped>; } // Overflow only
// Op
pub(crate) fn offsets_mut(&mut self) -> Vec<(String /* item */, &mut Rational)>;
// compose
/// Ops for an overwrite of `[at, at + duration)` on `track`: `item.split` of the one item strictly
/// containing it (with `$split`), then the overwrite insert. Echo data says what it sent.
pub fn overwrite(project: &Project, track: &str, at: Rational, duration: Rational, body: ItemBody, id: &str)
    -> Result<Composed, CoreError>;
pub struct Composed { pub ops: Vec<Op>, pub echo: serde_json::Value }
```
Rules:
- **Overwrite** insert and trim record no shifts.
- **Roll:** moves the cut between the item and its touching neighbour (`NOT_ADJACENT` otherwise). On a start edge, the neighbour's `sourceIn` moves by `delta × speed`, and the start-edge item records `by`.
- **Slip:** moves `sourceIn` only; the source range is policy.
- **Split:**
  - `offset` lies strictly inside `(0, duration)`, at least one frame from each end (policy);
  - the right part copies `body` and `common` (effects keep their ids; keyframes and words are rebased by `−offset`) and gets no `link`;
  - a caption is `INVALID_OP` with `fix: "use caption.split"`;
  - the inverse is internal: `item.remove` of the right part (lift), then `item.trim` of the left part (`strict`, +right length).

- [ ] **Step 1: Write failing tests:**
  - goldens for every mode (with `inverse_shifts_mirror`);
  - `overwrite_removes_covered_and_trims_partial`, `overwrite_inside_one_item_is_refused`, `compose_overwrite_splits_first`;
  - `roll_requires_adjacent_neighbour`, `roll_moves_both_edges_and_source`;
  - `slip_keeps_timeline_positions`, `slip_source_range_is_policy` (replay applies a slip past the asset), `split_copies_effects_keyframes_rebased`, `split_on_caption_is_invalid_op_with_fix`;
  - B4: `undo_of_overwrite_conflicts_with_later_insert` (an insert into the vacated range, then undo: `CLIP_OVERLAP` conflict, nothing carved), `undo_of_split_after_neighbour_extend_conflicts`;
  - B3: `stale_split_after_start_trim_maps_offset`, `content_map_inverse_negates`, `mirrored_trim_and_undo_cancel`;
  - rebase: `overwrite_vs_trim_of_overwritten_item_conflicts` (`SAME_EDGE` from the diff's edge change), `overwrite_vs_delete_of_covered_item` (not a conflict), `roll_vs_neighbour_start_trim_same_edge`, `stale_overwrite_maps_over_ripple_insert`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_modes --test rebase_modes --test content_map`. Expected: FAIL.
- [ ] **Step 3: Implement** (each module < 250 lines).
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): overwrite, roll, slip and split; content offsets in rebase`.

### Task 9: Moves, slides, link groups; `compose::clip_with_audio`, `compose::split_linked` — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/{item_move.rs, item_slide.rs, item_link.rs, item_unlink.rs}`, `crates/fin-core/src/ops/util/group.rs`, `crates/fin-core/src/compose/{audio.rs, split.rs}`.
- Modify: `ops/{item_delete.rs, item_trim.rs}` (group behaviour, Decision 19).
- Test: `crates/fin-core/tests/{ops_move_link.rs, rebase_moves.rs, compose.rs}`.

**Interfaces:**
```rust
pub struct ItemMove { pub item: String, pub track: String, pub at: Rational, pub mode: InsertMode }
pub struct ItemSlide { pub item: String, pub delta: Rational }
pub struct ItemLink { pub items: Vec<String>, pub id: String /* $lk */ }
pub struct ItemUnlink { pub group: String }
pub(crate) fn members(project: &Project, item: &str) -> Result<Vec<(usize, usize, usize)>, CoreError>; // ops::util::group
// compose (Decision 21; Decision 17)
pub fn clip_with_audio(project: &Project, track: &str, at: Rational, duration: Rational, asset: &str,
    source_in: Rational, speed: Rational, mode: InsertMode, audio_track: Option<&str>) -> Result<Composed, CoreError>;
pub fn split_linked(project: &Project, item: &str, at: Rational) -> Result<Composed, CoreError>;
```
Rules:
- **Move:**
  - `at` is in the pre-op frame (Decision 15). Ripple records, per member, a close on its source track (`{end, −dur}`), then an open on its target (`{at', +dur}`, where `at'` is `at` mapped through the close);
  - the inverse records them mirrored in reverse (Decision 23);
  - a same-track ripple move whose `at` lies inside the item is `MOVE_INTO_ITSELF`;
  - overwrite carves the target range for each member;
  - the target track kind must fit the item kind; only the named item changes track.
- **Slide:** moves the item by `delta`, trimming the touching neighbour on each side (start-edge `by` on the next item). A side without a touching neighbour needs room in the gap (`SLIDE_BLOCKED`). It never ripples.
- **Link:** all items on one timeline, none already linked (`ALREADY_LINKED`), at least 2. **Unlink** clears `link` on every member; its inverse is internal `item.link` with the literal id.

- [ ] **Step 1: Write failing tests:**
  - goldens for both move modes, across and within tracks (`inverse_shifts_mirror`); `move_into_itself_is_refused`; `slide_trims_both_neighbours`, `slide_blocked_by_gap`;
  - `link_unlink_inverse_exact`;
  - `linked_move_moves_members_same_delta`, `linked_trim_is_all_or_nothing`, `linked_delete_ripples_each_track`, `linked_delete_footprint_covers_members`;
  - compose: `clip_with_audio_links_video_and_audio`, `clip_with_audio_needs_an_audio_track`, `split_linked_splits_members_and_links_rights`;
  - rebase: `move_vs_concurrent_trim_composes`, `move_vs_delete_removed`, `move_to_other_track_meets_its_edits`, `stale_move_at_maps_over_target_track_insert`, `linked_delete_vs_member_trim_is_not_a_conflict`, `slide_vs_slide_same_param`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_move_link --test rebase_moves --test compose`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): item.move, item.slide, link groups; linked-audio and linked-split builders`.

### Task 10: Transitions; `compose::transition_with_links` — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/{transition_set.rs, transition_remove.rs}`, `crates/fin-core/src/ops/util/prune.rs`, `crates/fin-core/src/compose/transition.rs`.
- Modify: `crates/fin-core/src/ops/mod.rs` (the post-op prune pass in `apply`), `ops/util/check.rs` (`TRANSITION_INVALID`).
- Test: `crates/fin-core/tests/ops_transitions.rs`, `crates/fin-core/tests/compose.rs`.

**Interfaces:**
```rust
pub struct TransitionSet { pub id: String /* $ creates; literal updates (or restores, Internal) */,
    pub track: String, pub between: [String; 2], pub kind: TransitionKind, pub duration: Rational,
    #[serde(default)] pub curve: Curve }
pub struct TransitionRemove { pub transition: String }
/// After any op but transition.*: removes every transition on the tracks the op touched (Decision 25)
/// whose items no longer touch. Records `Removed` and returns `transition.set` restores, appended
/// after the op's own inverse.
pub(crate) fn prune(project: &mut Project, d: &mut OpDiff<'_>) -> Result<Vec<Op>, CoreError>;
pub fn transition_with_links(project: &Project, a: &str, b: &str, kind: TransitionKind, duration: Rational,
    curve: Curve, linked: bool) -> Result<Composed, CoreError>; // Decision 22
```
Rules (policy unless noted):
- `between` must touch (`NOT_ADJACENT`; also structural);
- `duration` is ≥ one frame and ≤ the shorter item;
- clip handles follow Decision 22;
- a `$` id at a cut that already has a transition is `TRANSITION_EXISTS`;
- the footprint touches `transition.out` on A and `transition.in` on B.

- [ ] **Step 1: Write failing tests:**
  - goldens for set, update and remove;
  - `ripple_delete_of_b_prunes_and_undo_restores`, `pruned_transition_restored_by_undo`, `trim_separating_items_prunes`, `move_prunes`, `overwrite_prunes`, `lift_prunes`;
  - `ripple_elsewhere_keeps_transition`;
  - `handles_are_policy` (replay applies), `replay_rejects_non_adjacent_transition`;
  - `two_sets_at_one_cut_conflict_same_param`;
  - compose: `transition_with_links_crossfades_audio_partners`, `transition_with_links_skips_partners_that_do_not_touch`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_transitions --test compose`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates and `rtk pnpm e2e` → PASS. **Commit** `feat(core): transitions at cuts, pruned by a post-op pass; linked-partner builder`. **Batch A is done.**

### Task 11: The settings registry and `param.set` — tier: opus

**Files:**
- Create: `crates/fin-core/src/params/{mod.rs (ParamSpec, ParamType, find, validate), item.rs, effects.rs, scopes.rs (track, timeline, marker, style), access.rs (get/set on the typed model)}`, `crates/fin-core/src/ops/param_set.rs`.
- Modify: `ops/item_insert.rs` (graphic slots checked against `LottieInfo.slots`: `SLOT_UNKNOWN`, `PARAM_TYPE`).
- Test: `crates/fin-core/tests/params.rs`.

**Interfaces:**
```rust
#[derive(Debug, Serialize, JsonSchema)]
pub struct ParamSpec { pub path: &'static str, pub scope: Scope, pub ty: ParamType, pub animatable: bool,
    pub default: DefaultValue, pub unit: Option<&'static str>, /** `{min}`, `{max}`, `{unit}` filled */ pub fix: &'static str }
pub enum Scope { Item { kinds: &'static [&'static str] }, Effect { kind: &'static str }, Track, Timeline, Marker, Style }
pub enum ParamType { Number { min: f64, max: f64 }, Bool, Enum { values: &'static [&'static str] }, Color,
    Vec2 { min: [f64; 2], max: [f64; 2] }, Time { min: Option<&'static str> }, Text { max_len: u32 },
    AssetRef { kinds: &'static [&'static str] } }
pub fn specs() -> &'static [ParamSpec];
pub fn find(scope: &ScopeOf<'_>, path: &str) -> Result<&'static ParamSpec, CoreError>; // PARAM_UNKNOWN + alternatives
pub fn validate(spec: &ParamSpec, value: &ParamValue) -> Result<(), CoreError>; // PARAM_TYPE / INVALID_RANGE
pub fn get(item: &Item, path: &str) -> Result<ParamValue, CoreError>;
pub fn set(item: &mut Item, path: &str, value: ParamValue) -> Result<ParamValue /* old */, CoreError>;
pub struct ParamSet { pub item: String, pub path: String, pub value: ParamValue } // op `param.set`
```
Paths:
- **Items:**
  - `transform.position|scale|rotation|anchor`, `crop.left|right|top|bottom`, `opacity`, `blend`;
  - `audio.gain|pan|fadeIn|fadeOut`;
  - `mask.feather|choke|invert` (`PARAM_UNKNOWN` on an item without a mask, until sub-project 6 adds mask edits);
  - `speed` (clip, not animatable; the source extent is policy);
  - `slots.<id>` (graphic), `effects.<fx>.<param>`.
- **Effects:** `colorAdjust.{exposure −4..4 EV, contrast −1..1, saturation 0..2, temperature −1..1, tint −1..1}`, `lut.{asset (lut), intensity 0..1}`, `blur.radius 0..200 px`.
- **Tracks:** `name, mute, solo, lock, gain`. **Timelines:** `name, resolution, rate` (rate only while it has no items: `TIMELINE_NOT_EMPTY`). **Markers:** `label, color, at, duration`.

`param.set`:
- touches `Param(path)`;
- a path with keyframes is `PARAM_ANIMATED`;
- an unknown typed path is structural (`REPLAY_FAILED` on replay), and a range is policy.

- [ ] **Step 1: Write failing tests:**
  - `every_spec_path_gets_and_sets`, `defaults_match_model_defaults`;
  - `range_errors_carry_details_and_fix_text` (exact strings, spec §8 risk);
  - `param_set_inverse_exact`, `param_set_on_animated_path_refused`;
  - `same_param_conflicts_disjoint_params_merge`;
  - `graphic_slots_validated_against_template`;
  - `specs_serialise_deterministically`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test params`. Expected: FAIL.
- [ ] **Step 3: Implement.** `access.rs` is one `match` per path group; files < 400 lines.
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): settings registry and param.set`.

### Task 12: Tracks and timelines — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/{track_add, track_remove, track_restore, track_move, track_set, timeline_add, timeline_set, timeline_remove, timeline_restore}.rs`.
- Test: `crates/fin-core/tests/ops_tracks.rs`, `crates/fin-store/tests/recover.rs` (an addition).

**Interfaces:**
```rust
pub struct TrackAdd { pub id: String, pub timeline: String, pub kind: TrackKind, pub index: Option<u32>, pub name: Option<String> }
pub struct TrackRemove { pub track: String, #[serde(default)] pub with_items: bool }
pub struct TrackRestore { pub track: Track, pub timeline: String, pub index: u32 } // internal
pub struct TrackMove { pub track: String, pub index: u32 }
pub struct TrackSet { pub track: String, pub path: String, pub value: ParamValue }
pub struct TimelineAdd { pub id: String, pub name: String, pub rate: Rational, pub resolution: Resolution }
pub struct TimelineSet { pub timeline: String, pub path: String, pub value: ParamValue }
pub struct TimelineRemove { pub timeline: String }
pub struct TimelineRestore { pub timeline: Timeline, pub index: u32 }             // internal
```
Rules:
- `track.remove` without `with_items` on a non-empty track is `TRACK_NOT_EMPTY`. With items, it removes them and their transitions; other members of their link groups keep `link`.
- `timeline.remove` is refused while a `nested` item names it (`TIMELINE_NESTED`, `details.items`) and for the last timeline (`INVALID_OP`).
- Footprints:
  - removals list every removed entity (diff `Removed`), and adds create;
  - `track.move` touches `Param("index")`; the `*_set` ops touch `Param(path)`;
  - none records shifts.

- [ ] **Step 1: Write failing tests:**
  - goldens for every op (`ids_cover_every_id_in_json` for both restores);
  - `track_remove_with_items_inverse_restores_items_transitions_links`, `timeline_remove_refused_while_nested` (a project built literally), `last_timeline_is_kept`;
  - `stale_edit_on_removed_track_conflicts_removed`;
  - fin-store: `recovery_remaps_ids_inside_track_restore` (B6).
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_tracks`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): track and timeline ops`.

### Task 13: Markers, bins and assets — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/{marker_add, marker_set, marker_remove, bin_add, bin_move, bin_remove, asset_move}.rs`.
- Modify: `ops/asset_remove.rs` (now a client op: `ASSET_IN_USE` with `details.items` over clips, graphics, images, `lut` effect params, and font pins with `source: Asset`; Task 18 adds those pins).
- Test: `crates/fin-core/tests/ops_library.rs`.

**Interfaces:**
```rust
pub struct MarkerAdd { pub id: String, pub timeline: String, pub at: Rational, pub duration: Option<Rational>,
    pub label: String, pub color: Option<String> }
pub struct MarkerSet { pub marker: String, pub path: String, pub value: ParamValue }
pub struct MarkerRemove { pub marker: String }
pub struct BinAdd { pub id: String, pub name: String, pub parent: Option<String> }
pub struct BinMove { pub bin: String, pub parent: Option<String> }
pub struct BinRemove { pub bin: String }
pub struct AssetMove { pub asset: String, pub bin: Option<String> }
```
Rules:
- markers never ripple (Decision 26);
- `bin.remove` is refused while assets or bins are in it (`BIN_NOT_EMPTY`); `bin.move` into its own subtree is `BIN_CYCLE`;
- inverses are the add op with a literal id (`Internal`) or the inverse set/move;
- footprints: `marker.set` touches `Param(path)`, `bin.move` touches `Param("parent")`, `asset.move` touches `Param("bin")`.

- [ ] **Step 1: Write failing tests:** goldens; `marker_positions_never_map` (a ripple before a marker leaves it); `bin_cycle_refused`; `asset_remove_in_use_lists_items`; `asset_remove_then_undo_restores_kind_fields`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_library`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): marker, bin and asset ops`.

### Task 14: Effects and keyframes — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/{effect_add, effect_remove, effect_restore, effect_move, keyframe_set, keyframe_remove}.rs`.
- Test: `crates/fin-core/tests/ops_effects_keyframes.rs`.

**Interfaces:**
```rust
pub struct EffectAdd { pub item: String, pub id: String /* $fx */, pub kind: String, pub at: Option<u32>,
    #[serde(default)] pub params: BTreeMap<String, ParamValue> }
pub struct EffectRemove { pub item: String, pub effect: String }
pub struct EffectRestore { pub item: String, pub effect: Effect, pub index: u32 } // internal
pub struct EffectMove { pub item: String, pub effect: String, pub to: u32 }
pub struct KeyframeSet { pub item: String, pub path: String, pub at: Rational, pub value: ParamValue, pub interp: Interp }
pub struct KeyframeRemove { pub item: String, pub path: String, pub at: Rational }
```
Rules (policy unless noted):
- the effect kind is a registry effect (`PARAM_UNKNOWN`);
- the path is animatable (`PARAM_NOT_ANIMATABLE`); the value follows the spec;
- bézier `x1`, `x2` lie in `[0, 1]`; `at` lies in `[0, duration]` (stored times outside are fine on replay, Decision 13);
- `keyframe.set` at an existing `at` replaces it, and its inverse restores the old keyframe (or removes it);
- `keyframe.*` `at` is an item offset (`offsets_mut`, Decision 16);
- footprints follow Decision 20.

- [ ] **Step 1: Write failing tests:**
  - goldens (`ids_cover_every_id_in_json` for `effect.restore`);
  - `effect_ids_come_from_the_global_counter_and_split_keeps_them`;
  - `keyframe_set_replaces_at_same_time`, `keyframe_on_static_path_refused`;
  - B3: `stale_keyframe_after_start_trim_keeps_identity` (the stale `keyframe.set` at `a` after a concurrent start trim by `d` replaces the keyframe now at `a − d`), `stale_keyframe_remove_after_start_trim`;
  - `concurrent_keyframes_at_different_times_merge`, `same_keyframe_conflicts_same_param`, `effect_move_vs_move_same_param`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_effects_keyframes`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): effects and keyframes`.

### Task 15: `eval_frame` — tier: opus

Eval covers every item kind, `nested` and masks included (built literally in tests, Decision 48).

**Files:**
- Create: `crates/fin-core/src/eval/{mod.rs, plan.rs, interp.rs, source.rs, frame.rs, transition.rs}`.
- Modify: `crates/fin-core/src/schema.rs` (`FramePlan`), `crates/fin-core/src/time.rs`.
- Test: `crates/fin-core/tests/{eval_frame.rs, eval_interp.rs}`.

**Interfaces:**
```rust
pub fn eval_frame(project: &Project, timeline: &str, t: Rational, m: Option<&dyn TextMeasurer>) -> Result<FramePlan, CoreError>;
pub struct FramePlan { pub timeline: String, pub t: Rational, pub resolution: Resolution, pub layers: Vec<Layer>,
    pub text_engine: Option<String> }
pub struct Layer { pub item: String, pub kind: ItemKind, pub track: String, pub local: Rational, pub source: Option<SourceRef>,
    pub transform: Transform, pub crop: Crop, pub opacity: f64, pub blend: BlendMode, pub effects: Vec<EffectPlan>,
    pub mask: Option<MaskPlan>, pub transition: Option<TransitionState>, pub caption: Option<CaptionPlan>,
    pub graphic: Option<GraphicPlan>, pub children: Option<Vec<Layer>> }
pub struct SourceRef { pub asset: String, pub time: Rational }
pub struct EffectPlan { pub id: String, pub kind: String, pub params: BTreeMap<String, PlanValue> }
pub enum PlanValue { Bool(bool), Number(f64), Vec2([f64; 2]), Color([f64; 4]), Text(String) } // Decision 12
pub struct TransitionState { pub id: String, pub kind: TransitionKind, pub curve: Curve, pub progress: f64, pub role: Role }
pub struct CaptionPlan { pub words: Vec<WordPlan>, pub style: String, pub layout: Option<TextLayout>,
    pub unmeasured: Option<Unmeasured> /* {reason: fontMissing|fontFetchFailed|fontUnpinned|noMeasurer, keys} */ } // filled in Task 20
pub struct GraphicPlan { pub asset: String, pub time: Rational, pub slots: BTreeMap<String, PlanValue> }
pub fn interpolate(kfs: &[Keyframe], at: Rational, base: &ParamValue, ty: &ParamType) -> PlanValue;
pub fn bezier_y(c: [f64; 4], u: f64) -> f64; // Decision 42
pub const PRESETS: &[(&str, [f64; 4])];   // ease, easeIn, easeOut, easeInOut (CSS)
```
Rules:
- An item is active for `start ≤ t < start + duration`. Within a transition `[cut − d/2, cut + d/2)`, A continues (its source time extends) and B starts early; `progress = (t − (cut − d/2)) / d`.
- Nested: the child time is `sourceIn + local`, recursing (a cycle is `NESTED_CYCLE`, depth ≤ 16).
- Hold, bool, enum and text values step; numbers, `vec2` and colours interpolate.
- Never NaN or ∞ (checked; `INTERNAL` if violated).

- [ ] **Step 1: Write failing tests:**
  - `bezier_matches_reference_table` (values computed offline at high precision, kept in the test), `presets_are_css_values`, `hold_linear_bezier_interp`, `colour_interpolates_per_component`, `interp_outside_keyframes_clamps`;
  - `speed_maps_source_time`, `transition_layers_and_progress`, `nested_source_time_and_group_layer`, `adjustment_layer_has_effects_only`, `mask_values_resolved`;
  - `muted_video_track_is_hidden`, `layer_order_is_track_order`;
  - `no_nan_in_plans` (prop, 128), `plan_json_is_stable`.
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test eval_frame --test eval_interp`. Expected: FAIL.
- [ ] **Step 3: Implement** within the float rules.
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): eval_frame with keyframes, transitions and nesting`.

### Task 16: `eval_audio` — tier: opus

**Files:**
- Create: `crates/fin-core/src/eval/{audio.rs, segments.rs}`.
- Test: `crates/fin-core/tests/eval_audio.rs`.

**Interfaces:**
```rust
pub fn eval_audio(project: &Project, timeline: &str, from: Rational, to: Rational) -> Result<AudioPlan, CoreError>;
pub struct AudioPlan { pub timeline: String, pub from: Rational, pub to: Rational, pub items: Vec<AudioItem> }
pub struct AudioItem { pub item: String, pub track: String, pub asset: String, pub from: Rational, pub to: Rational,
    pub source_from: Rational, pub speed: Rational, pub gain: Vec<GainSegment>, pub pan: Vec<PanSegment> }
pub struct GainSegment { pub from: Rational, pub to: Rational, pub db: [f64; 2], pub interp: Interp, pub shapes: Vec<Shape> }
pub struct Shape { pub kind: ShapeKind /* fadeIn|fadeOut|crossIn|crossOut */, pub curve: Curve, pub p: [f64; 2] }
pub struct PanSegment { pub from: Rational, pub to: Rational, pub pan: [f64; 2], pub interp: Interp }
```
Rules (Decision 41):
- `db` = track gain + item `audio.gain` (keyframed in dB);
- segments break at keyframes, fade ends and transition edges, clipped to `[from, to)`;
- mute omits; any solo keeps only soloed tracks;
- crossfades come from audio-track transitions.

- [ ] **Step 1: Write failing tests:** `segments_break_at_every_edge`, `gain_sums_track_and_item_db`, `fades_are_shapes`, `crossfade_shapes_with_curve`, `mute_and_solo`, `range_clipping`, `video_tracks_are_silent`, `no_nan` (prop, 64).
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test eval_audio`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): eval_audio gain and pan plans`.

### Task 17: The determinism corpus, the float gate and the wasm32 runner — tier: opus

**Files:**
- Create: `crates/fin-core/tests/eval_corpus.rs`, `crates/fin-core/tests/eval_corpus/{projects/*.json, expected.json}`.
- Create: `scripts/check-eval-floats.mjs`, `scripts/check-eval-floats.test.mjs`. Modify: `scripts/gates.mjs`.
- Modify: `.github/workflows/ci.yml`, on ubuntu only:
  - `rustup target add wasm32-wasip1`;
  - wasmtime through a SHA-pinned `bytecodealliance/actions/wasmtime/setup`;
  - `CARGO_TARGET_WASM32_WASIP1_RUNNER="wasmtime --dir ."`, then `cargo test -p fin-core --target wasm32-wasip1 --test eval_corpus`, `cargo test -p fin-text --target wasm32-wasip1 --test measure` and, with the warmed cache under the mapped dir, `--test corpus_real`. `--dir` gives the tests the corpus files and the font cache.
- Modify: `crates/fin-core/Cargo.toml` (`blake3` dev-dependency).

**Corpus:** about 20 projects × 6–10 times each. They cover:
- bézier keyframes on every animatable number path, plus colour keyframes;
- transitions with both curves; speeds 1/2, 3/2 and 30000/1001;
- nested 2 deep, an adjustment layer, masks;
- captions with a fake measurer (real-font layouts are pinned by `fin-text` and e2e);
- audio plans over ranges that cut segments.

`expected.json` lists `{case, t | range, blake3}`; a mismatch prints the first differing plan.

**Gate:** `check-eval-floats.mjs` refuses, in `crates/fin-core/src/{eval,params,captions}/**` and `layout.rs`:
- method calls `\.(sin|cos|tan|asin|acos|atan2?|sinh|cosh|tanh|exp|exp2|exp_m1|ln|ln_1p|log|log2|log10|powf|powi|cbrt|hypot|mul_add)\(`;
- path calls `f(32|64)::(sin|cos|…|mul_add)\(`;
- any `libm::` and `f64::consts`.

Its test feeds it `x.powf(2.0)`, `f64::sin(x)` and `libm::exp(x)` and expects three failures.

- [ ] **Step 1: Write** the corpus test with an empty `expected.json` (it fails, listing the computed hashes), and the gate with its test.
- [ ] **Step 2:** Fill `expected.json` from a Linux run, and check by hand that 5 sampled plans make sense (layer order, progress values, source times).
- [ ] **Step 3: wasm32:**
  - run `rtk rustup target add wasm32-wasip1`, install wasmtime locally in `archive/tools/`, and run both corpora under it with `--dir .`;
  - if dev-dependencies (proptest) keep the target from building, gate the property code behind `cfg(not(target_os = "wasi"))`;
  - if wasm cannot run in CI, keep the native run and say so in TASKS.md and spec §5.4 (the spec allows it).
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `test(core): plan-hash determinism corpus, float gate, wasm32 runner`. **Batch B is done.**

### Task 18: Captions, styles and font pins in the model — tier: opus

**Files:**
- Create: `crates/fin-core/src/captions/{mod.rs, blocks.rs, srt.rs, align.rs, presets.rs}`, `crates/fin-core/src/model/caption.rs` (`CaptionStyle`), `crates/fin-core/src/ops/{style_add, style_set, style_remove, font_pin, font_unpin, font_repin}.rs`.
- Modify: `crates/fin-core/src/layout.rs` (`font_needs`, `faces_for`, `scripts_of`), `params/scopes.rs` (style paths), `ops/asset_remove.rs`, `crates/fin-core/Cargo.toml` (`unicode-script`).
- Test: `crates/fin-core/tests/{captions.rs, ops_styles.rs, ops_fonts.rs, layout_faces.rs}`.

**Interfaces:**
```rust
pub struct CaptionStyle { pub id: String, pub name: String, pub text: TextStyle /* family, weight, italic, size, … */,
    pub color: String, pub stroke: Option<Stroke>, pub background: Option<CaptionBox>, pub highlight: Option<String>,
    pub position: CaptionPosition, pub safe_zone: String, pub max_chars: u32, pub max_lines: u32, pub gap: Rational,
    pub max_duration: Option<Rational> }
pub fn presets() -> &'static [CaptionStyle]; pub fn safe_zones() -> &'static [SafeZone];
pub fn blocks(words: &[Word], style: &CaptionStyle) -> Vec<BlockDraft>;
pub fn parse_srt(text: &str) -> Result<Vec<Word>, CoreError>;
pub fn align_text(old: &[Word], text: &str) -> Vec<Word>;
pub struct StyleAdd { pub id: String, pub name: String, pub from: Option<String>, pub set: BTreeMap<String, ParamValue> }
pub struct StyleSet { pub style: String, pub path: String, pub value: ParamValue }   pub struct StyleRemove { pub style: String }
pub struct FontPinOp { pub key: String, pub pin: FontPin, pub fallback: Option<u32> }      // op "font.pin"
pub struct FontUnpin { pub key: String }   pub struct FontRepin { pub key: String, pub pin: FontPin }
// layout
pub struct FontNeeds { pub faces: BTreeSet<(String, u16, bool)>, pub scripts: BTreeSet<String> }
pub fn font_needs(project: &Project) -> FontNeeds;           // every style in use and every caption's text
pub fn scripts_of(words: &[Word]) -> BTreeSet<String>;        // unicode-script; Common/Inherited ignored
/// The request faces for `style` and `words` (Decision 36), or the keys that have no pin.
pub fn faces_for(project: &Project, style: &CaptionStyle, words: &[Word]) -> Result<Vec<FaceRef>, Vec<String>>;
```
Rules:
- `font.pin` with the same `sha256` is a no-op; another hash is `FONT_PINNED` (policy; structural on replay);
- `font.unpin` is `FONT_IN_USE` while a style resolves to the key;
- a style whose face has no pin is valid (it is unmeasured: Decision 36);
- the footprint is entity `"font/<key>"` (pin creates, unpin removes, repin touches `Param("sha256")`);
- `style.remove` is `STYLE_IN_USE {items}` while a caption uses the style;
- safe-zone values come from the platforms' public creator guidelines, with each source and date named in the module doc.

- [ ] **Step 1: Failing tests:**
  - `blocks_split_on_gap_chars_lines_duration`, `srt_parses_and_distributes`, `srt_errors_name_the_line`, `align_keeps_matching_times`, `align_distributes_by_chars`, `align_is_exact_rational`;
  - style goldens, `style_remove_in_use_refused`, `presets_and_zones_are_valid`;
  - font goldens (`ids_cover_every_id_in_json` for an asset pin):
    - `same_hash_pin_is_a_noop_and_merges`, `other_hash_pin_is_font_pinned`;
    - `unpin_in_use_refused`, `repin_inverse_exact`, `font_pin_never_reads_bytes` (the op works with no provider anywhere);
  - `font_needs_lists_styles_and_scripts`, `faces_for_orders_primary_then_matching_fallbacks`, `faces_for_reports_unpinned_keys`.
- [ ] **Step 2–3:** Run → FAIL; implement.
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): caption blocks, SRT, alignment, styles and font pins`.

### Task 19: Caption item edits — tier: opus

**Files:**
- Create: `crates/fin-core/src/ops/{caption_set_text, caption_set_words, caption_split, caption_merge, caption_set_style}.rs`.
- Modify: `ops/item_insert.rs` (caption word rules).
- Test: `crates/fin-core/tests/ops_captions.rs`.

**Interfaces:**
```rust
pub struct CaptionSetText { pub item: String, pub text: String }
pub struct CaptionSetWords { pub item: String, pub words: Vec<Word> }   // internal (inverse)
pub struct CaptionSplit { pub item: String, pub word: u32, pub id: String /* $cap */ }
pub struct CaptionMerge { pub item: String, pub with: String }
pub struct CaptionSetStyle { pub item: String, pub style: String, #[serde(default)] pub overrides: Option<BTreeMap<String, ParamValue>> }
```
Rules (policy):
- words are non-empty and ordered without overlap, within `[0, duration]`;
- the style reference must exist (structural `NOT_FOUND`);
- `caption.merge` needs `with` to be the next caption item on the same track; the result spans to its end;
- the merge's inverse is `caption.setWords`, then `item.trim` (`strict`), then `item.restore` (lift);
- footprints follow Decision 20.

- [ ] **Step 1: Write failing tests:** goldens; `caption_merge_inverse_exact`; `set_text_vs_set_text_conflicts_same_param`; `split_after_set_text_reruns` (a policy failure is a conflict with that code); `caption_split_vs_start_trim` (word index unaffected; times rebased).
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --test ops_captions`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core): caption set-text, split, merge and style edits`.

### Task 20: Measurement in the edit path and eval; pin planning — tier: opus

**Files:**
- fin-core:
  - `apply.rs` (`ApplyEnv`, `apply_tx_env`; the overflow check in `finish`), `rebase/engine.rs` (`Rebase.env`, `rebase_tx(.., env)`);
  - `history/mod.rs` (`LogView::measurer`) and the planners;
  - `captions/overflow.rs`, `eval/frame.rs` (layouts, `unmeasured`);
  - `layout.rs` `testing { FakeMeasurer, PanicMeasurer, MapBytes }` (feature `test-measurer`).
- fin-store: `open.rs` (`OpenOptions.measurer`), `lib.rs` (`Store::set_measurer`), `commit.rs`, `history_ops/mod.rs`, `recover/rows.rs`.
- fin-fonts: `src/plan.rs`.
- `crates/fin-core/tests/common/memlog.rs`.
- Test: `crates/fin-core/tests/overflow.rs`, `crates/fin-store/tests/measure.rs`, `crates/fin-fonts/tests/plan.rs`.

**Interfaces:**
```rust
#[derive(Clone, Copy, Default)] pub struct ApplyEnv<'a> { pub measurer: Option<&'a dyn TextMeasurer> }
pub fn apply_tx_env(project: &Project, tx: &Tx, ids: &mut dyn IdAlloc, mode: ApplyMode, env: ApplyEnv<'_>) -> Result<Applied, CoreError>;
pub fn rebase_tx(base: &Project, head: &Project, tx: &Tx, since: &[CommittedTx], ids: &mut dyn IdAlloc, env: ApplyEnv<'_>)
    -> Result<Rebased, CoreError>;
pub trait LogView { /* 4B */ fn measurer(&self) -> Option<&dyn TextMeasurer> { None } }
/// Once per tx: each caption the tx changed (and each user of a changed style or resolution, and each
/// caption whose pins changed), laid out through `faces_for` + the measurer. `Unavailable` or unpinned
/// → skipped (Decision 36). CAPTION_OVERFLOW {item, lines, maxLines, width, maxWidth, missing, opIndex}.
pub(crate) fn check_overflow(project: &Project, diff: &Diff, m: &dyn TextMeasurer) -> Result<(), CoreError>;
// fin_fonts::plan (Decision 35)
pub fn plan_pins(head: &Project, ops: &[Op], catalog: &FontCatalog) -> Result<Vec<Op> /* font.pin */, CoreError>;
```

- [ ] **Step 1: Failing tests:**
  - `replay_never_calls_the_measurer` (`PanicMeasurer` through `apply_committed`, import and rebuild);
  - `client_insert_overflow_refused`, `overflow_checked_once_at_tx_end`, `style_set_rechecks_every_user`, `resolution_change_rechecks_timeline`, `repin_rechecks_users`;
  - `stale_commit_rechecks_overflow`, `merge_rechecks_overflow`, `undo_rechecks_overflow`, `recovery_rechecks_overflow`;
  - **`missing_font_never_blocks_and_logs_nothing`** (with a provider lacking the bytes, the edit commits and the record, diff and snapshot are byte-equal to the same edit made without a measurer);
  - `unpinned_text_is_unmeasured`;
  - **`same_pins_same_layout_across_providers`** (two `MapBytes` holding the same bytes: equal plans);
  - **`catalog_update_keeps_pinned_layout`** (a newer catalog with another file for the same family: an existing project's plans are unchanged until `font.repin`);
  - `eval_reports_unmeasured_with_reason`, `measurement_resumes_when_bytes_arrive`;
  - plan: `plan_pins_for_new_style_face`, `plan_pins_fallback_for_new_script`, `plan_pins_nothing_when_pinned`, `plan_never_pins_unknown_face` (`FONT_NOT_IN_CATALOG` is returned as a note, not an error).
- [ ] **Step 2: Run** `rtk cargo test -p fin-core --features test-measurer --test overflow`, `rtk cargo test -p fin-store --test measure` and `rtk cargo test -p fin-fonts --test plan` → FAIL.
- [ ] **Step 3: Implement.** Thread `env` explicitly, with no global state.
- [ ] **Step 4: Run** the three gates → PASS. **Commit** `feat(core,store,fonts): measurement from pinned bytes; unmeasured never blocks; pin planning`.

### Task 21: Probes for the new asset kinds — tier: sonnet

**Files:**
- Modify: `crates/fin-probe/src/lib.rs` (dispatch by magic bytes; `Probe` gains `kind` and the kind fields), `crates/fin-probe/Cargo.toml`.
- Create: `crates/fin-probe/src/{image.rs, svg.rs, lottie.rs, font.rs, lut.rs, wav.rs}`.
- Modify:
  - `crates/fin-probe/src/fixtures.rs` (`write_png`, `write_jpeg`, `write_webp`, `write_svg`, `write_dotlottie` (stored and deflated entries), `write_cube`, `write_wav`, `write_m4a`: header-valid bytes made in code, nothing committed);
  - `crates/fin-node/src/test_hooks.rs` (expose them for e2e), `crates/fin/src/backend.rs` (`Probed`), `crates/fin/src/commands/asset.rs` (`asset.add` per kind).
- Test: `crates/fin-probe/tests/probe_kinds.rs`.

**Interfaces:**
```rust
pub struct Probe { pub kind: AssetKind, pub duration: Option<Rational>, pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>, pub image: Option<ImageInfo>, pub lottie: Option<LottieInfo>,
    pub font: Option<FontFile>, pub lut: Option<LutInfo>, pub fingerprint: String, pub size: u64 }
```
Kinds:
- **PNG, JPEG, WebP:** `imagesize` reads the header only.
- **SVG:** `roxmltree` reads the root's `width`/`height`/`viewBox` (sizes in px; anything else is `UNSUPPORTED_CONTAINER`). Sanitising is sub-project 6.
- **dotLottie** (`.lottie`, zip):
  - `manifest.json` plus `animations/<id>.json`; `w`, `h`, `fr`, `ip`/`op` → `LottieInfo`;
  - slots from `slots` (id plus kind: text, color, image, number);
  - font names from `fonts.list`;
  - malformed JSON or `chars` is `UNSUPPORTED_CONTAINER` with `details.reason` (arch §27.10).
- **Font:** `fin_text::font_info` (Decision 38) for faces and scripts, plus a whole-file SHA-256 (`sha2`) → `FontFile`; TTF/OTF/TTC. This is what `fin fonts pin --asset` pins (Decision 37).
- **`.cube`:** `LUT_3D_SIZE` 2–64 with exactly size³ rows.
- **WAV:** a RIFF/`WAVE` `fmt ` + `data` parser.
- **M4A/AAC:** the existing MP4 path with no video track → `media` with audio only.

- [ ] **Step 1:** Add the crates and run `rtk cargo deny check licenses` (expected: MIT, MIT OR Apache-2.0, Zlib).
- [ ] **Step 2: Write failing tests:** one per kind from the fixture writers; `truncated_inputs_are_unsupported_container`; `unknown_magic_is_unsupported`; `lottie_slots_and_fonts_listed`; `font_probe_sha256_is_whole_file`; `font_family_matches_the_measurer` (the probed face, pinned, resolves in `faces_for` and measures) (the probed family resolves in a `FontSet` with that asset); `probe_is_deterministic`.
- [ ] **Step 3: Implement.** **Run** `rtk cargo test -p fin-probe`, then the three gates → PASS.
- [ ] **Step 4: Commit** `feat(probe): image, SVG, dotLottie, font, LUT and audio-only probes`.

### Task 22: The font manager in `fin-node`; eval and `fonts.*` RPCs — tier: sonnet

The semantics live in `fin-fonts` (Task 5) and `fin-core`/`fin-fonts::plan` (Task 20). This task wires them up.

**Files:**
- Create: `crates/fin-node/src/{fonts.rs (manager: cache, fetcher, provider over cache + asset files, triggers, status), eval.rs}`.
- Modify:
  - `crates/fin-node/src/{engine.rs, lib.rs, maintenance.rs}`, `crates/fin-node/Cargo.toml` (`fin-text`, `fin-fonts`);
  - `packages/contracts/src/rpc.ts`, `apps/finisherd/src/{handlers.ts, config.ts (fonts.autoFetch)}`;
  - `crates/fin-store/examples/export_schema.rs` (`FramePlan`, `AudioPlan`, `TextLayout`, `ParamSpec`, `FontCatalog`, `FileState`), `crates/fin/tests/rpc_contract.rs`, generated types.
- Test: `packages/fin-node/test/{eval,fonts}.test.ts`, `apps/finisherd/test/fonts.test.ts` (a loopback HTTP server and a test catalog through `FIN_FONT_CATALOG`, test-hooks build).

**Interfaces:**
- **Manager:**
  - one per server data (`<server-data>/fonts/`), shared by every engine;
  - one `Measurer` over a provider that answers from the cache, or from an open project's font asset file when its SHA-256 matches;
  - triggers per Decision 34;
  - status merges into `project.status` (`fonts {missing, fetching, failed}`, warnings `FONT_MISSING`, `FONT_FETCH_FAILED`, `FONT_UNPINNED`) and publishes `status` events on change.
- **napi:** `evalFrame`, `evalAudio`, `fontsCatalog()`, `fontsStatus()`, `fontsFetch({shas | pinned: projectId})`, `fontsPlan(projectId, branch, ops)`.
- **RPC:**
  ```ts
  Rpc.make("eval.frame",   { payload: EvalFramePayload,  success: FramePlanSchema,   error: FinError })
  Rpc.make("eval.audio",   { payload: EvalAudioPayload,  success: AudioPlanSchema,   error: FinError })
  Rpc.make("fonts.catalog",{ payload: Schema.Struct({}), success: FontCatalogSchema, error: FinError })
  Rpc.make("fonts.status", { payload: Schema.Struct({}), success: FontStatusSchema,  error: FinError })
  Rpc.make("fonts.fetch",  { payload: FontsFetchPayload, success: FontStatusSchema,  error: FinError })
  Rpc.make("fonts.plan",   { payload: FontsPlanPayload /* {projectId, branch, ops} */, success: FontsPlanSchema /* {ops, notes} */, error: FinError })
  ```

- [ ] **Step 1: Failing tests:**
  - `open_triggers_fetch_of_missing_pins`, `commit_with_new_pin_triggers_fetch`;
  - `fetch_progress_reaches_status_events`;
  - `failed_fetch_warns_with_fix_and_retries`;
  - `offline_setting_never_fetches`;
  - `asset_font_pin_measures_from_asset_file`, `changed_asset_file_is_hash_mismatch`;
  - `eval_frame_rpc_returns_layouts_and_unmeasured`, `eval_audio_rpc`;
  - `fonts_plan_rpc_returns_pin_ops`;
  - RPC fixtures.
- [ ] **Step 2: Implement.** **Run** the three gates and `rtk pnpm e2e` → PASS.
- [ ] **Step 3: Commit** `feat(node,rpc): font manager, eval and fonts RPCs`. **Batch C is done.**

### Task 23: `fin` edit commands — structure — tier: sonnet

All multi-op behaviour comes from the `fin_core::compose` builders (Tasks 8–10). This task parses arguments, calls them, and echoes.

**Files:**
- Create: `crates/fin/src/commands/{item.rs, track.rs, timeline.rs, marker.rs, bin.rs, transition.rs}`.
- Modify: `crates/fin/src/commands/{clip.rs, asset.rs, builder.rs}` (the `Builder` calls `fonts.plan` before an online commit and prepends the returned `font.pin` ops, Decision 35), `crates/fin/src/{registry.rs, map.rs, addr.rs, main.rs, output.rs}`.
- Test: `crates/fin/tests/cli_edit.rs` (new; offline snapshot hook plus a live server), `crates/fin/tests/cli_offline.rs` (map size).

**Commands** (every write takes `--dry-run`; global `--timeline <tl>`, default the first timeline):
- `fin clip insert --track --at --asset [--in] (--dur|--out) [--mode] [--no-audio] [--audio-track <t>]` (linked audio by default, Decision 21), `fin clip trim|delete` (now `item.*`);
- `fin image insert`, `fin graphic insert --template <a> [--slot id=value]…`;
- `fin item move <i> --to <time> [--track <t>] [--mode]`, `item split <i> --at <time>` (`compose::split_linked`), `item delete <i> [--mode]`, `item trim <i> (--start|--end) <delta> [--mode]`, `item slip <i> <delta>`, `item slide <i> <delta>`, `item link <i>…`, `item unlink <i|lk>`;
- `fin transition set --between <a> <b> --kind --dur [--curve] [--no-linked]` (`compose::transition_with_links`), `transition remove`;
- `fin track add|remove|move|set`, `fin timeline add|set|remove`, `fin marker add|set|remove`, `fin bin add|move|remove`, `fin asset move|remove`.

Behaviour:
- Mode defaults follow Decision 18, and every write echoes `resolved.mode` and what each builder sent.
- New optional arguments are omitted from the hashed input when absent (Decision 7).
- **Font pins:** every online write sends its ops to `fonts.plan` first and prepends the answer, echoed as `resolved.fonts` (keys and catalog entries). Offline writes plan nothing and say `measured: false`.
- **SP2 follow-ups:**
  - trim echoes the resulting edge (`resolved.edgeAt`);
  - `--out < --in` says "`--out` must be after `--in`" with both values;
  - a negative `details.max` reads "sourceIn is at or past the asset's end" with `fix` naming the asset duration;
  - `project open --dry-run` is `USAGE` instead of being ignored.

- [ ] **Step 1: Write failing tests:** per command: the dry-run JSON, a live commit, and its main refusal with exit code and `fix`. Plus:
  - `clip_insert_links_audio_by_default`, `clip_insert_no_audio`, `clip_insert_without_audio_track_is_usage`;
  - `mode_default_and_echo`, `overwrite_inside_item_composes_split`, `transition_set_crossfades_linked_audio`;
  - `args_hash_of_4b_inputs_is_unchanged` (end to end), `planned_pins_do_not_change_args_hash`;
  - `builder_prepends_planned_pins`, `offline_write_plans_no_pins`;
  - `trim_echoes_resulting_edge`, `out_before_in_wording`, `negative_max_wording`, `project_open_dry_run_is_usage`;
  - `map_is_small_and_lists_every_group` (< 1.5k tokens).
- [ ] **Step 2: Implement.** **Run** `rtk cargo test -p fin` (`FIN_REQUIRE_SERVER=1`), the three gates and `rtk pnpm e2e` → PASS.
- [ ] **Step 3: Commit** `feat(fin): structural edit commands with modes, linked audio and echoed values`.

### Task 24: `fin` commands — settings, captions, fonts, eval — tier: sonnet

**Files:**
- Create: `crates/fin/src/commands/{param.rs, keyframe.rs, effect.rs, caption.rs, style.rs, fonts.rs, eval.rs}`, `crates/fin/src/values.rs` (parse a value by `ParamType`).
- Modify: `crates/fin/src/commands/schema.rs` (`fin schema param [--scope] [--path]`), `crates/fin/src/registry.rs`, `crates/fin/src/client.rs` (keep it ≤ 600 lines; eval and fonts calls go in `client_eval.rs` and `client_fonts.rs`).
- Test: `crates/fin/tests/{cli_settings.rs, cli_captions.rs, cli_fonts.rs}`.

**Commands:**
- `fin param set <item> <path> <value>`;
- `fin keyframe set <item> <path> --at <time> <value> [--interp hold|linear|ease|easeIn|easeOut|easeInOut|bezier:x1,y1,x2,y2]`, `keyframe remove`;
- `fin effect add <item> <kind> [--at <n>] [--set p=v]…`, `effect remove|move`;
- `fin caption import --track <t> (--words <file.json>|--srt <file>) --style <s> [--at <time>]`, which expands through `captions::blocks` into one `item.insert` per block in one tx;
- `fin caption set-text|split|merge|style`, `fin style add|set|remove|ls`, `fin schema param`;
- `fin eval frame --at <time>`, `fin eval audio --range a..b` (server only);
- **`fin fonts`** (Decision 44, server only):
  - `ls [--catalog | --cached | --project]` answers compactly (per entry: family, styles, and cached/pinned flags; URLs only with `--detail`), within the map budget;
  - `fetch <key | entry | --pinned>` blocks until done, with progress on stderr; `FONT_FETCH_FAILED` is exit 5 with a fix (check the network or proxy, or `FIN_FONTS_OFFLINE`);
  - `status`;
  - `pin <family> [--weight] [--italic] [--asset <a>] [--fallback <n>]`, `repin <key> [--catalog-version <v>]`, `unpin <key>` (`FONT_IN_USE`).

`caption import`, `caption style` and `style add|set` plan their pins through `fonts.plan`, like every write (Task 23).

Values:
- numbers take an optional unit (`-6dB`, `45deg`, `120px`, `50%` for fractions);
- `x,y` for `vec2`, `#rgb`/`#rrggbb`/`#rrggbbaa` for colours, enum names, times as in `addr`.

`--words` is `[{text, start, end}]` with times as rationals or decimals, converted exactly. Offline dry runs answer `measured: false`.

- [ ] **Step 1: Write failing tests:** per command as in Task 23, plus `values_parse_by_type`, `bad_value_errors_name_the_spec_range`, `caption_import_words_and_srt_make_blocks`, `caption_import_overflow_refused_online`, `caption_import_pins_fonts_once`, `fonts_ls_fits_budget`, `fonts_fetch_failed_exits_5_with_fix` (a loopback server that answers 500), `fonts_pin_asset`, `fonts_repin_to_catalog_version`, `fonts_unpin_in_use_refused`, `schema_param_lists_the_registry`, `eval_needs_a_server`.
- [ ] **Step 2: Implement.** **Run** `rtk cargo test -p fin`, the three gates and `rtk pnpm e2e` → PASS.
- [ ] **Step 3: Commit** `feat(fin): param, keyframe, effect, caption, style, fonts, schema param and eval commands`.

### Task 25: `fin view` and `fin status` — tier: sonnet

**Files:**
- Modify: `crates/fin/src/commands/view.rs`. Create `crates/fin/src/view/{mod.rs, lines.rs, budget.rs}`.
- Test: `crates/fin/tests/cli_view.rs` with a golden `crates/fin/tests/view_golden/{project.json, full.txt, budget_200.txt}`.

**Interfaces:**
- `fin view [--timeline] [--range a..b] [--budget <tokens>]` returns `{rev, timeline, lines: [String], elided: [{step, from?, to?, items?}], tokens}`. Example lines:
  - `00:04.0–00:07.5  V1  c3 clip a1 [2.0s+3.5s] x1.5 opacity 0.8 fx: blur  ⟷ lk1`
  - `00:05.0–00:06.2  C1  cap2 "Let's go" (preset:bold)`
  - `00:06.0  ── marker m1 "drop"`
- Degrades as Decision 46 says; deterministic for a given project and budget.
- `fin view` caption lines mark unmeasured text, e.g. `(unmeasured: font missing)`.
- `fin status` returns `{project {id, name}, branch, head, lease, readOnly, lastExport, unmergedBranches: [{id, name}], fonts {missing, fetching, failed}, warnings}`. It merges the snapshot, `project.status` and `branch.list`; warnings include `FONT_MISSING`, `FONT_FETCH_FAILED` and `FONT_UNPINNED`, each with its fix. Offline, it returns `{project, branch, head, server: false}`.

- [ ] **Step 1: Write failing tests:** `view_full_matches_golden`, `view_budget_degrades_in_order_and_says_so`, `view_range_and_timeline`, `view_is_deterministic`, `view_marks_unmeasured_captions`, `status_online_fields` (with fonts), `status_offline`.
- [ ] **Step 2: Implement.** **Run** `rtk cargo test -p fin`, the three gates and `rtk pnpm e2e` → PASS.
- [ ] **Step 3: Commit** `feat(fin): screenplay view with a token budget; fuller status`. **Batch D is done.**

### Task 26: Property suite and time budget — tier: opus

**Files:** create `crates/fin-core/tests/{props_ops_4c.rs, props_rebase_4c.rs, props_history_4c.rs, common/gen4c.rs}`; modify `crates/fin-store/tests/props_replay.rs`, `crates/fin-core/tests/common/memlog.rs`, `TASKS.md`.

**Properties:**
1. `apply_then_inverse_is_exact_for_every_4c_op` (256). Every client and internal op, `strict` and the font ops included. The inverse is exact and involutive, its shifts are in reverse order (B5), and every op passes `ids_cover_every_id_in_json` (B6).
2. `rebase_over_empty_interval_is_identity` over the 4C generators (256).
3. `stale_and_fresh_agree_4c` (128).
   - Committed edits and our stale edit come from: inserts, deletes and trims in every mode (**start trims included**), cross-track moves, **slide**, split, slip, link, transitions, **keyframe set/remove**, **effect add/move**, **caption split/merge**, and **font pins** (the same pin on both sides merges).
   - When both succeed, the states are equal. A refusal is `SAME_EDGE`, `SAME_PARAM`, `REMOVED`, `FONT_PINNED` or a policy code the head explains.
4. `undo_redo_matches_reference_model_4c` (64), with cross-track ripple moves and splits.
5. `replay_and_export_import_roundtrip` (16, fin-store), with 4C ops, font pins and a font asset in the log; the follower's `stateHash` equals the writer's.
6. `apply_tx_never_panics_and_success_keeps_invariants_4c` (512), with no-shift ops past their policy limits in `Replay` mode (B1).

- [ ] **Step 1:** Write the files. Seed each bug, confirm it is caught, and revert it:
  - (a) `carve` leaves a 1-frame overlap;
  - (b) prune skips its restore;
  - (c) `split_diff` swaps op indexes;
  - (d) a move's inverse records its shifts in forward order (B5);
  - (e) content offsets are not mapped (B3);
  - (f) the replay check looks at shift tracks only (B1);
  - (g) a same-hash `font.pin` is not a no-op.
- [ ] **Step 2: Measure** every property binary:
  - `rtk proxy bash -c 'time cargo test -p fin-core --test props_ops_4c --test props_rebase_4c --test props_history_4c --test props_apply --test props_history --test rebase_props --test pick_props --test undo_to_props --test shift_map'`;
  - `rtk proxy bash -c 'time cargo test -p fin-store --test props_replay --test chain_props'`.

  The total must stay < 30 s; record it in TASKS.md.
- [ ] **Step 3:** Fix what the properties find, each fix with a regression test. Run the gates → PASS. **Commit** `test(core): 4C property suite`.

### Task 27: e2e reel scenario — tier: sonnet

**Files:** create `scripts/e2e/{reel.mjs, fontserver.mjs (a loopback server for generated fonts plus a test catalog)}`, `scripts/e2e/reel-golden/{view.json, frames.json, audio.json, frames-real.json}`; modify `scripts/e2e.mjs` (`SCENARIOS.reel`), `scripts/e2e.test.mjs`, `scripts/e2e/fixture.mjs`.

- [ ] **Step 1: Write the scenario** (spec §5.9), by `fin` only. finisherd runs with `FIN_FONT_CATALOG` pointing at a test catalog served by `fontserver.mjs` (the test-hooks build), and starts **offline**.
  1. Create a project; `track add --kind audio` (A2, music) and `--kind caption`.
  2. `asset add` an MP4 (A/V), a PNG, a dotLottie with a text slot, and a WAV.
  3. V1: three clips (one at speed 3/2) with linked audio on A1 by default. V2: an image overlay and a graphic. A2: music with a fade-in at `-6dB`.
  4. `caption import --words words.json --style preset:bold`. The write pins the style face and a fallback for the Hebrew words; `fin status` shows them `FONT_MISSING`, `fin view` shows `(unmeasured: font missing)`, and `fin eval frame` reports `unmeasured`.
  5. Go online (restart finisherd without the offline flag): the fonts fetch automatically, `fin fonts status` shows them cached, and the same `fin eval frame` now carries layouts. **The project's records are byte-identical before and after.**
  6. `keyframe set` opacity on the overlay with `--interp easeOut`; `transition set` a dissolve between two V1 clips (the linked audio gets its crossfade).
  7. Total length 30 s. `fin view` equals `view.json`; `fin eval frame` at 0, 0.5, 4.9, 12, 20 and 29.9 s equals `frames.json` (hashes); `fin eval audio --range 0..30` equals `audio.json`.
  8. An overflowing `caption set-text` is `CAPTION_OVERFLOW` (exit 3). Deleting the second V1 clip prunes both transitions, and `undo` restores them.
  9. **Real fonts** (only when `FIN_FONT_CACHE` is set; required in CI): a second project with the built-in catalog, offline, its cache seeded from `FIN_FONT_CACHE`, captions in Latin, Arabic, CJK and emoji; `fin eval frame` samples equal `frames-real.json`.
- [ ] **Step 2: Run** `rtk pnpm e2e` → every scenario passes.
- [ ] **Step 3: Commit** `test(e2e): 30-second reel built by fin; on-demand fonts; view and eval goldens`.

### Task 28: Spec and tracker — tier: sonnet

**Files:**
- `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md`:
  - §5 as built (Decisions 1–48 plus additions, codes, RPCs, commands, the header state line, §5.10 pending);
  - §3.1: DB v3 and m0003 (a version bump only); §3.2: schema version 2, the record, snapshot and diff upcasters, the aliases, per-op diffs, `float_roundtrip`; §3.4: the project-file `schemaVersion` rule (and whether 4B got the cherry-pick);
  - §4.3: per-op diff footprints, content offsets, the reverse shift order of inverses, the `SAME_PARAM` keys, replay's touched-track check;
  - §5.3: `style.*` and `font.*` ops, `strict` trims, `caption.import` as a command; §5.4: the wasm32 status;
  - **§5.1:** `fonts` stored as pins;
  - **§5.6 rewritten:**
    - the catalog, the cache, fetching, pinning by hash, unmeasured-never-blocks;
    - font assets;
    - **"bundled fonts" replaced by fetched, pinned fonts**, with arch §27.10's condition ("fonts always bundled, never from the system") restated as "always pinned bytes, never from the system";
  - §6: the crate map with `fin-fonts` and `fin-testfonts`.
  - **Deviations:**
    - no `swash`; `fin-text` written fresh;
    - `fin` neither links `fin-text`/`fin-fonts` nor measures;
    - no per-edit RPCs; dB ramps;
    - sizes measured by hand;
    - nested and mask edits moved to sub-project 6;
    - catalogs from the registry are sub-project 8.
- `docs/specs/2026-11-01-finisher-architecture-design.md`: a §27 item 15 pointer for 4C, noting that §27.10's bundled-fonts condition is now pinned fetched fonts.
- `README.md`: "Run it" gains a reel example and `fin fonts`. **"Upgrading from 4B"** states Decision 47; a "Fonts" note covers the cache location, auto-fetch, the offline switch and proxies.
- `TASKS.md`:
  - tick the 4C section and the SP2 "fin output" follow-up;
  - add the follow-ups: drop the `clip.*` aliases; nested and mask edits (sub-project 6); `fin fonts gc` and a cache size cap; registry and signed catalogs (sub-project 8); caching caption layouts; colour-emoji painting (sub-project 6); variable fonts in the catalog.

- [ ] **Step 1:** Write the edits; check each decision against the code and note in the commit body any that changed.
- [ ] **Step 2: Run** `rtk pnpm gates` → ok. **Commit** `docs: record sub-project 4C as built`.

### Task 29: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

**Files:** results in `archive/sp4c-xhost/`, conclusions in spec §5.10; create `crates/fin-store-xhost/src/edit4c.rs`, `tests/storage-xhost/analyse-edit4c.mjs` and its test; modify `crates/fin-store-xhost/src/main.rs`, `tests/storage-xhost/{run.mjs, README.md}`.

- [ ] **Step 1: Full suites** on macOS (arm64), Windows 11 (x64) and the Linux VM (x64), each with a font cache warmed by `warm_cache` on that host: `cargo test --workspace` (`FIN_REQUIRE_SERVER=1`, `FIN_REQUIRE_FONTS=1`), `pnpm test`, `pnpm gates`, `pnpm e2e`. Record the counts.
- [ ] **Step 2: Determinism with fetched fonts.**
  - On each OS: the cache files' SHA-256 equal the catalog's; the generated and real `fin-text` corpora, the eval corpus and the reel goldens (generated and real) match the committed hashes; on Linux, the wasm32-wasip1 runs match.
  - **Any mismatch blocks the merge.** Bisect it to the first differing layout or plan, fix it, add a corpus case, and re-run all three OSes.
- [ ] **Step 3: Fetch rows.** On each OS:
  - behind the host's real network: a cold fetch of the catalog;
  - with an HTTP proxy set (`HTTPS_PROXY`), and with the corporate-CA case simulated where the host allows it;
  - a kill mid-download, then a resume;
  - two `finisherd` processes on one server data fetching at once (one download).
- [ ] **Step 4: Upgrade rows:** a real 4B project per OS opens in 4C (state as upcast, `verify` ok, undo of a 4B record works) and rebuilds from the folder alone to the same `stateHash`.
- [ ] **Step 5: Mixed-version row** per NAS pair:
  - a 4B follower goes read-only at the first 4C record, with no lost records;
  - a 4C-created project on 4B is read-only (with the cherry-pick) or refused (without it); record which;
  - a 4C follower follows to the same hashes.
- [ ] **Step 6: `edit4c` row** per NAS pair, 5 runs each, the edit script pinning fonts and setting captions:
  - after one follow cycle both hosts report the same `stateHash` and the same eval plan hashes;
  - the follower fetches the pinned fonts on its own;
  - a follower kept offline reports `unmeasured` and nothing else differs.
- [ ] **Step 7: Sizes:** `fin` (no `fin-text`, `fin-fonts` or TLS) and the `fin-node` package (no fonts; the TLS stack added), recorded by hand.
- [ ] **Step 8:** Fix defects, each with a regression test in the owning crate; re-run the affected rows; record the conclusions in spec §5.10 and TASKS.md. **Commit** `test(storage): cross-host 4C rows; record 4C validation`.
