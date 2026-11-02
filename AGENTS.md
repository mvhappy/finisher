# Finisher agent rules

- Prefix shell commands with `rtk`.
- Conventional Commits.
- Pin GitHub Actions to commit SHAs.
- No GPL/LGPL/AGPL/SSPL/EUPL dependencies in shipped code.
- Primitive ops live only in `fin-core`, one module per op.
- TypeScript model types are generated, never hand-written.
- Non-test source files ≤ 600 lines, tests ≤ 1500.
- No cloud-only imports in this repository.
- The spec is `docs/specs/2026-11-01-finisher-architecture-design.md`.
- Spike code lives only on the `spike` branch.
