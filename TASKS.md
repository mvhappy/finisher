# Tasks

Spec: `docs/specs/2026-11-01-finisher-architecture-design.md` (§27 holds the spike conclusions).
Spike code, results and report are local-only in `archive/spike/` (git-ignored).

## Spike round 1 (2026-11-02): done

S1–S11 executed on macOS. Conclusions are in spec §27.

## Spike round 2: macOS follow-ups

- [x] S6b: caption word timing — learned boundary correction; p95 46/49 ms vs MFA; starts pass the perceptual gate, ends 91.2 % (open)
- [x] S3b: end-to-end A/V latency — audio 37 ms early uncompensated; compensated p95 7.4 ms
- [x] S11b: Lottie text — thorvg text rejected for slots; shared fin-text engine injects image slots
- [x] S5b: SVT-AV1 replaces rav1e (9–22× realtime, +1.2–1.5 dB); VP9 rt as reserve
- [x] Drop Intel Mac support (D31, §23, CI matrix)
- [x] Move spikes and evaluations to git-ignored `archive/`

## Blocked on hardware or accounts

- Windows PC (E4): S1, S3, S5, S8, S9 Windows rows
- Linux machine (E3, VM or Proxmox host): S4, S5 VA-API, S8 Landlock
- E2 GitHub repo (CI), E5 NAS, E6 sync services, E8 Apple agreement (notarisation),
  E9 Windows signing cert, own short-form speech clips (S6)

## Open from round 2

- S6: end-boundary accuracy (91.2 % within the window, needs 95 %); creator-speech check
- S3b: 240 fps phone check of panel response; Bluetooth/AirPlay output
- S11b: colour emoji in fin-text (COLRv1)
