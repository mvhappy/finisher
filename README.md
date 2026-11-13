# Finisher

Finisher is an agent-first video editor for creators. Point it at a media library
(local disk, NAS, or a cloud-synced folder) and let agents build edits on branches
that you review and merge.

Status: pre-alpha. The architecture is in
[docs/specs/2026-11-01-finisher-architecture-design.md](docs/specs/2026-11-01-finisher-architecture-design.md);
the current plan is in [docs/plans/](docs/plans/).

## Development

```bash
rtk npm install -g pnpm@12.15.0   # CI installs pnpm the same way
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

Secrets: a server keeps its identity key and the data key of stored provider keys in the OS
keychain when it answers without a prompt, else in files in its private data folder
(`FIN_SECRETS=auto|keychain|file`; `FIN_SECRETS_KEY_FILE` encrypts the files). The tests, the
e2e harness and `fin`'s test support always run servers with `FIN_SECRETS=file`, and a local
`.superpowers/env.sh` (git-ignored) should export it too: unsigned macOS builds prompt on every
keychain access.

## Run it

Sub-projects 2, 4A, 4B and 4C ship the core with durable storage, history and the full edit model: `finisherd` (the local server)
and `fin` (the agent CLI, JSON on stdout). Build both, start the server, then edit through `fin`.
Node 24, pnpm 12.15.0 (`npm install -g pnpm@12.15.0`) and the Rust toolchain from
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
fin view --budget 400                                # screenplay view; the whole JSON answer fits ~400 tokens
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
fin merge feature --include <txId>[,<txId>…] --up-to <seq>  # merge only what was reviewed; the rest is notIncluded
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
fin project takeover                 # take the lease from a stopped server (a sync folder needs the console's --confirm)
fin project verify                   # check database and folder, change nothing (VERIFY_FAILED, exit 3)
fin project rebuild                  # rebuild the database from the folder (works with no server running)
fin project compact                  # pack old segment files now (history is kept)
fin project recover                  # list set-aside records; the manual steps out of CHAIN_FORK / CONFLICT_COPY
fin project recover --create all     # bring set-aside records back as recovered-* branches
fin project recover --import <dir>   # read segment files moved out of the folder (after CHAIN_FORK)
fin project acknowledge --rewind 2 --branch main   # clear a REWOUND / SPLIT_BRAIN_SUSPECTED warning (--branch is global)
fin project acknowledge --salvage project.fdb.corrupt-<ts>   # clear SALVAGE_INCOMPLETE
```

A second server (its own server data, e.g. another machine on the same NAS share) that opens the
same folder follows read-only (`readOnly.reason` `LEASE_HELD`) and takes over automatically once
the first one has been stopped for about a minute; on a sync folder only a confirmed takeover
does, which is yours to make from the server's console (`finisherd project takeover <dir> --confirm`).
Likewise a project whose old folder cannot be reached is reopened from its new folder with
`finisherd project open <dir> --relocate`. `fin project status` explains any read-only state and its fix.

**Server data** (the project databases, `finisherd.json` with the server's pid, socket and
WebSocket port, and the lock file) lives in:

- macOS: `~/Library/Application Support/Finisher`
- Linux: `${XDG_DATA_HOME:-~/.local/share}/finisher`
- Windows: `%LOCALAPPDATA%\Finisher`

Set `FIN_SERVER_DATA` (read by both `finisherd` and `fin`), or pass
`finisherd --server-data <dir>`, to use another directory. The live database always stays in
server data, which must be on this computer's own disk: a network share, a sync folder or a
container's overlay filesystem is refused (`SERVER_DATA_NOT_LOCAL`; in Docker, mount a volume)
unless you set `FIN_ALLOW_NONLOCAL_SERVER_DATA=1`. The project folder holds `finisher.project.json` and `.finisher/`: the lease and a
write-once, hash-chained export of every edit (`segments/`), from which any server can rebuild
the database. A project folder can therefore live on a NAS or in a sync folder and be opened
by several servers: one writes, the others follow read-only.

