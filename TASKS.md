# Tasks

Spec: `docs/specs/2026-11-01-finisher-architecture-design.md` (§27 holds the spike conclusions).
Spike code, results and report are local-only in `archive/spike/` (git-ignored).

## Spike round 1 (2026-11-02): done

S1–S11 executed on macOS. Conclusions are in spec §27.

## Spike round 2: macOS follow-ups (done)

- [x] S6b: caption word timing — learned boundary correction; p95 46/49 ms vs MFA (held out)
- [x] S3b: end-to-end A/V latency — audio 37 ms early uncompensated; compensated p95 7.4 ms
- [x] S11b: Lottie text — thorvg text rejected for slots; shared fin-text engine injects image slots
- [x] S5b: SVT-AV1 replaces rav1e (9–22× realtime, +1.2–1.5 dB); VP9 rt as reserve
- [x] Drop Intel Mac support (D31, §23, CI matrix)
- [x] Move spikes and evaluations to git-ignored `archive/`

## Spike round 3: Linux, NAS, caption ground truth

- [x] S6c: no permissive human-labelled corpus; TTS refs rejected; ElevenLabs excluded; review render of a real reel
- [x] Caption labeller tool (`archive/tools/caption-labeler/`) for owner-corrected timings
- [x] Linux VM (no GPU): S8 Landlock 90/90, S1, S5b x86 10×, S11b determinism 20/20, S9 deb/AppImage, S10 ext4
- [x] Linux iGPU (Unraid Docker): S4 WebGPU Vulkan + VA-API WebCodecs, S3 1080p60, S5 VA-API 26.7× proxies
- [x] S10 SMB single client: round-1 lease split-brains (111/200); hardened lease 200/200
- [x] S10 cross-machine race: fenced lease required (0 early takeovers, 0 stale writes); SMB defaults in §27.9
- [x] S9 notarisation — notarised and stapled under Maryna's team 9XN7WN8JN2 (API key)
- [x] Remove leftover empty NAS dirs (removed server-side 2026-11-03)

## Spike round 4: Windows (E4)

Test PC: Win 11 Pro 26300, Ryzen 7 7800X3D, RX 9070 XT + Radeon iGPU, 31 GB.
Host access details: `archive/hosts.md` (local, git-ignored).

- [x] Batch A: toolchain (`toolchain\env.ps1`), gates fixed for Windows (888bf58), S1 pass, S5b pass (17×), S11b 20/20 identical, S9 NSIS signed 17/17 PEs, S10 NTFS pass
- [ ] S10 Windows SMB: hardened lease 105/105 split brain under the PC's +9.7 s clock skew → lease must not compare local clock to server mtime (spec)
- [ ] S10(c) Windows↔Mac cross-host race (running)
- [x] Batch B: S3 pass (D3D12, 59.96 fps), S3b compensated p95 9.1 ms at 144 Hz, S5 MF hardware decode + 18.1× proxies
- [ ] Windows open: iGPU/hybrid run (per-app graphics setting), HEVC without the extensions, 240 fps phone check
- [x] Batch C: S8 AppContainer launcher; escapes 90/90 blocked; fooled policy blocked; allow-only ACLs, per-project profile, ACL broker in finisherd
- [ ] S8 Windows: Claude Code's Bash tool (Git Bash/MSYS) cannot start in an AppContainer → decide the Windows shell-tool design
- [ ] S8 Windows: rerun as a non-elevated desktop user, with real agent turns

## Captions round (S6d/S6e)

- [x] Owner labelled the reel (187 words) and the 12-clip multilingual set (1,450 words), 2026-11-03
- [x] S6d English: B0 95.7 % starts / 75.7 % ends in window vs labels; ~40 candidates; keep B0 (+ a-priori −40 ms shift); labels anchored to prefill (46/882 edges moved)
- [x] S6e multilingual: one shared stack (whisper turbo transcript forced language + base-960h CTC w/ transliteration + GBM starts); labels = prefill (reviewed by holding Enter)
- [x] Labeller blind mode (jittered prefill, play-before-review gate, Unicode scorer); owner's en/uk labels accepted as confirmed (no relabel)
- [ ] Ukrainian number verbaliser with case agreement (MIT/Rust); multilingual end-boundary corrector on permissive data
- [x] S8 Windows follow-up: non-elevated 45/45; Credential Manager denied in container; shell tool → brokered
- [x] S5 Windows HEVC via D3D11VA direct (no MF extension): 1,500/1,500 frames exact, ~1,090 fps

