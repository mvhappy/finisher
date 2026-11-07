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

Lints: `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings`, and also
with `--all-features`; both are clean. Every build of the workspace compiles SVT-AV1 (`fin-svt-sys`,
needs cmake and, on x86_64, nasm) and rav1d (needs nasm): without nasm on `PATH` they fail in
their build scripts, not in clippy. A check-only run can skip the SVT-AV1 C build with
`FIN_SVT_CHECK_ONLY=1`, but rav1d still needs nasm, so install it (`apt install nasm`, or extract
the `.deb` into a user directory and add its `usr/bin` to `PATH`).

## Run it

Sub-projects 2, 4A, 4B and 4C ship the core with durable storage, history and the full edit model: `finisherd` (the local server)
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

**A short reel** (sub-project 4C; `pnpm e2e` builds a 30 s one by `fin` only). A clip of an
audio-and-video asset on a video track gets a linked audio item on the first audio track:

```bash
fin track add --kind video                          # V2, binds t3 (overlays)
fin track add --kind caption                        # C1, binds t4
fin asset add ~/Movies/logo.png                     # an image: binds a2 (kind echoed)
fin clip insert --track t1 --at 0 --asset a1 --dur 4s           # c1 on V1 + linked audio c2 on A1
fin clip insert --track t1 --at end --asset a1 --in 4s --dur 3s # c3 + c4
fin image insert --track t3 --at 1s --asset a2 --dur 2s         # img1
fin keyframe set img1 opacity --at 0 0 --interp easeOut
fin keyframe set img1 opacity --at 1s 1
fin transition set --between c1 c3 --kind dissolve --dur 1s     # the linked audio gets a crossfade
fin caption import --track t4 --words words.json --style preset:bold   # [{"text","start","end"}]
fin view --budget 400                                # screenplay view, elided to fit the budget
fin status                                           # timelines, fonts, unmeasured captions, warnings
fin eval frame --at 2s                               # the frame plan (needs finisherd)
fin eval audio --range 0..7s
fin fonts status                                     # every font file's state on this server
fin fonts ls --pinned                                # this project's pins, their state and users
```

Every edit group has a command (`fin help <group>`; `fin schema param` lists every setting).
Structural edits take `--mode` (`ripple` is the default for clips, `overwrite` for other kinds,
`lift` for deletes, `roll` for trims), and every write echoes the mode and values it used.

**Fonts.** No fonts are bundled. A project pins the exact font files its captions use (by
SHA-256), from Finisher's pinned catalog (Inter, Noto Sans, Noto Sans Arabic, Hebrew and CJK,
Noto Color Emoji; all OFL-1.1) or from a font asset (`fin fonts pin <family> --asset <a>`).
`fin` asks the server to plan the pins a caption or style edit needs and sends them with the edit.
`finisherd` then downloads pinned files it lacks on its own, from the catalog's upstream URLs at
pinned commits, into `fonts/` in server data (shared by every project; each file verified by
SHA-256). Until a file arrives, captions that use it are unmeasured (`fin status` warns
`FONT_MISSING`); edits are never blocked, and nothing is rewritten when it arrives.
- Offline: set `{"fonts": {"autoFetch": false}}` in `<server-data>/settings.json` (read while the
  server runs) or start `finisherd` with `FIN_FONTS_OFFLINE=1`; then only `fin fonts fetch
  --pinned` (or `fin fonts fetch <key|entry>`) downloads. Cached fonts work either way.
- Proxies: `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY`, and the Windows system proxy.
  Certificates are checked against the OS trust store, so a corporate CA works.

**Upgrading from 4B.** 4C's model and op schema are version 2, and its database is version 3:
- the first edit a 4C server makes turns 4B followers of that project read-only (`NEWER_FORMAT`);
- a project created by 4C opens read-only on 4B;
- a 4B server refuses a database that 4C has migrated (`DB_TOO_NEW`); there is no downgrade;
- older projects open in 4C with the same state and pin no fonts until their first caption or
  style edit.

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