**Stop the server** with Ctrl+C in its terminal, or from anywhere with `finisherd stop`
(`node apps/finisherd/src/main.ts stop`, with the same `--server-data` if you passed one). It
stops gracefully on every OS, Windows included, and needs no terminal, so a service manager
can run it. `kill` (SIGTERM) also shuts down cleanly on macOS and Linux.

`pnpm e2e` runs the whole flow (`fin` → `finisherd` → a WebSocket subscriber) in a temp
directory.

## Reviews

An agent's branch goes through a review that you decide, one transaction at a time. What you
decide binds every later merge: a merge copies only what was reviewed, and a rejection is kept
in the project's history, so it holds on every server that opens the project.

```bash
fin --branch feature review request --title "Tighten the intro" [--into main] [--summary "…"]
fin review ls [--state open] [--kind branchReview]   # kinds: branchReview, mergeConflict, pluginSuggestion
fin review show <id>                                  # JSON: the change list, your marks, comments, "stale"
fin review comment <id> "too slow here" [--tx <txId>]
fin review decide <id> --accept <txId> --accept <txId> --reject <txId> [--pending <txId>] [--reason "…"]   # or --accept a,b
fin review decide <id> --request-changes --reason "…"   # hand the branch back to its agent
fin review refresh <id>                               # re-plan at the current heads; your marks stay
fin review merge <id> [--accept-pending]              # copy the accepted transactions (and the pending ones with the flag)
fin review close <id> --rejected | --dismissed        # --rejected keeps every unmerged change out of later merges
```

- **Requesting.** A review lists the branch's transactions that `main` (or `--into`) does not
  hold yet, with a status each (`willMerge`, `dropped`, `dependsOnSkipped`, `conflict`, …). One
  review is open per branch pair; asking again returns it. Review text (titles, summaries,
  comments) stays on the server and is never audited or sent to diagnostics.
- **Deciding.** `accept`, `reject` or leave `pending`. Every `decide` and `merge` sends the
  snapshot it saw, so a view that is out of date is `REVIEW_STALE` (exit 4) with the fresh list;
  `fin review refresh` moves the snapshot and keeps your marks. A rejection writes a `drop`
  record on the target branch at once, before anything else. `--accept` brings a rejected
  transaction back (a drop someone else made needs `review.decide` on the target); `--pending`
  lifts the review's own rejection.
- **Merging.** `fin review merge` copies exactly the accepted transactions, up to the
  snapshot: anything the branch gained after the review is not merged by it. Pending
  transactions are left out (the review closes as merged, and you review the rest with a new
  request) unless you pass `--accept-pending`; when the project requires review, any pending
  one stops the merge (`REVIEW_UNDECIDED`). With nothing accepted and transactions still open,
  the merge is `REVIEW_UNDECIDED` too. A conflict is `REVIEW_HAS_CONFLICTS` and nothing is
  written. If the target moved since the snapshot, the merge still goes ahead on what was
  reviewed and says `targetMoved`. An agent's branch is archived once nothing of it is left.
- **Merge conflicts** from any `fin merge` appear as `mergeConflict` items, and a plugin's
  suggestion as a `pluginSuggestion` item; close those with `fin review close`.
- **Server moves.** Rejections travel with the project. Open reviews and their comments and
  marks live in the server's own data: on another server, open agent branches get a fresh
  review, marked re-derived.

```bash
fin drop <txId>… --from feature [--into main]         # keep changes out of every merge into a branch, without a review
fin drop --undo <txId>… --from feature [--into main]  # bring them back
fin merge feature --include <txId>… --up-to <seq>     # merge only what you name, up to a record on the branch
```

A plain `fin merge` leaves a dropped transaction out (`excluded`, reason `DROPPED`), and a
cherry-pick of it is still allowed.

**Protected branches.** `main` is protected by default: a caller without the
`branch.protected` scope cannot write it, and the server refuses again inside the write
transaction itself (`BRANCH_PROTECTED`, exit 7). `fin` run by you keeps the scope, so you can still
edit `main` directly; agent and MCP credentials do not have it, and `review merge`, `review
decide`, `review close` and `fin drop` into a protected branch need `review.decide`, which
only you hold by default. Plugin commands commit as whoever runs them, so the same rule binds
them: an MCP credential's command into protected `main` is `BRANCH_PROTECTED`. The policy is yours alone, set at the server's console:

