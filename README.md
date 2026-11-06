# Finisher

Finisher is an agent-first video editor for creators. Point it at a media library
(local disk, NAS, or a cloud-synced folder) and let agents build edits on branches
that you review and merge.

Status: pre-alpha. The architecture is in
[docs/specs/2026-11-01-finisher-architecture-design.md](docs/specs/2026-11-01-finisher-architecture-design.md);
the current plan is in [docs/plans/](docs/plans/).

## Development

```bash
rtk npm install -g pnpm@12.15.0   # Node 26 ships no corepack; CI does the same
rtk pnpm install
rtk cargo test                    # default members; the cross-host harness: cargo build -p fin-store-xhost
rtk pnpm test
rtk pnpm gates
rtk pnpm e2e    # slow: builds fin and fin-node, then runs the end-to-end check
```

## Run it

Sub-projects 2, 4A and 4B ship a minimal core with durable storage and history: `finisherd` (the local server)
and `fin` (the agent CLI, JSON on stdout). Build both, start the server, then edit through `fin`.
Node 26, pnpm 12.15.0 (`npm install -g pnpm@12.15.0`) and the Rust toolchain from
`rust-toolchain.toml` are required.

macOS / Linux:

```bash
pnpm install
cargo build --release -p fin
pnpm --filter @finisher/fin-node build

# Terminal 1: the server (logs on stderr; Ctrl+C stops it cleanly)
node apps/finisherd/src/main.ts

# Terminal 2: the CLI
export PATH="$PWD/target/release:$PATH"
fin project create ~/Movies/demo --name Demo        # video track t1, audio track t2
cd ~/Movies/demo
fin asset add ~/Movies/clip.mp4                     # binds a1
fin clip insert --track t1 --at 0 --asset a1 --dur 4s
fin clip insert --track t1 --at end --asset a1 --in 4s --dur 3s
fin view
```

Windows (PowerShell):

```powershell
pnpm install
cargo build --release -p fin
pnpm --filter '@finisher/fin-node' build

# Window 1: the server (Ctrl+C stops it)
node apps\finisherd\src\main.ts

# Window 2: the CLI
$env:PATH = "$PWD\target\release;$env:PATH"
fin project create "$HOME\Videos\demo" --name Demo
cd "$HOME\Videos\demo"
fin asset add "$HOME\Videos\clip.mp4"
fin clip insert --track t1 --at 0 --asset a1 --dur 4s
fin clip insert --track t1 --at end --asset a1 --in 4s --dur 3s
fin view
```

`clip.mp4` must be at least 7 s long: the second insert reads the source from 4 s to 7 s
(a shorter file fails with `INVALID_RANGE`). Any 7 s+ video works; `pnpm e2e` uses a 10 s one.

Run `fin` with no arguments for the command map, and `fin help clip` for details.

**Branches and history** (every command takes `--branch <b>`, default `main` or the one
`fin branch default` / `fin try` set; undo stacks are per `$FIN_AUTHOR`, default `fin`):

```bash
fin branch new feature                              # fork the current branch at its head
fin --branch feature clip trim c1 --end +1s         # edit on the branch
fin branch ls                                       # branches, heads and states (--all: archived too)
fin merge feature                                   # into the current branch; CONFLICT (exit 4) lists conflicts
fin merge feature --skip-conflicting                # merge the rest, leave conflicting records on the branch
fin undo                                            # undo your last edit or merge (fin redo reverses it)
fin checkpoint before-cut                           # name the head; fin undo --to before-cut
fin cherry-pick <txId>                              # copy one record; fin revert <mergeId> undoes a merge
fin try                                             # a throwaway branch; fin try keep | fin try drop
fin branch rename feature main-cut                  # fin branch archive <b> [--restore [--as <new>]]
```

Every write takes `--dry-run` (it works on a follower too) and `--idem-key <key>`.

**Project storage** (run in the project folder, like the other commands):

```bash
fin project status                   # lease, read-only reason, branches, export, warnings
fin project takeover [--confirm]     # take the lease from a stopped server (--confirm on sync folders)
fin project verify                   # check database and folder, change nothing (VERIFY_FAILED, exit 3)
fin project rebuild                  # rebuild the database from the folder (works with no server running)
fin project compact                  # pack old segment files now (history is kept)
fin project recover                  # list set-aside records; the manual steps out of CHAIN_FORK / CONFLICT_COPY
fin project recover --create all     # bring set-aside records back as recovered-* branches
fin project recover --import <dir>   # read segment files moved out of the folder (after CHAIN_FORK)
fin project acknowledge --rewind 2 --branch main   # clear a REWOUND / SPLIT_BRAIN_SUSPECTED warning (--branch is global)
fin project acknowledge --salvage project.fdb.corrupt-<ts>   # clear SALVAGE_INCOMPLETE
fin project open <dir> --relocate    # the project moved and its old folder cannot be reached
```

A second server (its own server data, e.g. another machine on the same NAS share) that opens the
same folder follows read-only (`readOnly.reason` `LEASE_HELD`) and takes over automatically once
the first one has been stopped for about a minute; on a sync folder only `fin project takeover
--confirm` does. `fin project status` explains any read-only state and its fix.

**Server data** (the project databases, `finisherd.json` with the server's pid, socket and
WebSocket port, and the lock file) lives in:

- macOS: `~/Library/Application Support/Finisher`
- Linux: `${XDG_DATA_HOME:-~/.local/share}/finisher`
- Windows: `%LOCALAPPDATA%\Finisher`

Set `FIN_SERVER_DATA` (read by both `finisherd` and `fin`), or pass
`finisherd --server-data <dir>`, to use another directory. The live database always stays in
server data. The project folder holds `finisher.project.json` and `.finisher/`: the lease and a
write-once, hash-chained export of every edit (`segments/`), from which any server can rebuild
the database. A project folder can therefore live on a NAS or in a sync folder and be opened
by several servers: one writes, the others follow read-only.

**Stop the server** with Ctrl+C in its terminal, or from elsewhere:

```bash
kill "$(node -p 'require(process.argv[1]).pid' "$HOME/Library/Application Support/Finisher/finisherd.json")"   # macOS; adjust the path on Linux
```

```powershell
Stop-Process -Id (Get-Content "$env:LOCALAPPDATA\Finisher\finisherd.json" | ConvertFrom-Json).pid
```

`kill` (SIGTERM) shuts down cleanly. Windows has no SIGTERM: `Stop-Process` ends the
process at once and leaves `finisherd.json` behind; `fin` then reports `SERVER_UNAVAILABLE`,
and the next `finisherd` start replaces the file.

`pnpm e2e` runs the whole flow (`fin` → `finisherd` → a WebSocket subscriber) in a temp
directory.

## License

MIT. See [LICENSE](LICENSE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
