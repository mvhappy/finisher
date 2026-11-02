# Finisher

Finisher is an agent-first video editor for creators. Point it at a media library
(local disk, NAS, or a cloud-synced folder) and let agents build edits on branches
that you review and merge.

Status: pre-alpha. The architecture is in
[docs/specs/2026-11-01-finisher-architecture-design.md](docs/specs/2026-11-01-finisher-architecture-design.md);
the current plan is in [docs/plans/](docs/plans/).

## Development

```bash
rtk corepack enable
rtk pnpm install
rtk cargo test --workspace
rtk pnpm test
rtk pnpm gates
```

## License

MIT. See [LICENSE](LICENSE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