```bash
finisherd review policy show --project <id> [--json]
finisherd review policy set --project <id> --protect release --unprotect old-line   # branch names
finisherd review policy set --project <id> --require-review on                       # pending never merges
finisherd review policy set --project <id> --auto-merge on                           # refused: AUTO_MERGE_UNAVAILABLE
```

`--protect` and `--unprotect` take branch names (or ids), may be repeated, and add to or remove
from the project's list as it is when the change lands; the console asks `Proceed? [y/N]` first, like the other human-only actions. Automatic merging by
policy is built but switched off: turning it on is refused until the QA gate and critic
(sub-project 10) can vouch for a change.

## Remote access

Off by default: the server listens only on `127.0.0.1` and its local socket or pipe. Turn it on
from the server's console:

```bash
finisherd remote enable                                   # 0.0.0.0:7311, a self-signed certificate
finisherd remote enable --listen 0.0.0.0:7311 --cert <cert.pem> --key <key.pem>   # your own certificate
finisherd remote enable --public-url https://<your-domain>                       # behind a reverse proxy
finisherd remote status                                   # what runs, where, and its pin
finisherd remote disable                                  # closes the listener; devices stay paired
```

Off this computer it is TLS only (`LISTEN_NEEDS_TLS` otherwise). `--tls self-signed` (the
default) makes a certificate from the server's identity key; the Finisher app checks it by its
pin, which comes with the pairing link. **Browsers cannot check a pin**, so they need a route whose
certificate they trust (`--cert`/`--key` files, for example from `tailscale cert`, re-read when
they change, or a reverse proxy) or an SSH tunnel to the loopback listener. The tunnel must use
the same port number on both ends, because the server checks the port in `Host`: fix the
loopback port with `{"server": {"loopback": {"port": 7310}}}` in `<server-data>/settings.json`,
restart, then `ssh -L 7310:127.0.0.1:7310 <server>`. The first listen off loopback brings up the
macOS or Windows firewall prompt.

**Pairing a device.** `finisherd pair` (`--label <text>`, `--ttl 30s` to `10m`, default 5
minutes) prints a one-time link and, when it fits the terminal, a QR code. Open the link or scan
the code on the device. The console then shows the device's name, platform and a six-digit
safety code; approve only if the code matches the one the device shows. A link works once: if
the device says it was already used, someone else redeemed it, so reject that request and pair
again. Treat a link as a password, and do not paste it into chats.

```bash
finisherd devices                  # paired devices (--all for ended ones, --json)
finisherd revoke <device-id>       # ends it now, with its connections (type its name to confirm)
finisherd tls show                 # the identity's pin, in groups of four, and the routes
finisherd tls rotate               # a new identity: every device pairs again (type the environment's name)
finisherd env show                 # the environment devices pair with
finisherd env name <name>          # the name devices show for it
```