## Privacy and diagnostics

Finisher keeps a local journal of what it does, for two purposes: improving the app
(**diagnostics**) and, only if a person opts in, training an open-weight video-editing model
(**training**). Nothing is uploaded: this version has no uploader and no network path for it.
The design is in §21.1 of the [architecture spec](docs/specs/2026-11-01-finisher-architecture-design.md).

**The consent model** (owner decisions, 2026-11-07):

- **Basic diagnostics are on by default, with an opt-out.** This is metadata only: error codes,
  timings, counts, ids, keyed hashes, the commands and flag names you ran (never their values),
  and what became of each edit (accepted, rebased, deduped, undone, conflicted…). Basic
  diagnostics never hold content, paths, host names or user names.
- **Training content is opt-in only, and only a person can opt in.** It adds edit intents (the
  ops of each transaction), error messages and, later, captions, prompts, model conversations,
  frames and audio. Until someone opts in, training is `undecided` and no content is recorded.
  An agent's `fin` cannot turn it on.
- **Closed-model outputs may become training targets, pending counsel.** Every agent turn is
  recorded with its model's provenance. No training run or upload may use closed-model outputs
  until counsel has reviewed the providers' terms and a person has recorded the result. Until
  then the gate stays at `pendingCounsel`. The dataset builder and any upload path (neither
  exists yet) must drop those examples while it does.

**Where it lives.** `<server-data>/events/`: dated JSONL files (`j-<date>-<id>.jsonl`), `blobs/`
for large content, `key` (the per-install key that keys every hash and non-ULID id; `0600`)
and `state.json`. Only `finisherd` writes the journal. `fin` sends its `cli.command` entries
to the server, and records nothing when no server is running. Files rotate at 8 MiB or once
a day. Entries are kept for at most 30 days and 128 MiB, blobs included
(`privacy.retentionDays` and `privacy.maxMiB` in `<server-data>/settings.json`). With
everything off, no `events/` directory is created at all.

**The switches**, strongest first:

| Switch | Effect |
|---|---|
| `DO_NOT_TRACK=1` in `finisherd`'s env | Both purposes off. `fin` with `DO_NOT_TRACK` set sends nothing. |
| `FIN_DIAGNOSTICS=on\|off`, `FIN_TRAINING=on\|off` in `finisherd`'s env | Override the settings (`basis: env`). Off wins over on. |
| `finisher.project.json` `"collect": "never"` | Records nothing about this project on any collaborator's server, and purges what each server had once it sees the flag. |
| `<server-data>/settings.json` `privacy.diagnostics`, `privacy.training` | `on` or `off` (`basis: user`). An absent key takes the default: diagnostics on, training undecided. |

**How to opt in to training:** answer the onboarding question in the Finisher app (coming in a
later version), set `"privacy": {"training": "on"}` in `<server-data>/settings.json` by hand
(the running server picks it up within a few seconds), or start `finisherd` with
`FIN_TRAINING=on` on your own machine. `fin diagnostics consent training on` is refused on
purpose, with a message that names these three ways.

**See it, change it, delete it:**

```bash
fin diagnostics show                          # consent and its basis, project flags, files, sizes, counts per kind, drops
fin diagnostics export --limit 20             # the newest 20 entries, as JSONL (content included: it is your data)
fin diagnostics export --kind log.commit --since 2026-11-01
fin diagnostics export --as-upload diagnostics   # exactly what a future uploader could send now
fin diagnostics consent diagnostics off       # or on, or unset (back to the default)
fin diagnostics consent training off          # withdraw; unset makes it undecided again
fin diagnostics project never                 # this project: record nothing, purge its entries (default: follow the settings)
fin diagnostics purge --project <id>          # or --before 2026-10-01, or --all (also removes the key and every blob)
```

Withdrawing a purpose takes effect before anything else is written. It stops the upload view
from offering anything recorded under that purpose. It deletes nothing by itself: use
`fin diagnostics purge` for that.

## License

MIT. See [LICENSE](LICENSE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
