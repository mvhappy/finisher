# Tasks

Spec: `docs/specs/2026-11-01-finisher-architecture-design.md` (§27 holds the spike conclusions).
Spike code, results and report are local-only in `archive/spike/` (git-ignored).

## Spike round 1 (2026-11-02): done

S1–S11 executed on macOS. Conclusions are in spec §27.

## Spike round 2: macOS follow-ups

- [ ] S6b: caption word timing meets p95 ≤ 50 ms (onset refinement)
- [ ] S3b: capture-based end-to-end A/V latency of the preview
- [ ] S11b: Lottie text slots and font loading in WASM vs native
- [ ] S5b: SVT-AV1 vs rav1e for the software AV1 fallback (speed, quality, licence)
- [x] Drop Intel Mac support (D31, §23, CI matrix)
- [x] Move spikes and evaluations to git-ignored `archive/`

## Blocked on hardware or accounts

- Windows PC (E4): S1, S3, S5, S8, S9 Windows rows
- Linux machine (E3, VM or Proxmox host): S4, S5 VA-API, S8 Landlock
- E2 GitHub repo (CI), E5 NAS, E6 sync services, E8 Apple agreement (notarisation),
  E9 Windows signing cert, own short-form speech clips (S6)