**Moved or copied server data.** When the server data turns up on another machine or in
another folder, devices are refused (`ENV_MOVED`) and the remote listener stays off until you
decide: `finisherd env adopt` if it was a move (devices are let in again), or `finisherd env
reset` if it is a copy that will run beside the original (a new identity, devices deleted,
agent and MCP credentials revoked; type the environment's name).

**Docker and services.** `FIN_LISTEN=<host:port>` runs the remote listener whatever
`settings.json` says, self-signed unless `FIN_TLS_CERT` and `FIN_TLS_KEY` name certificate
files. In a container the server cannot see the host's addresses, so set
`server.remote.publicUrl` (an `https` URL) in `settings.json`, or run Tailscale inside the
container, for devices to get a usable route. `finisherd remote status` (or `finisherd status --json`) says whether
the listener came from `env` or `settings`.

**Where the keys live.** The identity key and the data key of stored provider keys go to the OS
keychain (macOS Keychain, Windows Credential Manager, the Secret Service on Linux) when it
answers without a prompt. Otherwise, as on a headless Linux box, a locked Secret Service or in
a container, they go to files in `<server-data>/secrets/` (owner-only), and `finisherd status`
warns `SECRETS_FILE_STORE`. `FIN_SECRETS=auto|keychain|file` chooses (default `auto`; a server
keeps the store it used first). `FIN_SECRETS_KEY_FILE` names a file of 32 random bytes in
base64url (a Docker secret, for example) that encrypts those files; without it they are plain
files protected only by their folder.

## Security model

**Who can talk to the server.** Every connection proves who it is before the server does
anything for it, and the server proves itself first, so a program that took over the socket
or pipe name gets nothing. There are five kinds of callers today:

- **`local`**: `fin` and scripts you run. They prove with `<server-data>/auth/local.key`.
- **`console`**: you at the server, through `finisherd <command>`. It proves with
  `<server-data>/auth/console.key`.
- **`agent`** and **`mcp`**: an agent harness or the MCP adapter, with a credential you mint
  for it (below).
- **`device`**: a Finisher app you paired (Remote access, above). It proves with its own key on
  every request.
- Nothing else. A connection that has not proved anything can only say hello.

`<server-data>/auth/` holds the keys (owner-only; `local.key` and `console.key` are replaced at
every start). The Unix socket is `<server-data>/run/fin.sock`, alone in its folder. On Windows
the server creates its named pipe itself, readable only by you, and turns away other accounts
and remote clients. The WebSocket has no password in `finisherd.json`. A client gets a one-time
ticket over a proved connection and sends it when it connects.

`fin` exits **7** when it is refused because it is the wrong kind of caller (`HUMAN_ONLY`,
`SCOPE_DENIED`, `BRANCH_NOT_ALLOWED`, `CREDENTIAL_REVOKED`, `CREDENTIAL_EXPIRED`,
`SERVER_UNTRUSTED`, …). Retrying will not help: these need you. Without a key or credential
`fin` sends nothing and says so.

**Things only you can do.** These are refused with `HUMAN_ONLY` for `fin`, agents and MCP:

- changing diagnostics or training consent (training is turned on only in the app, by hand in
  `settings.json` or with `FIN_TRAINING`, never from a command line);
- deleting recorded diagnostics, clearing a project's `never` flag, exporting recorded content
  (an export holds content only when it asks for it);
- a confirmed takeover and a relocation;
- adding libraries, installing plugins;
- minting, listing and revoking credentials, reading or purging the audit log, stopping the
  server;
- pairing, renaming and revoking devices, remote access and TLS settings, and the environment
  commands (`env adopt`, `env reset`, `tls rotate`).

You do them in the Finisher app (later) or the server's console. `finisherd` with no argument
serves. `finisherd help` lists the console commands. `finisherd status`, `audit`, `agent ls`,
`agent credentials` and `stop` run anywhere. **Every command that changes something needs an
interactive terminal**: it says what will happen and asks you to confirm. Most ask `Proceed? [y/N]`. What
cannot be undone or destroys something asks you to type the target's name back, GitHub-style:
the library's name for `library rm` (and for `library set --accept-unmounted` or
`--accept-empty-root`, which can mark files missing), the registry's id for `registry rm`, the
root's first 8 characters for `registry forget`, the plugin's id for `plugin rm --purge-data`
and for third-party code, the word `audit` for `audit purge`, the word `diagnostics` for
`diagnostics purge`, `credential.master` for `auth rotate-master`, the project id's first 8
characters for `project takeover --confirm`, the folder's name for `project open --relocate`,
the session id's first 8 characters for `agent delete`, the key id's first 8 characters for
`provider key rm` and the word `local` for `provider local rm`.
A command run from a pipe or a script is refused with `CONSOLE_NEEDS_TTY`. Once you confirm, the
console fetches a one-time code from the server and sends it with the change; you never see it.
The server still refuses a confirmed change without a fresh code from the same console session.

**What the confirmation protects against, and what it does not.** The terminal rule stops pipes
(`yes | finisherd …`), pasted scripts, cron jobs and an agent that runs `fin`. A `y` on its own
would be satisfied by `yes |`, so for y/N actions the terminal rule is what keeps that pipe out;
typing the name is the step a blind `y` cannot pass, in a pipe or in a terminal. It
does **not** stop a program running as your user that can read `<server-data>/auth/`: such a
program can prove as the console itself, or drive a pseudo-terminal (`script`, `expect`) and
answer `y` or type the name it reads off the screen. **An agent harness you start in a plain
shell runs as you**: human-only operations, scopes and branch limits bind it only if it goes
through `fin` or MCP with its own credential. They record who did what and prevent mistakes.
They are not a wall against hostile code. Finisher's OS sandbox (below) is that wall for the
processes Finisher starts itself, on each OS where it is switched on.

