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
- Spikes, evaluations, benchmarks and scratchpads live in `archive/` (git-ignored). Only durable conclusions go into the spec.
- Evaluation data must be permissively licensed too: no NC or share-alike corpora.
- Every dependency (a Rust crate or an npm package, new or updated) must be published ≥7 days before it's locked; run `pnpm check:dep-age` (or `check:crate-age` or `check:npm-age` alone) before merging dependency changes.