## Sub-project 2: minimal core (done, merged 2026-11-04)

Plan: `docs/superpowers/plans/2026-11-04-sub-project-2-minimal-core.md`.

- [x] Tasks 0–5: spec decisions, `Rational` time, model/tx/errors with generated TS types, ops `asset.add`, `clip.insert`, `clip.trim`, `clip.delete`, rebase
- [x] Tasks 6–9: `fin-store` SQLite op log, `fin-probe`, `fin-node` napi binding, `@finisher/contracts` RPC group
- [x] Tasks 10–12: `finisherd` (socket/pipe + bearer WebSocket, live op stream), `fin` CLI offline and online
- [x] Task 13: `pnpm e2e` (fin → finisherd → WebSocket subscriber), README "Run it", CI runs e2e
- [x] Cross-platform: `cargo test --workspace` (FIN_REQUIRE_SERVER=1), `pnpm test`, `pnpm gates`, `pnpm e2e` pass on macOS, Windows 11 and the Linux VM
- [x] Windows fixes from the first run: per-server-data named pipe, fin timeouts on named pipes, probe path-through-a-file code, Windows-portable tests
- [x] Final whole-branch review; fix wave (idem-key reuse → `IDEM_KEY_REUSED`, `PROJECT_DIR_UNWRITABLE`, `CANCELLED` exit 5, empty tx rejected, clock field ranges, rustfmt gate); merged to `main`

### Follow-ups for sub-project 4 (core breadth)
- Lease: implement spec §27.9 "v3" (in-place checksummed record, missing+claims = held, server-time staleness with periodic re-probe); validate with Linux clients, network loss and real skew
- Store open replays the tail through the current `fin-core` and fails `DB_ERROR` on any diff/schema change: add migrations and rebuild-from-segments **before any op or model change**
- `project create`: idempotent retry, reject empty `--name`, atomic create (orphan db when the JSON write fails, check-then-rename race)
- `ops.subscribe` with `fromRev` > head should return `BASE_REV_AHEAD`
- Typecheck gate (`tsc`) for all TS packages, tests included (`@types/node`)
- Idempotency: `tx.get` RPC (asset add retry after the file moved) and a stored-args hash (same key, same op kind, different args)
- Rebase: per-track (not per-tx) conservatism; stale insert vs committed `asset.remove` → `CONFLICT`; stronger proptest oracle (positions, multi-track); `clip.restore` placeholders
- fin output: trim echoes the resulting edge; `project open` ignores `--dry-run`; `--out < --in` fix wording; negative `details.max` text when `sourceIn` ≥ asset duration
- Rational parsing: reject non-canonical input ("2/4", "-0"); `details.rev` should stay numeric
- `fin-node`: Linux loader libc detection; libuv pool use under many projects; hubs never removed in finisherd
- Generate `Probe` TS type from `fin-probe`; MOV fixture test

### Follow-ups for sub-project 5 (control plane, auth)
- Windows: named-pipe DACL and pipe-squatting protection; server-data ACL check; graceful stop (no SIGTERM)
- RPC for non-`fin` clients: decode failures and unknown tags as `FinError` (not `Die`); reject relative paths server-side (`INVALID_PATH`)
- Client hardening: overall deadline and frame-size cap; test hooks (`FIN_TEST_COMMIT_GATE`, `FIN_OFFLINE_SNAPSHOT`, `FIN_NODE_BINARY_DIR`) out of release builds
- `childEnv`: pass provider auth/config dir variables when providers arrive

## Blocked on hardware or accounts

- Linux real desktop (vsync, Wayland/X11) and Linux S3b latency
- E2 GitHub repo (CI), E6 sync services, E9 Windows signing cert

## Open

- D33 i18n-ready from day one (spec §17.1): catalogue library choice in sub-project 4

- S6: end-boundary accuracy; spell out digit words before CTC
- S3b: 240 fps phone check of panel response; Bluetooth/AirPlay output
- S11b: colour emoji in fin-text (COLRv1)
- Cleanup when done: `~/finisher-spike` on the Linux VM (11 GB) and its `finisher-spike/gfx:1` image