**The OS sandbox.** Every process Finisher starts for a CLI agent session, a media job or a
plugin goes through `fin-sandbox`, under one of four profiles (`agent`, `hook`, `worker`,
`plugin`; hooks and the Claude Code and Codex harnesses come in later versions). Where a profile
is on:

- **no profile can read the server's data**: `auth/` (`local.key`, `console.key`,
  `credential.master`), `settings.json`, `control.db`, the journal and the audit log, other
  sessions' own folders, your `~/.ssh`, `~/.gnupg`, cloud and container credentials, and your own
  `~/.claude` and `~/.codex`;
- **an agent session's processes** read only an allow-list (the toolchain, the project folder
  with the other sessions' workspaces in it, its libraries, the session's own folder) and write
  only their workspace, `workspace/shared/`, a private temp folder, their own CLI configuration
  folder and the CLI's sign-in file; never `.finisher/`, `assets/`, `finisher.project.json` or
  `finisher.rules.json`. They reach the server with the session's own credential: on Linux over
  a loopback listener that admits only agent and MCP credentials, on macOS through `fin.sock`, on
  Windows through the project's pipe;
- **media workers and plugins** read their job's inputs and write only its `out/` folder (a
  plugin also its own data folder), with no credential;
- on Linux: Landlock (kernel 6.2 or later) confines files, program starts and links, and TCP
  ports from kernel 6.7; agent processes are reaped with their session even if they leave its
  process group.
  On macOS: Seatbelt, with system services (`mach-lookup`) closed except a short list, no
  keychain, no loopback but Finisher's own. On Windows: an AppContainer per project with a
  capability per session, a kill-on-close job object and a private desktop. Media workers and
  plugins have no network on macOS and Windows, and no TCP on Linux from kernel 6.7 (see the
  Linux limits under Agents).

**It is not on by default yet.** Each profile is switched on per OS once its escape suite has
passed on that OS's validation machine; until then it runs as before, and `finisherd status`
and every agent session warn `SANDBOX_DISABLED` (`profileNotReady`; `PLUGINS_UNCONFINED` for
plugins). Once on, it cannot be turned off. On a Linux kernel with Landlock disabled (not built
in, or missing from the `lsm=` boot parameter) everything still runs, unconfined, with the
warning `SANDBOX_DISABLED` (`landlockDisabled`) naming the fix. A sandbox that is present but
fails to apply stops the start (`SANDBOX_UNAVAILABLE`).

**Agent and MCP credentials** are the recommended way to run a harness by hand. They are not
required: `fin` run by you needs none.

```bash
finisherd agent mint --label my-harness --out ~/agent.cred           # default scopes, all projects
finisherd agent mint --label reviewer --scope project.read --project <id> --out ~/r.cred
finisherd agent mint --label writer --branch own --out ~/w.cred      # may write only branches it makes
finisherd mcp setup --out ~/mcp.cred                                 # for the MCP adapter
finisherd agent credentials                                          # live credentials (--all for ended ones)
finisherd agent revoke <id>                                          # ends it now, with its connections
FIN_CREDENTIAL=~/agent.cred FIN_SERVER=<socket> FIN_PROJECT_DIR=<project> fin whoami
```

Under an agent credential `fin` reads nothing of the server's data: it needs `FIN_SERVER` (the
server's socket, as `finisherd.json` names it) and `FIN_PROJECT_DIR` (the project's folder)
beside the credential, and it never falls back to your own key.

- **Default scopes:** the server's status, read and write projects and reviews, read and submit jobs, read and scan
  libraries, read and record diagnostics. Never a human-only scope, and not `project.create`,
  `project.admin`, `fonts.fetch`, `jobs.control`, `branch.protected` or `review.decide` unless
  you ask for them. Without `--project` a credential reaches every project (the confirmation
  says so).
