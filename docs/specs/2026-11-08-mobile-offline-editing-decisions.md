# Mobile app with limited offline editing: decisions

Owner decisions, 2026-11-08. This is a decision note, not a design. The design is **Sub-project 12: mobile core and offline editing**, a later sub-project that is not planned yet. It supersedes the bare "mobile" entry in the "After v1" row of §24 of the architecture spec.

## 1. Decisions

1. **Native iOS and Android apps.** They share the Rust core (`fin-core`, `fin-store`; the project format is SQLite). They use the OS's own decoders and encoders: VideoToolbox/AVFoundation on iOS, MediaCodec on Android.
2. **Without a server, the phone does simple editing only.** Allowed offline:
   - import from the camera roll;
   - trim, split, reorder, delete, ripple;
   - simple transitions: cut, dissolve, fade;
   - typed text overlays and manually typed captions, with built-in styles and bundled or pinned fonts only;
   - volume, fades, one music track;
   - simple adjustments: brightness, crop, rotate, speed;
   - undo/redo and full history;
   - export at phone resolution with the OS encoder;
   - preview playback.
3. **Server-only** (the phone needs a server for these):
   - AI agents and prompt editing;
   - transcription and automatic captions;
   - library indexing (NAS, cloud folders);
   - plugins, especially code plugins;
   - masks, mattes, advanced effects, colour grading;
   - font fetching and plugin caption styles;
   - heavy exports (4K, many layers, other codecs);
   - review workflow and sharing;
   - diagnostics and training-data upload.
4. **Sync.** Phone edits are a branch. On reconnect the branch syncs and merges like any other branch (4B history), with conflicts shown as usual. Footage shot on the phone uploads to the server.
5. **Server projects offline.** "Make available offline" downloads small preview copies (6B-1 proxies). The phone edits against them, and the final full-quality render happens on the server.

## 2. Implications (follow-ups for the owning plans)

- **5B (pairing, device principal):** the device principal and pairing already cover phones. Sync needs a device-scoped project/branch sync RPC (push/pull of branches, media upload). **It is not in 5B yet; follow-up.**
- **6B-1 (proxies, VideoToolbox):** the VideoToolbox work is reusable on iOS. **Follow-up:** add a small phone-sized proxy tier to the offline downloads. The capability-URL media fetch suits downloads.
- **Determinism:** phone renders must match the server (T0/T1 on mobile GPUs: Apple GPU, Adreno, Mali). **Follow-up:** add mobile to the cross-platform matrix later.
- **Builds:** `fin-core` and `fin-store` must build for `aarch64-apple-ios` and `aarch64-linux-android`. Known blockers and risks to check first:
  - native dependencies: SQLite (bundled build per target) and the dav1d assembly (per-target toolchain flags);
  - SVT-AV1 is not needed on phones, because phones do not encode AV1; keep it out of the mobile build.
- **Storage and battery:** limits for offline downloads and exports must be visible to the user (space used, a cap, charging or battery state before long work).
- **Placement:** the frontend UI belongs to sub-project 9. The mobile core (Rust on iOS/Android, sync, OS codec backends) becomes the new **Sub-project 12: mobile core and offline editing**, designed later. Its scope is the decisions above. It is not planned yet.
