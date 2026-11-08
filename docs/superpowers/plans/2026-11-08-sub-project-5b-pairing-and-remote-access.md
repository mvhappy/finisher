# Sub-project 5B: Pairing, devices, remote access and environments — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (the vault and the identity key, confirmations, environments and moved server data, DPoP, pairing, tokens and revocation, the remote listener, allowed roots, validation), `sonnet` for plumbing (contracts and the `devices` store, certificates and QR codes, the HTTP front, the console, e2e, docs).

**Goal:** a paired device is a principal that reaches `finisherd` from another machine, safely. After this plan:
- a device pairs by **link or QR code** with the owner's approval (name and safety code compared), keeps a P-256 key, and gets **DPoP-bound access tokens** (RFC 9449, freshness from server nonces) and then one-time **WebSocket tickets**; every device is the owner (`device` principal, all scopes, `human: true`);
- a device is a human principal whose human-only calls need **no confirmation, except `server.stop`**, which the app confirms in its dialog and sends with a `console.challenge` code of its own connection (owner decision of 2026-11-08), so 5A's last refusal of devices (`CONFIRMATION_REQUIRED` from the chain) is gone;
- the server has a P-256 **identity key** bound to its `envId`, kept in the OS keychain or a private file store; remote listening is **off by default**, and when the owner enables it a **TLS-only** listener serves either a self-signed certificate made from the identity key (SPKI-pinned by the desktop shell and native clients) or the owner's trusted certificate files (browsers); loopback plus an SSH tunnel is the other browser route (owner decision 3);
- an **environment descriptor** (`env.describe`, `GET /.well-known/finisher`) with discovered **routes**; copied or moved server data is caught (`ENV_MOVED`) until the owner adopts or resets it;
- devices name server paths only inside **allowed roots**;
- devices are listed, renamed and **revoked** at once (tokens, tickets, live connections);
- the **secret store** gets a real keychain backend, so 7A's `secrets.json` values are sealed and the file backend runs on Windows;
- console commands `pair`, `devices`, `revoke`, `remote …`, `tls …`, `env …`.

**Architecture:**
- **`@finisher/contracts`** gains `devices.ts` (devices, offers, pending requests, the environment descriptor, routes, the HTTP bodies) and `device-rpcs.ts` (the new RPCs as an array spread into `FinRpcs`), rules in `scopes.ts`, codes, audit details.
- **`finisherd`** gains `control/migrations/devices/0001.ts`, `auth/{devices.ts, pairing.ts, dpop.ts, nonces.ts, device-tokens.ts}`, `http/{front.ts, cors.ts, limits.ts}`, `net/{routes.ts, remote.ts}`, `env/{descriptor.ts, moved.ts}`, `secrets/vault-box.ts`, `console/commands/{pair.ts, devices.ts, remote.ts, tls.ts, env.ts}`; it changes `upgrade.ts` (per-listener checks and pools), `auth/revoke.ts` (devices), `auth/challenges.ts` and `chain/chain.ts` (confirmations for every human principal), `chain/resolve.ts` (device roots), `settings-write.ts` and `config.ts` (`server.*` settings), `server-admin.ts`, `connections.ts` (`wss`), `main.ts`, `secrets/store.ts`.
- **`fin-node`** gains `vault/` (keyring and file backends), `identity.rs` (the identity key), `cert.rs` (self-signed certificates, SPKI pins, files-mode checks), `qr.rs`, and `serverIdentity` (the `renewed` flag) as napi.
- **`fin-core::events`** gains the `auth.pair` kind and the `PLATFORMS` and `ROUTE_KINDS` tables.
- **`fin`** gains only exit codes (`DEVICE_REVOKED`, `ENV_MOVED`, `DEVICE_LIMIT`); it has no pairing command and never links TLS.