- **Expiry:** an agent credential ends after 24 hours unused or 7 days in all (`--idle`,
  `--absolute`, `--absolute none`). An MCP credential ends after 30 days unused and has no total
  limit. A credential that keeps being used never reaches its idle limit, so one with no total
  limit lives until you revoke it.
- **The file is a password.** It is created for you alone (0600; on Windows an owner-only ACL),
  and `fin` refuses one others can read. Without `--out` its contents are shown once.
- **Revoking** ends it at once on every connection, and its live streams. No store write it
  sent lands afterwards. Calls of other kinds that were already running may finish.
- **The limit** is 4,096 live credentials (`CREDENTIAL_LIMIT`, exit 4: revoke some, then retry).

**Backups and restores.** Keep `<server-data>/auth/` out of backups. On macOS it is excluded
from Time Machine automatically. On Windows, File History does not back up the default server
data. Elsewhere, exclude it yourself. After restoring an older `control.db`, or if a credential
file may have leaked, run `finisherd auth rotate-master`. It ends every agent and MCP credential
(an older `control.db` would otherwise bring back credentials you revoked since).

**Paired devices** are you: each one may do what you may do, including the things only you can
do, on any project. Three limits apply. A device names server folders only inside the allowed
roots: `server.projectRoots` in `settings.json` (default `<home>/Finisher`), your libraries and
the folders of projects open on the server (`PATH_NOT_ALLOWED` elsewhere). A device cannot
change those roots or point the server at certificate files; that is done from the console.
And only one call from a device needs a confirmation code, stopping the server: the app asks
you in its own dialog, then fetches the code and sends it itself. No other action from a device
takes a code; the app asks what it asks. That code protects against a stop you did not mean
and against scripts that skip the dialog. It does **not** protect against code running inside
the app: such code can fetch and answer its own code, and needs none for anything else. Treat a
compromised app as a stolen device and revoke it (`finisherd revoke <device-id>`, or from
another device). Revoking ends the device's tokens and connections at once.

**Pairing and raw-shell agents.** `fin` has no pairing command, and offers and approvals are
things only you can do, confirmed on the console. But an agent harness running in a plain shell
as you can read `<server-data>/auth/console.key` and make and approve a pairing offer through
the console, or read the key files in `<server-data>/secrets/` when they are not in the
keychain. As above, this stays true until Finisher sandboxes agents.

**Protected branches and raw-shell agents.** A protected branch (`main` by default) binds
the credentials that go through the server: an agent or MCP credential cannot write it, decide a
review or merge into it, and the server enforces that inside the store's write transaction, so a
renamed or re-resolved branch cannot slip past. Like the human-only list above, this is advisory
against an agent in a plain shell as you until Finisher sandboxes agents: such an agent can read
`<server-data>/auth/local.key`, which holds `review.decide` and `branch.protected` (so `fin`
keeps writing `main`), or `<server-data>/auth/console.key`, which can change the policy. Run
harnesses with their own credential (above), and the sandbox closes the rest.

The audit log (`finisherd audit`) records sign-in failures, refusals, credential changes and the
other security events, locally and for you only.

## Agents

Finisher runs its own agent: a loop inside the server that edits a project through `fin`, on a
model at [OpenRouter](https://openrouter.ai) (your key) or on a local OpenAI-compatible server
such as Ollama or LM Studio (no key). Each session works on **its own branch** with its own
credential, so it cannot write `main` or any branch but its own, and reads only its project.
You review its branch and merge it yourself.

```bash
finisherd provider key set openrouter [--project <dir|id>]   # paste the key (not echoed)
finisherd provider local set ollama                          # or lmstudio, or a base URL
finisherd provider local test                                # lists the server's models
finisherd agent providers                                    # what is set up (never a key)

finisherd agent start --project <dir|id> --model <id> [--provider openrouter|local] \
  [--mode plan|draft|auto] [--role editor] [--budget-usd 2] --prompt "tighten the intro"
finisherd agent watch <session>              # follow it; exits when the session ends
finisherd agent ls | show <session> [--nodes] | prompt <session> <text> | steer <session> <text>
finisherd agent interrupt | resume | end <session>
finisherd agent approve <approval> allow --add 1   # raise a budget it ran out of, then prompt again
fin merge <branch>                           # take its work into your branch
```

- **Modes:** `plan` reads and proposes (no branch); `draft` edits its branch (the default);
  `auto` is `draft` for now. A session never holds more than the person who started it.
- **Budgets** (tokens, money, wall clock, calls per run) stop a session at a safe point and ask
  you for more. A model with no known price is refused (`PRICING_UNKNOWN`); local models cost
  nothing.
- **Conversations stay on your server** in `control.db` for resume until you delete the
  session (`finisherd agent delete`) or 90 days after it ends. Deleting recorded diagnostics
  does not delete them. A session's metadata goes to the diagnostics journal while diagnostics
  are on; its content is recorded only if you turned training on, and then in a separate store.
- **Not sandboxed by default yet.** The OS sandbox (Security model) is switched on per OS once it
  passes validation there; until then an agent's tools run as your user and the server warns
  `SANDBOX_DISABLED` on every session. The built-in agent's tools are `fin` and file tools
  confined to its workspace, but run agents only on projects and prompts you trust. Agents have
  internet access (by design in this version; on Linux from kernel 6.7 ports 80 and 443 only),
  so whatever they can read, including the CLI's own sign-in, can be sent anywhere.
- **Linux sandbox limits.** On Linux the OS sandbox uses Landlock alone for now; the seccomp
  filter is an open hardening item. Landlock confines reads, writes, program starts, links and
  TCP ports. Agent and hook processes also start in a user and mount namespace where `/run`,
  `/tmp`, `/var/tmp`, `/dev/shm`, `$XDG_RUNTIME_DIR`, your home directory, the server's data
  folder and the container managers' socket folders are empty except for the paths the
  sandbox grants, so the local Unix sockets your user may open (the D-Bus
  session bus, `ssh-agent`, `gpg-agent`, `docker.sock`, `fin.sock`) are out of reach, and the
  variables that name them (`SSH_AUTH_SOCK`, `DBUS_SESSION_BUS_ADDRESS`, `DOCKER_HOST`, …) are
  removed; a hook without network gets an empty network namespace too. Ubuntu 24.04 and later
  allow that namespace only with Finisher's AppArmor profile
  (`resources/linux/apparmor/finisher-sandbox-exec`, which the installer loads); where user
  namespaces are not available the server warns `SANDBOX_SOCKETS_EXPOSED` and those sockets
  stay reachable. Still open everywhere: a socket inside a folder the sandbox grants (a
  project folder) or in a place none of those folders covers stays reachable; agents (and hooks that use `fin`) can send UDP and accept
  TCP connections on an ephemeral port; job workers and plugins get no namespace yet (they
  can reach the sockets on kernels without Landlock's socket rule, and send UDP); and every
  profile keeps the rest of the kernel's interface (process creation, `io_uring`, 32-bit
  system calls, namespaces). On kernels before 6.12 an agent's process can signal its
  launcher.
- **macOS sandbox limits.** A process an agent starts that leaves its process group (`setsid`)
  outlives the session there.
- **Windows sandbox limits.** Node tools that spawn child processes with piped stdio (npm
  scripts, Node hooks, Node MCP servers) fail fast with `WINDOWS_NODE_PIPES` inside the Windows
  sandbox, until Node ships libuv's fix. Warm media workers run cold there under the sandbox.

**Rules, approvals and subagents.** Command rules (`allow`, `ask` or `deny` on a command
pattern) apply to every agent call the server gets, however it arrives. They come from
`managed-settings.json`, your local rules, `agents.rules` in `settings.json`, the session's role
and, once you trust the project, its `finisher.rules.json`; a `deny` from any of them wins, then
an `ask`. An `ask` puts an approval card in front of you and the agent waits; `allow` lets that
one call, with exactly those arguments, through once, and `--remember` also adds a rule. A
session can start child sessions on child branches, each with a share of its budget, and merge
their work into its own branch.

```bash
finisherd agent rules ls [--project <dir|id>] [--session <id>]
finisherd agent rules add ask asset.add --project <dir|id> [--branch …] [--mode …] [--bytes-over …]
finisherd agent rules rm <rule>                  # type the rule id's first 8 characters
finisherd project trust <dir|id>                 # shows its rules and saved prompts, then trusts them
finisherd agent approve <approval> allow|deny [--remember session|project|projectLocal]
finisherd agent prompts [--project <dir|id>]     # saved prompts, such as make-shorts
finisherd agent run make-shorts --project <dir|id> --param count=3
finisherd agent rollback <session> --to-run <seq> # undo the edits of its later runs (0: every run)
```

- `--remember session` keeps the rule for that session, `projectLocal` for the project on this
  server only, and `project` writes it into the project's shared `finisher.rules.json`. When a
  project's rules or saved-prompt files change, they stop applying until you trust it again.
- Agents use `fin agent spawn|wait|ls|send|stop|rollback` for their own children and
  `fin approval wait|ls` for their own cards. A command waiting for your answer exits 8 ("wait,
  then repeat"); a denied one exits 7. Children go at most 3 deep, with 8 live per root session
  (`agents.subagents.maxDepth`, `.maxLive`); at most 4 CLI and 8 built-in sessions run at once
  (`agents.maxLiveProviders`, `agents.maxLiveBuiltin`), and the rest queue.

## Plugins

A plugin adds something to Finisher: a content pack (styles, presets, templates) or a code
plugin. Plugins come from **registries**: signed folders (local, on a NAS or a synced drive, or
a clone of a git repository) that a publisher keeps.

- **Only a person installs plugins**, through the app or the server console (`finisherd plugin
  install <id>`, which prints the plan with every permission and its risk and asks
  `Proceed? [y/N]`; third-party code asks for the plugin's id typed back). Agents and `fin` can read and suggest,
  never install, update, enable or remove.
- **Registry trust is the publisher's root hash.** `finisherd registry add <id> --dir <path>`
  shows the registry's root SHA-256 and the key ids behind it, and adds nothing until you paste
  back the hash the publisher gave you (or pass `--root <file>` / `--root-sha256 <hex>`). There
  is no trust on first use.
- **Reading** is open to `fin`:

```bash
fin plugin search [text] [--kind content|code] [--registry <id>]
fin plugin info <id>               # versions, permissions, licence, publisher
fin plugin ls                      # what is installed
fin registry ls                    # the added registries
fin registry status <id>           # freshness, trusted version, problems
fin registry refresh [<id>]        # fetch newer signed metadata (installs nothing)
```

- **Publishers** use `fin-plugin`: `fin-plugin new content|code <dir>`, `validate`, `pack <dir> -o
  <file.finpkg>`, `keygen`, and `fin-plugin registry init|delegate|publish|yank|advise|rotate-key`
  to build and sign a registry folder. `registry init` prints the root hash users pin. Private
  keys stay in a folder outside the registry. `fin-plugin registry --help` has the details.

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
purpose, with a message that names these three ways, and so is every other command line.

**See it, change it, delete it:**

```bash
fin diagnostics show                          # consent and its basis, project flags, files, sizes, counts per kind, drops
fin diagnostics export --limit 20             # the newest 20 entries, as JSONL (content removed: fin is not a person)
fin diagnostics export --kind log.commit --since 2026-11-01
fin diagnostics export --as-upload diagnostics   # exactly what a future uploader could send now
fin diagnostics project never                 # this project: record nothing, purge its entries (default: follow the settings)
```

Changing consent, deleting what was recorded, clearing a project's `never` flag and exporting
recorded content are yours alone: `fin` (and any agent that runs it) is refused with
`HUMAN_ONLY`. You make these changes in the Finisher app or the server's console
(`finisherd diagnostics consent …`, `purge`, `project default`, `export --content`), which asks
`Proceed? [y/N]` before anything changes (a purge asks you to type `diagnostics`). Training cannot be turned on from
the console either.

Withdrawing a purpose takes effect before anything else is written. It stops the upload view
from offering anything recorded under that purpose. It deletes nothing by itself: purge from
the console for that.

## License

MIT. See [LICENSE](LICENSE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