**Tech Stack:**
- Node 24.22 (the repository's Node): `node:https`, `node:crypto` (AES-256-GCM, SHA-256, HMAC), `node:os` `networkInterfaces`; Effect 4.0.6; `ws` 8 (already direct); **`jose` 6** (MIT, no dependencies; exact pin, the newest 6.x at least 7 days old at the start of Task 7); vitest.
- Rust 2024 in `fin-node`: **`keyring` 3** pinned exactly (`=3.x.y`, the newest 3.x at least 7 days old at the start of Task 2, recorded in `TASKS.md` and in spec §5 as built) with `default-features = false` and `apple-native`, `windows-native`, `async-secret-service`, `async-io`, `crypto-rust` (the pure-Rust `zbus` Secret Service; `zbus` pinned through the lock file and checked by `cargo deny`); **`rcgen` 0.14** with the `ring` provider (`ring` is Apache-2.0 AND ISC, allowed); **`qrcode` 0.14** with only `svg`; `ring` `aead` for the wrapped file store (already in the tree through `rcgen`).
- **No other new dependency.** No ACME client, no JWT library, no mDNS.

**Spec:** `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` revision 6: §1.2, §2, §3.2–3.5, §3.7, **§5** (all of it), §9, §10, §12 decisions 3, 4, 6, §13, §14 items 4, 5, 9, 10; and the 5A as-built constraints in §4.3 (tickets, Host and Origin, the 16 pre-upgrade slots, the per-owner caps), §4.4 (the chain), §4.7 (the console challenge is the confirmation model), §4.15–4.17 (Decisions as built, owner decisions, the residual that `server.stop` from a device must become confirmable). Parent: arch §5 (`AuthProvider`, `ConnectRelay` as extension points only), §16.

**Branch:** `sp5b-remote`, stacked on `sp4c-edit-breadth` (the integration branch, which holds 5A). 5B touches only transport, auth, settings and secret-store files, so it may run beside 5C-2, 5D, 5E and 7A-2 (spec §1.4). Files it shares with them, and how to keep merges small:
- `packages/contracts/src/rpc.ts` (540 lines): one spread line; the RPCs live in `device-rpcs.ts`.
- `packages/contracts/src/scopes.ts`: rules added at the end of `RPC_SCOPES`, in their own block.
- `apps/finisherd/src/handlers.ts` (350 lines): handlers live in `auth/device-rpc.ts`, `env/rpc.ts`; one spread line each.
- `apps/finisherd/src/chain/resolve.ts` `rootsOf`: Task 11 adds a device branch; 7A-2's `ChainHooks.extraRoots` (on `sp7a2-*`) is left alone.
- `apps/finisherd/src/secrets/*`: 7A-1 built them on this branch; 7A-2 may change their callers, not the box seam.

**Out of scope:** the desktop shell's keychain storage of its device key, SSH and WSL launch and port forwarding, the pairing UI and pages, the web client and where it is served (sub-project 9); LAN discovery; `ConnectRelay` and hosted accounts (cloud); device roles and project lists, typed pairing codes, capability URLs, signed rotation statements (cut, spec §1.2); remote `fin`.

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code. `pnpm gates` runs `cargo deny` and the JS licence gate over what each task adds; `keyring`'s `dbus` backends (libdbus, AFL-2.1 OR GPL-2.0+) are never enabled (a `cargo tree -e features` check in Task 2).
- **Primitive ops live only in `fin-core`.** This plan adds none.
- **TypeScript model types are generated, never hand-written.** The new journal kind is regenerated with `pnpm --filter @finisher/core-types generate` (drift gate `scripts/check-generated-types.mjs`). TS-owned control-plane records are Effect Schemas in `@finisher/contracts`; their TS types are `Schema.Type` of them.
- **Dependency rules:** `fin` links no TLS or HTTP stack; the server's TLS is Node's; certificates and keys are made in `fin-node`.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. Near the limit: `packages/contracts/src/rpc.ts` 540, `apps/finisherd/src/projects.ts` 596, `apps/finisherd/src/jobs/store.ts` 535, `apps/finisherd/src/library/registry.ts` 540: call-site lines only. `upgrade.ts` is 349: the HTTP front goes into `http/`, not there.
- Errors are `{code, message, details, fix?}`. New codes go into `crates/fin/src/output.rs`, `apps/finisherd/src/console/exit-codes.ts` and the contracts' code list; HTTP endpoints answer the same body with the status of Decision 33.
- **Fix texts** for human-only and device codes address the user and contain no command line (5A's `fix_texts_contain_no_command` test covers the new codes).
- **No secret anywhere it can leak.** Pairing secrets, links, offer ids with their secret, safety codes, tokens, tickets, DPoP proofs, nonces, `jti`s, private keys and certificate keys are never printed except where the owner asked for them (the console prints the link and QR code it was asked to make), never logged, journalled, audited or put in an error. The audit log holds device names and remote addresses for pairing, revocation and failed authentication only (owner decision 4).
- **The repo is public:** no host names, IPs, user names or key names in tracked files, fixtures included. Tests use `127.0.0.1` and names they make up.

**Every commit is green.** Where a task changes a server answer that a client in this repository reads (the console, the e2e harness, `fin`'s exit codes), both sides land in one commit.

**Testing and process**
- Unit tests never use the network: listeners bind `127.0.0.1`, and the remote listener's TLS path is exercised on loopback with a test option that forces it (Decision 34). Every test uses a temp server data and `FIN_SECRETS=file` unless it tests the keychain (those are `#[ignore]`d unless `FIN_TEST_KEYCHAIN=1`, and run in Task 15).
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`; tasks that touch the server's answers also run `rtk pnpm e2e` once at the end.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh`.
- **Tracker:** each task ticks its line in `TASKS.md` under "Sub-project 5B", in the same commit.
- **Batches:**
  - A: Task 1, then Tasks 2, 3 and 6 (in any order);
  - B: Tasks 4, 5, 7 (4 needs only Task 1);
  - C: Tasks 8 → 9; Tasks 10 and 11 beside them;
  - D: Tasks 12, 13;
  - E: Tasks 14, 15.

## Decisions taken here (the spec fixes the shape; these fix the details)

### Contracts, store and the device principal

1. **RPC definitions** live in `packages/contracts/src/device-rpcs.ts` as `DEVICE_RPCS` (an array of `Rpc.make`), spread into `RpcGroup.make(…)` in `rpc.ts`; their schemas are in `devices.ts`. New RPCs: `auth.pair.offer`, `auth.pair.approve`, `auth.pair.reject`, `auth.pair.pending` (stream), `auth.device.list`, `auth.device.rename`, `auth.device.revoke`, `auth.devices.watch` (stream), `env.describe`, `env.name`, `env.adopt`, `env.reset`, `env.rotateIdentity`, `settings.write`. Rules (spec §5.9): every `auth.pair.*`, `auth.device.*`, `auth.devices.watch` and `env.*` write needs `auth.admin`; `env.describe` `server.read`; `settings.write` `settings.admin`; the streams and `auth.device.list` are `unconfirmed: "readOnly"`.
2. **The `devices` chain**, migration `0001` (spec §3.1, §5.1): `devices {id, name, platform, jwk, jkt UNIQUE, createdAt, approvedBy, lastSeenAt, lastRoute, revokedAt, revokedReason}`, `pair_offers {id, secretHash, label, self, createdBy, createdAt, expiresAt, state}`, `pair_pending {id, offerId, deviceName, platform, jwk, jkt, safetyCode, redeemedAt, remote, expiresAt, state}`. `lastRoute` is a route **kind** (`loopback|lan|tailscale|public|connect|other`), never an address: addresses live only in the audit log (owner decision 4). Names are user text, 1–64 characters, NFC, no control characters; `platform` is `Name(PLATFORMS)` (`macos|windows|linux|ios|android|web|other`). Rows load at start; a device revoked more than 180 days ago is deleted then (as 5A's credentials). **At most 64 live devices** (`DEVICE_LIMIT`, exit 4, the owner revokes some); `lastSeenAt` is written at most once a minute.
3. **The device principal:** `{kind: "device", id: <deviceId>, scopes: SCOPES, projects: "*", branches: "*", human: true}`; `defaultScopes("device")` returns every scope (spec §4.5; it returns `[]` today). Source `ui` and actor `{kind: user, session: <deviceId>}` are 5A's (`diagnostics.ts` `sourceOf`/`actorFor`); `stampsAuthor` already covers `device`, so a device's new branches are authored `device:<id>`.

### The vault, the identity key and the secret box

4. **The vault** (`fin-node` `vault/`): `Vault.open(serverData, envId, {mode})` with `mode` from `FIN_SECRETS` = `auto` (default) | `keychain` | `file`. Keychain entries use service `finisher` and account `<envId>:<name>`. `auto` uses the OS keychain (macOS Keychain, Windows Credential Manager, the Secret Service on Linux) when it answers without a prompt; **when the Secret Service is absent or locked, or any backend errors, it falls back to the file store without prompting and never blocks the start** (spec Decision 22), and `server.status` carries the warning `{code: "SECRETS_FILE_STORE", reason}`. The file store is `<server-data>/secrets/<name>` (0600, the directory 0700; `makePrivateDir`/`checkPrivateDir` on Windows), written temp-then-rename with fsyncs; with `FIN_SECRETS_KEY_FILE` (a path to 32 random bytes in base64url, a Docker secret) each file is AES-256-GCM (`ring` `aead`, a random 96-bit nonce, the name and `envId` as associated data). A wrapped file that does not open is `SECRETS_KEY_INVALID` at start. Development and tests use `FIN_SECRETS=file` (macOS prompts unsigned builds on keychain access, spec §5.11); `.superpowers/env.sh` gets the line on every machine (git-ignored; the README says so).
5. **The identity key:** one P-256 key per environment (`rcgen::KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)`), PKCS#8, kept in the vault as `identity`. `control.db` `meta` records `identitySpki`, so the server can tell a key that went missing (keychain wiped, file store lost) from a first start: a missing key with devices on record is handled as a **rotation** (Decision 30: new key, every device revoked, audited `identity.rotate {reason: lost}`); a key whose SPKI differs from `identitySpki` refuses the start (`IDENTITY_MISMATCH`, start error, fix in words: the secrets of another server data are being used). The PEM crosses into JS only to build the TLS context; it is never logged.
6. **SPKI pins** are `base64url(SHA-256(SubjectPublicKeyInfo DER))` without padding (43 characters), computed in `fin-node` (`spkiPin(pem)`). The link carries it as `spki=`; `env.describe`, `/.well-known/finisher` and `server.status` show the same string; the console's `tls show` prints it in groups of four for reading aloud.
7. **7A's secret store seals for real.** `secrets/vault-box.ts` is a `SecretBox` sealing with AES-256-GCM under a data key `secrets-dek` (32 random bytes) kept in the vault: `Sealed {v: 2, data, iv}` (base64url). `v: 1` values (the identity box) still open and are resealed at the next `put`. With the file store unwrapped the data key sits beside `secrets.json`, so the protection is the private directory as before, and `server.status` says so. `FileSecretStore`'s Windows refusal (`SECRET_STORE_UNAVAILABLE`, its `TODO(5A)`) is replaced by 5A's `ensurePrivateDir`. Nothing outside `ModelGateway` reveals a value (7A's rule stands).

### Certificates and routes

8. **Self-signed certificates** (`fin-node` `cert.rs`, `rcgen` with `ring`): made from the identity key; subject `CN=finisher`; SANs: every current route's host (IP addresses and `os.hostname()`), `localhost`, `127.0.0.1`, `::1`; valid 397 days; remade **with the same key** when the SANs change or 30 days before expiry, so the pin never changes. `server.setSecureContext` swaps it on a live listener.
9. **Files mode:** `tls {mode: files, certFile, keyFile}` (absolute paths) are read at start and re-read when their mtime changes (polled every 60 s; `tailscale cert` renews every 90 days). Checked: the key matches the certificate, it is not expired, it parses (`TLS_FILES_INVALID` at start; at a reload the old context stays and `server.status` warns). A files-mode route is pinned by **WebPKI** (`pin: "webpki"`): clients verify it the normal way and check the `envId` from `/.well-known/finisher`; only self-signed routes are SPKI-pinned (`pin: "identity"`). There is no second, pinned listener for native clients. **Decision (2026-11-08, coordinator):** as written: on a `files` route native clients verify the certificate normally and check the `envId`.
10. **Routes** (`net/routes.ts`), discovered at start and every 30 s (Node has no address-change event): `loopback` (the local listener); `lan` (non-internal interfaces; IPv6 link-local excluded); `tailscale` (addresses in `100.64.0.0/10` and `fd7a:115c:a1e0::/48`); `public` (`server.remote.publicUrl`); `connect` (none in v1; the `ConnectRelay` extension point). Each route is `{kind, url, pin}`. Routes other than `loopback` exist only while the remote listener runs. A change remakes the certificate (Decision 8) and updates the descriptor; the descriptor holds no address that is not a route.

### The HTTP front and limits

11. **One request handler** (`http/front.ts`) serves both the loopback server (5A's `node:http` on `127.0.0.1`) and the remote TLS listener: `GET /.well-known/finisher`, `POST /pair/redeem`, `POST /pair/status`, `POST /auth/token`, `POST /auth/ws-ticket`, `OPTIONS` for each; everything else 404, and `/rpc` without an upgrade 426 (as today). Bodies are JSON (`Content-Type: application/json`), at most 16 KiB; answers carry `Connection: close`, `Cache-Control: no-store`, `X-Content-Type-Options: nosniff` and never set a cookie. **Host** is checked per listener before anything else: the loopback listener keeps 5A's three names with its port; the remote listener accepts its route hosts, `publicUrl`'s host and `server.remote.hosts`, each with the listener's port (or `publicUrl`'s own) → `HOST_REFUSED` (403). **Origin:** absent, a loopback origin of the loopback listener, an `https://` origin of a route, `server.origins` or `server.remote.origins` → otherwise `ORIGIN_REFUSED` (403). **CORS** is answered for allowed origins only: preflights get `Access-Control-Allow-Methods: POST`, `-Allow-Headers: authorization, dpop, content-type`, `-Expose-Headers: DPoP-Nonce`, `-Max-Age: 600`, never `-Allow-Credentials`. The WebSocket upgrade keeps 5A's order (path, version, Host, Origin, ticket) with the listener's own Host and Origin lists.
12. **Pre-upgrade pools are per listener.** Each listener has its own 16 sockets that have not upgraded (`MAX_UNAUTHENTICATED`), its own header timeout (10 s) and 5A's 1 s grace at the cap; HTTP requests count as such sockets until they end (`Connection: close` ends them). The remote listener also allows **at most 4 pre-upgrade sockets per remote address**, so one LAN host cannot hold the pool. `WebSocketSlots` stays one shared pool (64 in all, 16 only for `device` and `console`); a device's WebSockets are owned by `device:<id>` (8 per device, 5A's `MAX_WS_PER_OWNER`).
13. **Rate limits** (`http/limits.ts`, token buckets): pairing redeems 10/min per remote address and 30/min per server (spec §5.2) → `PAIRING_RATE_LIMITED` (429, `Retry-After`); `/auth/token` and `/auth/ws-ticket` 30/min per device and 120/min per server → `TOKEN_RATE_LIMITED` (429); at most 4,096 buckets (oldest dropped). Every refusal before a known device is audited `auth.fail {credentialKind: dpop|pairing|ticket, reason, remote}`, aggregated per credential kind and remote address per minute (spec §3.3).

### DPoP, nonces and tokens

14. **DPoP** (`auth/dpop.ts`, RFC 9449, `jose` 6): the `DPoP` header is one compact JWS; header `typ: dpop+jwt`, `alg: ES256`, `jwk` an EC P-256 public key with no private members; the signature verifies with that key; claims `jti` (1–64 characters), `htm` (the request method), `htu`, `iat`, `nonce`, and `ath` (`base64url(SHA-256(access token))`) wherever a token is presented. **`htu`** must have an origin among the listener's route origins, `publicUrl` and (on loopback) the loopback origins, and the request's own path; query and fragment are ignored (spec §5.3). **`iat`** is sanity-checked only (±7 days). **`nonce`** must be the current server nonce or the previous one; **`jti`** is remembered with its nonce until that nonce expires (at most 65,536 remembered: past it a new proof is refused `DPOP_INVALID {reason: replayCache}`, never an old entry evicted). The thumbprint is RFC 7638 (`jose` `calculateJwkThumbprint`).
15. **Server nonces** (`auth/nonces.ts`): 32 random bytes, base64url, rotated every 5 minutes, the previous one still valid; every HTTP answer carries the current one in `DPoP-Nonce`. A proof without a nonce or with an old one is 401 `DPOP_NONCE` with `WWW-Authenticate: DPoP error="use_dpop_nonce"`, and the client retries once with the header's nonce.
16. **Access tokens** (`auth/device-tokens.ts`): `POST /auth/token` with a proof signed by a known, live device key → `{accessToken, tokenType: "DPoP", expiresIn: 900}`; 32 random bytes, base64url; only `SHA-256(token)` is kept, with `{deviceId, jkt, expiresAt}`, in memory (a restart needs a new proof; no refresh token exists, because any proof by a known key gets a token). At most 4 live per device (a newer drops its oldest). `POST /auth/ws-ticket` with `Authorization: DPoP <token>` and a proof with `ath` whose key is the token's `jkt` → 5A's `Tickets.mint(devicePrincipal, 30)` (5A's caps and `TICKET_LIMIT` apply) → `{ticket, expiresIn: 30}`. A token for a revoked device is `DEVICE_REVOKED` (401); an expired one `TOKEN_EXPIRED` (401).

### Pairing

17. **Offers:** `auth.pair.offer {label?, ttlS ≤ 600 (default 300), self?}` → `{offerId, link, qr, expiresAt}`. The secret is 32 random bytes; only `SHA-256(secret)` is stored. The link is spec §5.2's `finisher://pair?v=1&env=<envId>&spki=<pin>&r=<base64url(JSON routes)>#<offerId>.<secret>`; when a `webpki` route exists the result also carries `browserLink` = `https://<that route>/pair#<offerId>.<secret>` (the page itself is sub-project 9's). `qr` is an SVG of the link from `fin-node` `qrSvg`; the console prints `qrText` (Unicode half blocks). At most 8 open offers (the oldest is expired first); audited `pair.offer {label}`.
18. **Redeem:** `POST /pair/redeem {offerId, secret, device: {name, platform, jwk}}` over any listener; the secret is compared in constant time; the offer is consumed at the **first valid** redeem (`PAIRING_USED` 409 afterwards, visible to the legitimate device, spec §5.10); expired `PAIRING_EXPIRED` (410). Answer `{pendingId, safetyCode}`; `safetyCode` = the first 4 bytes of `HMAC-SHA-256(secret, jkt)` as a big-endian `u32` mod 10⁶, six digits with leading zeros. At most 16 pending requests; each expires 10 minutes after its redeem. Audited `pair.redeem {deviceName, platform, remote}`.
19. **Approve and reject:** `auth.pair.approve|reject {pendingId}` (human-only; confirmed on the console, not from a device, Decision 22) on the console or a device; approval writes the `devices` row in one transaction with the pending row's state; `pair.approve {deviceName, remote}` / `pair.reject`. `auth.pair.pending` streams the pending requests (snapshot first; `{pendingId, deviceName, platform, safetyCode, redeemedAt, expiresAt}`, never the remote address, which only the audit holds).
20. **Status:** `POST /pair/status {pendingId}` with a DPoP proof by the pending request's key → `{state: pending|approved|rejected|expired, deviceId?, environment?}` (the descriptor once approved). `PAIRING_REJECTED` (403) once rejected.
21. **Self offers** (the desktop shell pairing its own renderer, spec §5.2): `auth.pair.offer {self: true}` only from a `console` principal (`INVALID_REQUEST` otherwise); redeemable only over the **loopback** listener (a remote redeem of it is `PAIRING_EXPIRED`, so it cannot be told apart); approved at redeem; and it **needs no challenge code**, through the rule's payload-dependent opt-out `unconfirmedIf: (p) => p.self === true ? "service" : undefined` (a new `RuleOf` field), because the desktop shell's main process has no terminal. **Decision (2026-11-08, coordinator):** as written: loopback only, a `self`-flagged offer, approved at redeem.

### Confirmations for every human principal

22. **Devices confirm only `server.stop`.** **Decision (2026-11-08, owner):** from a device, only `server.stop` needs a confirmation, and the app asks for it in its own dialog; no other human-only action from a device needs one. This follows the console model of the same day (5A §4.7: the person confirms, and the client fetches and sends the challenge code itself; the code is never shown or typed). A device is a human principal. The chain's confirmation stage (`chain.ts`) asks `needsConfirmation(tag, payload, facts, principal)`: for a `console` it is 5A's rule (a human-only scope, no `unconfirmed` opt-out); for a `device` it is true **only for `server.stop`**. `console.challenge` keeps its tag (clients are version-locked; no rename churn) and is served to `console` and `device` principals (`INVALID_REQUEST` otherwise); after the person confirms the stop in the dialog, the app fetches a code on its own connection and sends it with `server.stop`, retrying once on `wrong` or `expired`. `unconfirmed: "service"` stays console-only, which is what leaves a device's `server.stop` confirmed, and the chain's blanket refusal of non-console humans for `service` rules goes. `undoesOwnMint` stays console-only. Pairing approvals, device revocation, `settings.write` and onboarding's training-on answer from a device are not confirmed by the chain; the app's own UI asks what it asks.
23. **What a device's `server.stop` confirmation protects against, stated:** a stop the person did not mean, and scripted calls that skip the dialog. **Not** code running in the renderer: it can fetch a challenge and answer it on the same connection, and it can make every other human-only call unconfirmed. The README and spec §5.10 say so; a compromised renderer is a stolen device (revoke it).

### The remote listener and settings

26. **`server.remote`** (`settings.json`): `{enabled, listen, tls: {mode: selfSigned | files, certFile?, keyFile?}, publicUrl?, origins?, hosts?}`; **`server.projectRoots`** (absolute paths; default `[<home>/Finisher]`); **`server.loopback.port`** (default absent = a random port, as today). Read with the schema in `config.ts` (an invalid section is ignored with a `server.status` warning, never a crash).
27. **`settings.write {path, value}`** (`settings.admin`, human-only; confirmed on the console, Decision 22): `path` is one of `server.remote`, `server.projectRoots`, `server.origins`, `server.loopback`; the value is checked against its schema; `settings-write.ts` generalises `mergeSettings` from `privacy` to these keys (every other key kept; a file that does not parse is never overwritten). The server applies it live (starts, restarts or stops the remote listener) and audits `settings.write {path}` (never the value). A remote config whose `tls` is missing or whose files are invalid is refused before it is written (`LISTEN_NEEDS_TLS`, `TLS_FILES_INVALID`, exit 3).
28. **`FIN_LISTEN=<host:port>`** (Docker) runs the remote listener whatever `settings.json` says, self-signed unless `FIN_TLS_CERT` and `FIN_TLS_KEY` name files; `server.status` reports the source (`env` or `settings`).
29. **TLS only off loopback:** a non-loopback `listen` is always TLS (`node:https` with the certificate of Decision 8 or 9; TLS 1.2+, the default ciphers); `LISTEN_NEEDS_TLS` at start is reachable only through a hand-edited file. The remote listener is not started while the environment is `moved` (Decision 31). The default `listen` is `0.0.0.0:7311` (and `[::]:7311` where the OS gives a dual-stack socket); an SSH tunnel forwards the loopback listener and **uses the same port number on both ends**, because the Host allow-list names the port (5A's check is kept); `server.loopback.port` makes that port stable. **Decision (2026-11-08, coordinator):** as written: remote default `0.0.0.0:7311`, loopback on a random port unless `server.loopback.port`, SSH tunnels on the same port number at both ends. Starting and stopping are audited `listener.start {spki, mode}` / `listener.stop`.

### Environments

30. **Descriptor and identity:** `env.describe` → `{envId, name, spki, serverIds, protocol: {min, max}, routes, capabilities, state: ok | moved}`; `env.name {name}` (1–64 characters); `GET /.well-known/finisher` → `{envId, protocol, spki}` only (anonymous, spec §5.5). **`env.rotateIdentity`** (`finisherd tls rotate`): a new identity key, the same `envId`, **every device revoked** (they pair again, spec §5.5), audited `identity.rotate {reason: owner|lost, count}` (a new audit kind).
31. **Moved or copied server data** (spec §5.8): at start, `fin-node` `serverIdentity()` reports `renewed`; when the id was renewed and `server_ids` already maps other ids to this `envId`, `meta.envState` becomes `moved` (audited `env.moved {reason}`). While moved: the remote listener stays off, and `/auth/token`, `/pair/*`, device tickets and device WebSockets answer **`ENV_MOVED`** (403, exit 7); `local` and `console` work. `env.adopt` (it was a move) clears the state (`env.adopted`). `env.reset` (it is a copy) mints a new `envId` and identity key, deletes devices, offers and pending requests, rewrites `finisherd.json`, and revokes every agent and MCP credential through the one path (their keys bind the old `envId` and would stop proving anyway; revoking says so), audited `env.reset {devices, credentials}`.

### Reach

32. **Allowed roots for devices** (spec §5.7): `rootsOf` gives a `device` principal roots although its `projects` is `"*"`: every `server.projectRoots` entry, the library roots (5D's registry `roots()` when it is on the branch; until then none), and the folders of projects open here, all canonical. Every path resolver (`createDir`, `openDir`, `probePath`, `importDir`, `outDir`) is checked against them → `PATH_NOT_ALLOWED` (exit 7); `library.add` joins when its RPC lands (5D). `local` and `console` keep full reach. A root that does not exist is skipped, never created.

### Journal, codes and tests

33. **Journal:** `auth.pair` v1 (`source: finisherd`, meta only): `outcome: offered|redeemed|approved|rejected|expired|revoked`, `platform: Name(PLATFORMS)`, `route: Name(ROUTE_KINDS)`, `micros?` (redeem → approve). New closed tables in `fin_core::events::names`; `RPC_METHODS` gains every new RPC. **Codes:** `PAIRING_EXPIRED` 410, `PAIRING_USED` 409, `PAIRING_REJECTED` 403, `PAIRING_RATE_LIMITED` 429, `TOKEN_RATE_LIMITED` 429, `DPOP_INVALID` 401, `DPOP_NONCE` 401, `TOKEN_EXPIRED` 401, `TICKET_INVALID` 401, `ORIGIN_REFUSED` 403, `HOST_REFUSED` 403; exit 7: `DEVICE_REVOKED` (401), `ENV_MOVED` (403), `PATH_NOT_ALLOWED`; exit 4: `DEVICE_LIMIT` (409); start: `LISTEN_NEEDS_TLS`, `TLS_FILES_INVALID`, `IDENTITY_MISMATCH`, `SECRETS_KEY_INVALID`; client side (sub-project 9, contracts only): `TLS_PIN_MISMATCH`.
34. **Tests without a network:** `serve` takes `remote: {forceTls: true, listen: "127.0.0.1:0"}` (a test option, not a setting), so the TLS listener, its Host list and its pools run on loopback; the per-address pool is tested with injected remote addresses. The e2e `remote` scenario's cross-host part runs in Task 15 with addresses from the run's environment.

## Owner decisions (2026-11-07) this plan implements

- **3:** browsers reach a remote server only over a route with a certificate they trust (files mode: `tailscale cert`, a reverse proxy) or over loopback through an SSH tunnel; pinning is for the desktop shell and native clients; no ACME, no click-through (Decisions 8, 9, 29).
- **4:** the audit log keeps device names and remote addresses for pairing, revocation and failed authentication only; nothing else stores an address (`lastRoute` is a kind) (Decisions 2, 13, 18, 19, 25).
- **5:** `local` keeps write access to `main` (`branch.protected`, `review.decide`); 5B changes nothing for `local`, and a device is the owner.
- **2026-11-08 (owner):** from devices only `server.stop` needs a confirmation, through the app's dialog (Decision 22); the console asks y/N or the target's name and sends the challenge itself (5A §4.7). **(Coordinator):** Decisions 9, 21 and 29 as written.
- **6:** human-only is advisory against raw-shell agents until 7C: an unsandboxed agent can read `console.key` and mint and approve pairing offers through the console, or read the file store; the README says so beside 5A's statement. `fin` has no pairing command, and offers and approvals are human-only and confirmed.

## Review Focus

1. **Nothing trusts a route:** loopback grants nothing; every device request carries a DPoP proof by a known key with a fresh server nonce; tokens and tickets are bound and short. → Task 7 `rfc9449_vectors`, `replayed_jti_refused`, `old_nonce_is_use_dpop_nonce`, `htu_behind_proxy`; Task 9 `token_needs_matching_jkt`, `ws_ticket_needs_ath`.
2. **Pairing cannot be hijacked quietly:** one-time offers, first valid redeem consumes, the legitimate redeem then fails visibly, owner approval with name and safety code, rate limits. → Task 8.
3. **Confirmations are what the owner decided:** a device's `server.stop` needs its own connection's code; no other device call does; the console's rule is unchanged; the self offer is the console's only new opt-out. → Task 4 `device_server_stop_needs_code`, `device_code_is_per_connection`, `device_human_only_mutations_need_no_code`, `console_rule_is_unchanged`.
4. **TLS only off loopback; the pin never changes** except by rotation. → Tasks 3, 10.
5. **Limits from 5A survive a LAN:** per-listener pre-upgrade pools, per-address cap, shared slots with the device and console reserve. → Task 6 `remote_pool_does_not_starve_loopback`, `per_address_cap`.
6. **Revocation is immediate everywhere** (tokens, tickets, every transport, streams within 100 ms). → Task 9.
7. **Moved or copied server data fails closed** until the owner decides. → Task 5.
8. **Allowed roots** for every path a device names. → Task 11.
9. **No secret leaks** (pairing secrets, links, tokens, proofs, nonces, private keys). → Task 13 leak grep.
10. **Licences:** no `dbus`/libdbus; `cargo deny` and the JS gate pass. → Task 2.

---

### Task 1: Contracts, the `devices` chain and the device principal — tier: sonnet

**Files:**
- Create: `packages/contracts/src/{devices.ts, device-rpcs.ts}`, `apps/finisherd/src/control/migrations/devices/0001.ts`, `apps/finisherd/src/auth/devices.ts`.
- Modify: `packages/contracts/src/{rpc.ts (spread `DEVICE_RPCS`), scopes.ts (the rules, `unconfirmedIf` on `RuleOf`/`RpcRule`), auth.ts (`defaultScopes("device")`), audit.ts (details for `pair.*`, `device.*`, `listener.*`, `env.*`, `settings.write`, new `identity.rotate`), index.ts}`, `crates/fin/src/output.rs` and `apps/finisherd/src/console/exit-codes.ts` (the codes of Decision 33), `crates/fin-core/src/events/names.rs` (`RPC_METHODS`), `apps/finisherd/src/handlers.ts` (stub handlers answering `INTERNAL {reason: notYet}` until their task, so the mapped types hold), `TASKS.md`.
- Test: `packages/contracts/src/scopes.test.ts`, `packages/contracts/src/audit.test.ts`, `apps/finisherd/test/devices-store.test.ts`.

**Interfaces:**
```ts
// devices.ts
export const PLATFORMS = ["macos", "windows", "linux", "ios", "android", "web", "other"] as const;
export const ROUTE_KINDS = ["loopback", "lan", "tailscale", "public", "connect", "other"] as const;
export const RouteSchema = Schema.Struct({ kind: Schema.Literals(ROUTE_KINDS), url: Schema.String, pin: Schema.Literals(["identity", "webpki"]) });
export const EnvironmentSchema = Schema.Struct({ envId, name, spki, serverIds, protocol: {min, max}, routes, capabilities, state: Schema.Literals(["ok", "moved"]) });
export const DeviceSchema = Schema.Struct({ id, name, platform, jkt, createdAt, approvedBy, lastSeenAt, lastRoute, revokedAt: optional });
// …PairOffer*, PairPending*, DeviceList/Rename/Revoke*, DevicesWatchItem, SettingsWrite*, the HTTP bodies

// auth/devices.ts
export class DeviceStore {
  constructor(store: ControlStore, audit: Audit, clock?: () => number);
  list(all?: boolean): ReadonlyArray<Device>;
  byJkt(jkt: string): Device | undefined;          // live only
  add(d: NewDevice): Device;                         // DEVICE_LIMIT
  rename(id: string, name: string): Device;
  markEnded(id: string, reason: "owner" | "self" | "reset" | "rotation"): boolean;
  seen(id: string, route: RouteKind): void;          // at most once a minute
  persist(): void;
  principalOf(d: Device): Principal;
}
```
Decisions 1–3, 33.

- [ ] **Step 1: Write failing tests:** `device_rpcs_have_rules` (the mapped type and `rpc_scopes_cover_every_rpc` cover the new tags), `device_default_scopes_are_all`, `device_rules_human_only` (every new write needs `auth.admin`/`settings.admin`), `audit_details_closed_for_new_kinds` (names and `remote` only on `pair.*`, `device.revoke`, `auth.fail`), `devices_chain_migrates_once`, `device_limit_is_exit_4`, `last_seen_written_once_a_minute`, `ended_rows_deleted_after_180_days`, `name_rules` (NFC, length, control characters), `codes_have_exit_classes` (Rust and TS tables agree).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(contracts): devices, pairing and environment schemas; the devices store`.

### Task 2: The vault, the identity key and the secret box — tier: opus — after Task 1

**Files:**
- Create: `crates/fin-node/src/vault/{mod.rs, keychain.rs, file.rs}`, `crates/fin-node/src/identity.rs`, `apps/finisherd/src/secrets/{vault.ts, vault-box.ts}`.
- Modify: `crates/fin-node/Cargo.toml` (`keyring` exact pin and features), `crates/fin-node/src/lib.rs`, `packages/fin-node` (typings), `apps/finisherd/src/secrets/store.ts` (Windows through `ensurePrivateDir`; the box from the vault), `apps/finisherd/src/main.ts` (open the vault after the control store; the identity key; `server.status` warnings), `apps/finisherd/src/server-admin.ts`, `THIRD_PARTY_NOTICES.md`, `deny.toml` if a licence needs naming, `TASKS.md` (the pinned versions).
- Test: `crates/fin-node/tests/vault.rs`, `apps/finisherd/test/vault.test.ts`, `apps/finisherd/test/secrets.test.ts` (extended).

**Interfaces:**
```rust
#[napi] pub struct Vault { /* backend */ }
#[napi] impl Vault {
  #[napi(factory)] pub fn open(server_data: String, env_id: String, mode: String, key_file: Option<String>) -> napi::Result<Vault>;
  #[napi] pub fn backend(&self) -> VaultBackend;           // {kind: keychain|file|fileWrapped, reason?}
  #[napi] pub fn get(&self, name: String) -> napi::Result<Option<Buffer>>;
  #[napi] pub fn set(&self, name: String, value: Buffer) -> napi::Result<()>;
  #[napi] pub fn delete(&self, name: String) -> napi::Result<bool>;
}
#[napi] pub fn identity_key(vault: &Vault, expected_spki: Option<String>) -> napi::Result<IdentityKey>; // {pem, spki, created}
#[napi] pub fn new_identity_key(vault: &Vault) -> napi::Result<IdentityKey>;
#[napi] pub fn spki_pin(pem: String) -> napi::Result<String>;
```
```ts
export function vaultBox(vault: Vault): SecretBox;   // Sealed v2; opens v1
```
Decisions 4–7.

- [ ] **Step 1: Write failing tests:** `file_store_is_private` (0600/0700; Windows DACL through `checkPrivateDir`), `wrapped_file_store_round_trips`, `wrapped_file_with_wrong_key_is_secrets_key_invalid`, `associated_data_binds_name_and_env` (a file renamed or moved to another env does not open), `locked_secret_service_falls_back_without_prompt` (a fake backend that answers `Locked`), `keychain_error_falls_back`, `identity_key_persists`, `identity_spki_mismatch_refuses_start`, `lost_identity_with_devices_is_rotation` (devices revoked, audited), `vault_box_seals_and_opens`, `v1_values_open_and_reseal`, `secret_store_runs_on_windows` (no `SECRET_STORE_UNAVAILABLE`), `no_dbus_feature` (`cargo tree -e features -p fin-node` has no `dbus`); keychain round trips behind `FIN_TEST_KEYCHAIN=1`.
- [ ] **Step 2: Implement.** Pin `keyring` exactly; record the version, its `zbus` version and the date in `TASKS.md`.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(fin-node,finisherd): a vault with keychain and file stores, the identity key, sealed secrets`.

### Task 3: Certificates, SPKI pins and QR codes — tier: sonnet — after Task 2

**Files:**
- Create: `crates/fin-node/src/{cert.rs, qr.rs}`.
- Modify: `crates/fin-node/Cargo.toml` (`rcgen` 0.14 with `ring`; `qrcode` 0.14 with `svg` only), `crates/fin-node/src/lib.rs`, `packages/fin-node` (typings), `THIRD_PARTY_NOTICES.md`.
- Test: `crates/fin-node/tests/cert.rs`, `apps/finisherd/test/cert.test.ts`.

**Interfaces:**
```rust
#[napi] pub fn self_signed_cert(identity_pem: String, sans: Vec<String>, not_after_days: u32) -> napi::Result<String>; // PEM
#[napi] pub fn check_tls_files(cert_pem: String, key_pem: String, now_ms: f64) -> TlsFilesReport; // {ok, reason?, spki, notAfter}
#[napi] pub fn qr_svg(text: String) -> napi::Result<String>;
#[napi] pub fn qr_text(text: String) -> napi::Result<String>;
```
Decisions 6, 8, 9.

- [ ] **Step 1: Write failing tests:** `cert_spki_is_identity_spki`, `remade_cert_keeps_pin` (other SANs, same pin), `sans_cover_ips_and_names`, `node_tls_accepts_the_pair` (a `tls.createSecureContext` round trip and a loopback handshake whose peer SPKI matches the pin), `files_key_mismatch_is_invalid`, `files_expired_is_invalid`, `qr_decodes_to_link` (a decoder in the test only, dev-dependency checked by the gates, or the module matrix compared with a golden).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(fin-node): self-signed certificates from the identity key, SPKI pins, QR codes`.

### Task 4: Confirmations for every human principal — tier: opus — after Task 1

**Files:**
- Modify: `apps/finisherd/src/chain/chain.ts` (the confirmation stage asks `needsConfirmation` with the principal: devices only for `server.stop`; the `service` opt-out console-only; `unconfirmedIf`), `apps/finisherd/src/auth/challenges.ts` (`console.challenge` for `console` and `device`), `apps/finisherd/src/handlers.ts` (the `console.challenge` handler's principal check), `packages/contracts/src/scopes.ts` (`needsConfirmation(tag, payload, facts, principal)`), `TASKS.md` (ticks the 5B line "make `server.stop` … confirmable by a device").
- Test: `apps/finisherd/test/chain.test.ts` (the 5A test "a device with auth.admin must confirm it" becomes the positive case), `apps/finisherd/test/challenges-device.test.ts`.

**Interfaces:**
```ts
export function needsConfirmation(tag: FinRpcTag, payload: unknown, facts: RuleFacts, principal: Pick<Principal, "kind" | "human">): boolean;
// console: 5A's rule; device: tag === "server.stop"; every other principal: false.
// RuleOf<T>: readonly unconfirmedIf?: (payload: PayloadOf<T>) => "readOnly" | "service" | undefined;
```
Decisions 21–23.

- [ ] **Step 1: Write failing tests:** `device_server_stop_needs_code` (refused `CONFIRMATION_REQUIRED {reason: missing}`, accepted with its connection's code), `device_code_is_per_connection` (another device connection's code is `wrong`), `device_wrong_code_voids_codes`, `device_human_only_mutations_need_no_code` (the matrix over `RPC_SCOPES`: every human-only call but `server.stop` goes ahead from a device without `challenge`; pairing approval, revocation, `settings.write`, training on included), `console_rule_is_unchanged` (the same matrix from the console still needs a code), `console_stop_stays_service`, `self_offer_needs_no_code_only_from_console`, `non_human_gets_no_challenge` (`local`, `agent`: `INVALID_REQUEST`), `console_refused_audited_for_devices` (a device's `server.stop` without a code).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(auth): server.stop confirmable from a device; devices otherwise unconfirmed`.

### Task 5: Routes, the environment descriptor and moved server data — tier: opus — after Tasks 2 and 3

**Files:**
- Create: `apps/finisherd/src/net/routes.ts`, `apps/finisherd/src/env/{descriptor.ts, moved.ts, rpc.ts}`.
- Modify: `crates/fin-node/src/lib.rs` (`serverIdentity` napi with `renewed`), `apps/finisherd/src/main.ts` (route discovery; the moved check after the control store opens; `finisherd.json` rewrite on reset), `apps/finisherd/src/control/db.ts` (`meta.envState`, `identitySpki`; minting a new `envId` for `env.reset`), `apps/finisherd/src/auth/credentials.ts` (revoke every credential on reset through the one path), `apps/finisherd/src/server-admin.ts` (`server.status` gains `env.state`, `routes`, `spki`), `apps/finisherd/src/handlers.ts` (spread `envHandlers`), `TASKS.md`.
- Test: `apps/finisherd/test/routes.test.ts`, `apps/finisherd/test/env.test.ts`.

**Interfaces:**
```ts
export interface RouteSource { interfaces(): NodeJS.Dict<os.NetworkInterfaceInfo[]>; hostname(): string }
export class Routes { constructor(src: RouteSource, opts: { remote?: RemoteInfo; publicUrl?: string }); current(): ReadonlyArray<Route>; onChange(f: (r: ReadonlyArray<Route>) => void): () => void; poll(): void }
export class Environment {
  describe(): Environment;                       // Decision 30
  name(n: string): void;
  state(): "ok" | "moved";
  adopt(): void;
  reset(): { envId: string; devices: number; credentials: number };
  rotateIdentity(reason: "owner" | "lost"): { revoked: number };
}
```
Decisions 10, 30, 31.

- [ ] **Step 1: Write failing tests:** `routes_classify_interfaces` (fake interfaces: internal skipped, IPv6 link-local skipped, Tailscale ranges, LAN), `public_route_from_public_url`, `route_change_fires_once`, `describe_matches_well_known_subset`, `renewed_server_id_marks_moved` (a copied server data: `env.moved` audited, `state: moved`), `moved_refuses_devices_and_keeps_local`, `adopt_keeps_env_and_devices`, `reset_mints_env_drops_devices_and_credentials` (old credential files no longer prove; `finisherd.json` has the new `envId`), `rotate_identity_revokes_every_device_and_changes_pin`, `env_writes_are_human_only_and_confirmed`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): routes, the environment descriptor, moved server data and identity rotation`.

### Task 6: The HTTP front and per-listener limits — tier: sonnet — after Task 1

**Files:**
- Create: `apps/finisherd/src/http/{front.ts, cors.ts, limits.ts}`.
- Modify: `apps/finisherd/src/upgrade.ts` (`makeTicketedWebSocketServer` takes a `listener` description: its Host list, Origin list, pool size, per-address cap, and the HTTP handler; `loopbackHosts` stays), `apps/finisherd/src/config.ts` (`server.*` settings read; `cachedOrigins` covers `server.remote.origins`), `TASKS.md`.
- Test: `apps/finisherd/test/http-front.test.ts`, `apps/finisherd/test/upgrade-limits.test.ts` (extended).

**Interfaces:**
```ts
export interface ListenerPolicy {
  readonly name: "loopback" | "remote";
  readonly hosts: () => ReadonlyArray<string>;          // host:port, lower case
  readonly origins: () => ReadonlyArray<string>;
  readonly maxUnauthenticated: number;                  // 16
  readonly perAddress?: number;                         // remote: 4
}
export type Route = (req: IncomingMessage, body: unknown, ctx: HttpCtx) => Promise<HttpAnswer>;
export function makeFront(routes: Record<string, Route>, policy: ListenerPolicy, nonces: Nonces): (req: IncomingMessage, res: ServerResponse) => void;
export class Buckets { constructor(rate: { perMinute: number; burst: number }, max?: number); take(key: string): boolean }
```
Decisions 11–13.

- [ ] **Step 1: Write failing tests:** `host_checked_first`, `origin_matrix_per_listener`, `cors_only_for_allowed_origins` (no `Allow-Credentials` ever), `body_cap_and_content_type`, `answers_close_and_no_store`, `unknown_path_404_rpc_426`, `remote_pool_does_not_starve_loopback`, `per_address_cap` (a fifth socket from one address is closed; another address gets in), `ws_slots_shared_with_device_reserve`, `rate_buckets_bounded`, `refusals_audited_with_remote`.
- [ ] **Step 2: Implement.** The loopback listener behaves exactly as in 5A for every existing test.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): an HTTP front with per-listener Host, Origin and pre-upgrade limits`.

### Task 7: DPoP and server nonces — tier: opus — after Task 6

**Files:**
- Create: `apps/finisherd/src/auth/{dpop.ts, nonces.ts}`, `tests/fixtures/auth/dpop.json` (RFC 9449's examples and generated cases with fixed fake keys).
- Modify: `apps/finisherd/package.json` (`jose` exact pin), `pnpm-lock.yaml`, `THIRD_PARTY_NOTICES.md`, `TASKS.md`.
- Test: `apps/finisherd/test/dpop.test.ts`.

**Interfaces:**
```ts
export class Nonces { constructor(clock?: () => number); current(): string; accepts(n: string): boolean; }   // 5 min, previous kept
export interface DpopCheck { readonly method: string; readonly url: URL; readonly allowedOrigins: ReadonlyArray<string>; readonly accessToken?: string }
export type DpopResult = { ok: true; jkt: string; jwk: JWK } | { ok: false; code: "DPOP_INVALID" | "DPOP_NONCE"; reason: string };
export class DpopVerifier { constructor(nonces: Nonces, clock?: () => number); verify(header: string | undefined, check: DpopCheck): Promise<DpopResult> }
```
Decisions 14, 15.

- [ ] **Step 1: Write failing tests:** `rfc9449_vectors`, `typ_alg_and_jwk_checked` (`HS256`, `none`, an RSA key, a JWK with `d`), `htm_must_match`, `htu_behind_proxy` (origin from `publicUrl`, path kept, query ignored), `htu_other_origin_refused`, `iat_sanity_window`, `old_nonce_is_use_dpop_nonce` (status, `WWW-Authenticate`, header), `previous_nonce_still_valid`, `replayed_jti_refused`, `replay_cache_full_refuses`, `ath_required_with_token`, `thumbprint_rfc7638`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(auth): DPoP proofs with server nonces`.

### Task 8: Pairing — tier: opus — after Tasks 4, 5 and 7

**Files:**
- Create: `apps/finisherd/src/auth/{pairing.ts, device-rpc.ts}`, `crates/fin-core/src/events/payloads/auth.rs`.
- Modify: `crates/fin-core/src/events/{kinds.rs (`auth.pair` v1, `Finisherd`), names.rs (`PLATFORMS`, `ROUTE_KINDS`), payloads/mod.rs}`, the regenerated schema and `generated.ts`, `apps/finisherd/src/http/front.ts` (the `/pair/*` routes), `apps/finisherd/src/handlers.ts` (spread `deviceHandlers`), `apps/finisherd/src/main.ts`, `TASKS.md`.
- Test: `apps/finisherd/test/pairing.test.ts`, `crates/fin-core/tests/events.rs` (golden).

**Interfaces:**
```ts
export class Pairing {
  constructor(deps: { store: ControlStore; devices: DeviceStore; audit: Audit; journal: Journal; env: Environment; clock?: () => number });
  offer(by: Principal, req: { label?: string; ttlS?: number; self?: boolean }): { offerId: string; link: string; browserLink?: string; qr: string; expiresAt: number };
  redeem(body: RedeemBody, ctx: { remote: string; listener: "loopback" | "remote"; route: RouteKind }): { pendingId: string; safetyCode: string };
  approve(by: Principal, pendingId: string): Device;
  reject(by: Principal, pendingId: string): void;
  status(pendingId: string, jkt: string): PairStatus;
  pending(): Stream.Stream<PendingView>;
  sweep(): void;                                   // expiry, every 10 s
}
export function safetyCode(secret: Buffer, jkt: string): string;
```
Decisions 17–21, 33.

- [ ] **Step 1: Write failing tests:** the state machine (`offer → redeem → approve`, `→ reject`, `→ expire` at 10 minutes, offer expiry), `first_valid_redeem_consumes` (the second is `PAIRING_USED`), `bad_secret_does_not_consume`, `safety_code_vector`, `redeem_rate_limits` (per address and per server), `status_needs_pending_key`, `approval_is_human_only_and_confirmed`, `self_offer_loopback_only_and_approved_at_redeem`, `self_offer_from_device_refused`, `open_offer_and_pending_caps`, `pending_stream_snapshot_first_without_address`, `audit_rows_hold_name_and_remote_only_where_allowed`, `auth_pair_journal_meta_only`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(auth): pairing by link or QR code with owner approval and safety codes`.

### Task 9: Tokens, device tickets and revocation — tier: opus — after Task 8

**Files:**
- Create: `apps/finisherd/src/auth/device-tokens.ts`.
- Modify: `apps/finisherd/src/http/front.ts` (`/auth/token`, `/auth/ws-ticket`), `apps/finisherd/src/auth/{revoke.ts (devices), tickets.ts (device owner keys)}`, `apps/finisherd/src/auth/device-rpc.ts` (`auth.device.*`, `auth.devices.watch`, `auth.revokeSelf` for devices), `apps/finisherd/src/connections.ts` (`DEVICE_REVOKED` for devices; `seen`), `TASKS.md`.
- Test: `apps/finisherd/test/device-tokens.test.ts`, `apps/finisherd/test/device-revoke.test.ts`.

**Interfaces:**
```ts
export class DeviceTokens {
  constructor(clock?: () => number);
  issue(device: Device): { accessToken: string; expiresIn: 900 };
  check(token: string, jkt: string): { deviceId: string } | { refused: "TOKEN_EXPIRED" | "DEVICE_REVOKED" | "DPOP_INVALID" };
  dropDevice(id: string): number;
}
// RevocableRef: { kind: "agent" | "mcp" | "device"; id: string }
```
Decisions 16, 24, 25.

- [ ] **Step 1: Write failing tests:** `token_needs_known_live_key`, `token_needs_matching_jkt`, `token_expires_at_900s`, `four_tokens_per_device`, `ws_ticket_needs_ath`, `device_ws_is_device_principal` (`auth.whoami` over the ticketed WebSocket; source `ui`), `revoke_closes_live_subscription_within_100ms`, `revoke_drops_tokens_and_tickets`, `revoke_self_from_device`, `revoked_device_token_is_device_revoked`, `devices_watch_snapshot_and_changes`, `rename_audited_with_name_only`, `moved_env_refuses_tokens`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(auth): DPoP-bound device tokens, device tickets and immediate revocation`.

### Task 10: The remote listener, `settings.write` and `FIN_LISTEN` — tier: opus — after Tasks 3, 5 and 6

**Files:**
- Create: `apps/finisherd/src/net/remote.ts`.
- Modify: `apps/finisherd/src/settings-write.ts` (generalised patches), `apps/finisherd/src/config.ts` (`server.remote`, `server.projectRoots`, `server.loopback` schemas), `apps/finisherd/src/main.ts` (the remote listener in the server's scope; `FIN_LISTEN`; `server.loopback.port`; `test` options of Decision 34), `apps/finisherd/src/connections.ts` (`Transport` gains `wss`), `apps/finisherd/src/server-admin.ts` (listeners with `spki` and source), `apps/finisherd/src/handlers.ts` (`settings.write`), `apps/finisherd/src/env.ts` (`FIN_LISTEN`, `FIN_TLS_CERT`, `FIN_TLS_KEY`, `FIN_SECRETS`, `FIN_SECRETS_KEY_FILE` documented and kept out of `childEnv`), `packages/contracts/src/auth.ts` (`ServerStatusSuccess` listeners), `TASKS.md`.
- Test: `apps/finisherd/test/remote-listener.test.ts`, `apps/finisherd/test/settings-write.test.ts` (extended).

**Interfaces:**
```ts
export interface RemoteConfig { readonly listen: string; readonly tls: { mode: "selfSigned" } | { mode: "files"; certFile: string; keyFile: string }; readonly publicUrl?: string; readonly origins?: ReadonlyArray<string>; readonly hosts?: ReadonlyArray<string>; readonly source: "env" | "settings" }
export class RemoteListener { start(c: RemoteConfig): Effect.Effect<void, ServerStartError>; stop(): Effect.Effect<void>; reload(): void; info(): { endpoint: string; spki: string; mode: string } | undefined }
export function patchSettings(file: string, path: SettingsPath, value: unknown): void;   // SETTINGS_INVALID, SETTINGS_WRITE_FAILED
```
Decisions 26–29, 34.

- [ ] **Step 1: Write failing tests:** `remote_off_by_default`, `non_loopback_without_tls_refused` (start and `settings.write`), `self_signed_listener_pins_identity` (a TLS client on loopback checks the peer's SPKI against the pin), `files_mode_serves_owner_cert_and_reloads`, `bad_files_keep_old_context_and_warn`, `settings_write_applies_live` (enable, change port, disable; audited without the value), `settings_write_keeps_other_keys`, `settings_write_refuses_unknown_paths`, `settings_write_is_human_only_and_confirmed`, `fin_listen_wins_over_settings`, `listener_not_started_while_moved`, `remote_host_allow_list`, `fixed_loopback_port`, `status_lists_listeners_with_pin`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): a TLS-only remote listener, settings writes and FIN_LISTEN`.

### Task 11: Allowed roots for devices — tier: opus — after Task 1 (Task 10 for `server.projectRoots` writes)

**Files:**
- Modify: `apps/finisherd/src/chain/resolve.ts` (`rootsOf`: device roots; `ChainProjects.allowedRoots?`), `apps/finisherd/src/chain/authorize.ts` (paths of every resolver for devices), `apps/finisherd/src/main.ts` (the roots source: settings, open projects, 5D's library registry when present), `TASKS.md`.
- Test: `apps/finisherd/test/device-roots.test.ts`.

**Interfaces:**
```ts
// ChainProjects
readonly allowedRoots?: () => Effect.Effect<ReadonlyArray<string>>;   // projectRoots ∪ library roots ∪ open project folders, canonical
```
Decision 32.

- [ ] **Step 1: Write failing tests:** `device_create_outside_roots_refused`, `device_open_outside_roots_refused`, `device_probe_and_import_and_out_dir_checked`, `open_project_folder_is_a_root`, `symlink_out_of_root_refused` (canonical paths), `missing_root_skipped_not_created`, `local_and_console_keep_full_reach`, `default_project_root_is_home_finisher`, `agent_reach_unchanged`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(chain): allowed server roots for devices`.

### Task 12: Console commands — tier: sonnet — after Tasks 8–10

**Files:**
- Create: `apps/finisherd/src/console/commands/{pair.ts, devices.ts, remote.ts, tls.ts, env.ts}`.
- Modify: `apps/finisherd/src/console/main.ts` (`COMMANDS`, `HELP`), `apps/finisherd/src/console/format.ts` (QR text, pin groups), `TASKS.md`.
- Test: `apps/finisherd/test/console-remote.test.ts`.

**Commands:** `pair [--label <text>] [--ttl <d>]` (prints the link, `browserLink` when there is one and the QR code; waits on `auth.pair.pending`; for each request shows the device name, platform and safety code and asks approve, reject or skip, each answer through the console's own confirmation (5A §4.7: approving is y/N); Ctrl-C leaves the offer to expire), `devices [--all] [--json]`, `revoke <id>`, `remote enable [--listen <host:port>] [--tls self-signed | --cert <file> --key <file>] [--public-url <url>]`, `remote disable`, `remote status [--json]`, `tls show [--json]`, `tls rotate`, `env show [--json]`, `env name <name>`, `env adopt`, `env reset`. Mutations need a TTY and the console's confirmation (5A §4.7, owner decision of 2026-11-08): y/N (`revoke <id>` too, like `agent revoke`), or the target typed back for what cannot be undone: `env reset` takes the environment's name and `tls rotate` the word `tls`; the console sends the challenge code itself. `tls rotate`, `env reset` and `remote enable` say first what they do (who must pair again, that the server opens to the network, and for `self-signed` that browsers need trusted files or an SSH tunnel on the same port).

- [ ] **Step 1: Write failing tests** through the injected `Prompt`: each command's text and `--json`, confirmations, `CONSOLE_NEEDS_TTY` for every mutation, `pair` approving and rejecting two requests, `remote enable` refusing a non-loopback listen without TLS before anything is asked; `env reset` and `tls rotate` refusing a `y` where the name is asked.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(console): pair, devices, revoke, remote, tls and env commands`.

### Task 13: e2e `remote` scenario and a device test client — tier: sonnet — after Tasks 9–12

**Files:**
- Create: `scripts/e2e/{remote.mjs, device-lib.mjs}` (WebCrypto P-256 keys, `jose` proofs, the token and ticket flow, a ticketed WebSocket).
- Modify: `scripts/e2e.mjs` (`SCENARIOS.remote`), `scripts/e2e.test.mjs`, `scripts/e2e/lib.mjs`.

- [ ] **Step 1: Write the scenario:**
  1. Start `finisherd` with the test remote listener (Decision 34). `/.well-known/finisher` answers `{envId, protocol, spki}`; the TLS peer's SPKI equals it.
  2. `consoleRpc` makes an offer; the device client redeems it; a second redeem is `PAIRING_USED`; the console approves after comparing the safety code; `/pair/status` answers approved.
  3. Token without a nonce → `DPOP_NONCE`, then a token; a ws-ticket; a device WebSocket streams `ops.subscribe` of a project `fin` edits.
  4. A device's human-only call (`auth.pair.offer`) goes ahead without a code; its `server.stop` without a code → `CONFIRMATION_REQUIRED`; with a challenge's code, the server stops; restart.
  5. A device's `project.create` outside `server.projectRoots` → `PATH_NOT_ALLOWED`; inside works.
  6. Revoking the device ends its live subscription within 100 ms; its next token request is `DEVICE_REVOKED`.
  7. A copied server data started beside the original: devices refused `ENV_MOVED`; `env reset` gives it a new `envId`.
  8. **Leak grep** of every pairing secret, link fragment, safety code, token, ticket, proof, nonce and private key over `finisherd.json`, `events/`, `control.db` and its WAL, `settings.json`, `secrets/`, the server's stderr and every output: no hit (the console's own printed link excepted, only in that command's stdout).
- [ ] **Step 2: Run** `rtk pnpm e2e` (all scenarios) → PASS. **Commit** `test(e2e): remote scenario for pairing, DPoP, device tickets and revocation`.

### Task 14: Spec as built, architecture pointer, README and tracker — tier: sonnet — after Task 13

**Files:** `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` (§5 rewritten **as built**, with a §5.12 mapping every Decision of this plan to As built or Deviation, §5.13 owner decisions and review fixes, §5.14 residuals; §3.3, §3.4 and §4.17 updated; the pinned versions of `keyring`, `zbus`, `rcgen`, `qrcode` and `jose`), `docs/specs/2026-11-01-finisher-architecture-design.md` (a §27 item "Sub-project 5B as built"; §16's pinning sentence points at owner decision 3), `README.md` ("Remote access": off by default, `remote enable`, trusted certificate files or an SSH tunnel on the same port for browsers, self-signed pins for the app, pairing and safety codes, revoking, `env adopt|reset`, `FIN_LISTEN` and the Docker notes, `FIN_SECRETS` and the keychain; "Security model": devices and their confirmations, what the code does not stop in a compromised renderer, the raw-shell residual for pairing), `TASKS.md`.

- [ ] **Step 1:** Write the as-built notes from the code; list every deviation.
- [ ] **Step 2: TASKS:** tick the 5B lines; record residuals (Decision 23's renderer statement, Decision 29's same-port tunnels, the unwrapped file store) and follow-ups for sub-project 9 (the device key in the keychain, the pairing pages, `TLS_PIN_MISMATCH`, the confirmation dialog).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `docs: record sub-project 5B as built`.

### Task 15: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

On macOS, Windows 11 and the Linux VM, as a non-elevated user; host details in `archive/hosts.md`.

- [ ] **Step 1:** `cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e` on each OS.
- [ ] **Step 2: Secret stores:** `FIN_TEST_KEYCHAIN=1` round trips on the macOS Keychain (a signed build, and the prompt an unsigned one gets, recorded), Windows Credential Manager (the identity PEM under its 2,560-byte limit), the Linux Secret Service unlocked, locked (falls back without a prompt) and absent (headless); the wrapped file store under Docker with `FIN_SECRETS_KEY_FILE`.
- [ ] **Step 3: Cross-host `remote`:** the e2e scenario's device client on a second host over `lan` (self-signed, pinned) and over `tailscale` with `tailscale cert` files (WebPKI); the macOS and Windows firewall prompts at the first non-loopback listen (expected; recorded); a browser on another host through an SSH tunnel on the same port; a browser on a trusted-certificate route; Docker with `FIN_LISTEN` and `publicUrl`.
- [ ] **Step 4: Measure** and record (TASKS and spec): DPoP verification p50/p99, token and ticket round trips over TLS on the LAN, the cost of a certificate remake.
- [ ] **Step 5:** Fix what fails (one commit each, with a test); re-run Step 1.

---

## Questions for the owner — Answered

All four were answered on 2026-11-08:

1. **Device confirmations (Decision 22). Answered (owner):** only `server.stop` from a device needs a confirmation, through the app's dialog. No other human-only action from a device is confirmed by the chain. Task 4 matches.
2. **Ports and SSH tunnels (Decision 29). Answered (coordinator):** as written.
3. **Self-pairing of the desktop renderer (Decision 21). Answered (coordinator):** as written.
4. **Trusted-certificate routes (Decision 9). Answered (coordinator):** as written. There is no second, pinned listener.
