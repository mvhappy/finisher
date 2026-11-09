// The "remote" scenario (plan 5B Task 13): pairing, DPoP, device tickets and revocation end to end
// against a real `finisherd` whose remote listener runs on loopback (`FIN_LISTEN=127.0.0.1:0`,
// self-signed), with a device client written apart from the server (`device-lib.mjs`):
//   1. `/.well-known/finisher` answers `{envId, protocol, spki}`; the TLS peer's pin equals it;
//   2. the console's offer, the device's redeem, a second redeem `PAIRING_USED`, the safety code
//      the console sees equals the one the device works out, approval, `/pair/status` approved;
//   3. a token without a nonce is `DPOP_NONCE`, then a token, a ticket, and a device WebSocket
//      over `wss` that streams `ops.subscribe` of a project `fin` edits;
//   4. a device's human-only call (`auth.pair.offer`) goes ahead without a code; its `server.stop`
//      without one is `CONFIRMATION_REQUIRED`, with its own connection's code the server stops;
//   5. after a restart: `project.create` outside `server.projectRoots` is `PATH_NOT_ALLOWED`,
//      inside works;
//   6. revoking the device ends its live subscription within 100 ms; its next token request is
//      `DEVICE_REVOKED`;
//   7. a copy of the server data started beside the original: devices are `ENV_MOVED`, its remote
//      listener stays off, and `env reset` gives it a new `envId`;
//   8. the leak grep: no pairing secret, link fragment, safety code, token, ticket, proof, nonce,
//      private key or challenge code in `finisherd.json`, `events/`, `control.db` and its WAL,
//      `settings.json`, `secrets/`, the servers' stderr or any output.
import { createHmac } from "node:crypto";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join, sep } from "node:path";
import { writeFixture } from "./fixture.mjs";
import { deviceSecrets, httpsCall, newDeviceKey, redeem, RemoteDevice } from "./device-lib.mjs";
import { assertEqual, consoleRpc, errorCode, fin, live, log, observed, outputs, proveConnection, removeTmp, scenarioTmp, startServer, stopServer, waitFor } from "./lib.mjs";
import { secretHits } from "./auth-lib.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);
const REMOTE_ENV = { FIN_LISTEN: "127.0.0.1:0" };

/** The `wss` listener's port and pin, from `server.status`. */
async function remoteOf(server) {
  const { listeners } = await consoleRpc(server, "server.status", {}, { confirm: false });
  const wss = listeners.find((l) => l.transport === "wss");
  if (wss === undefined) throw new Error(`no wss listener: ${JSON.stringify(listeners)}`);
  return { port: Number(new URL(wss.endpoint.replace(/^wss:/, "https:")).port), pin: wss.spki };
}

/** Every file under `dir` (bytes as latin1), as grep sources. */
function filesIn(dir, name) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir, { recursive: true })
    .map((f) => join(dir, String(f)))
    .filter((f) => statSync(f).isFile())
    .map((f) => ({ name: `${name}/${f.slice(dir.length + 1)}`, text: readFileSync(f, "latin1") }));
}

export async function run() {
  const started = Date.now();
  observed.clear();
  outputs.length = 0;
  deviceSecrets.clear();
  const tmp = scenarioTmp("finisher-e2e-remote-");
  live.tmp = tmp;
  const sd = join(tmp, "s");
  const roots = join(tmp, "roots");
  const project = join(roots, "remote project");
  mkdirSync(sd, { recursive: true, mode: 0o700 });
  mkdirSync(roots);
  writeFileSync(join(sd, "settings.json"), JSON.stringify({ server: { projectRoots: [roots] } }));
  const servers = [];
  const stderrs = [];
  const needles = new Set();
  try {
    let server = await startServer(sd, REMOTE_ENV);
    servers.push(server);
    const envId = server.info.envId;

    step(1, "the well-known descriptor and the TLS pin");
    let { port, pin } = await remoteOf(server);
    const known = await httpsCall(port, "GET", "/.well-known/finisher");
    assertEqual([known.status, Object.keys(known.body).sort()], [200, ["envId", "protocol", "spki"]], "/.well-known/finisher");
    assertEqual([known.body.envId, known.body.spki, known.pin], [envId, pin, pin], "the descriptor's pin is the TLS peer's");

    step(2, "offer, redeem, a second redeem, the safety code, approval");
    const offer = await consoleRpc(server, "auth.pair.offer", { label: "Studio tablet" });
    const addLink = (link) => {
      const fragment = link.slice(link.indexOf("#") + 1);
      // The secret (the offer's id is no secret: `control.db` keeps it).
      needles.add(fragment);
      needles.add(fragment.slice(fragment.indexOf(".") + 1));
    };
    addLink(offer.link);
    const key = await newDeviceKey();
    const redeemed = await redeem(port, pin, offer.link, "E2E tablet", key);
    assertEqual(redeemed.status, 200, `the redeem (${JSON.stringify(redeemed.body)})`);
    const again = await redeem(port, pin, offer.link, "Another", await newDeviceKey());
    assertEqual([again.status, again.body.error?.code], [409, "PAIRING_USED"], "a second redeem");
    // The console's view of the request, from `auth.pair.pending`'s snapshot.
    const watcher = await proveConnection(server, "console");
    let pending;
    void watcher.stream("auth.pair.pending", {}, (values) => {
      for (const v of values) if (v.kind === "snapshot") pending = v.pending;
    });
    await waitFor("the pending requests' snapshot", () => pending !== undefined, 10_000);
    watcher.close();
    const view = pending.find((p) => p.pendingId === redeemed.body.pendingId);
    const secret = Buffer.from(offer.link.slice(offer.link.indexOf(".", offer.link.indexOf("#")) + 1), "base64url");
    const mine = String(createHmac("sha256", secret).update(key.jkt, "utf8").digest().readUInt32BE(0) % 1_000_000).padStart(6, "0");
    assertEqual([view?.deviceName, view?.safetyCode, redeemed.body.safetyCode], ["E2E tablet", mine, mine], "both sides show the same safety code");
    needles.add(mine);
    const approved = await consoleRpc(server, "auth.pair.approve", { pendingId: view.pendingId });
    const deviceId = approved.device.id;
    const device = new RemoteDevice(port, pin, key);
    const status = await device.call("/pair/status", { pendingId: view.pendingId });
    assertEqual([status.status, status.body.state, status.body.deviceId, status.body.environment?.envId], [200, "approved", deviceId, envId], "/pair/status");

    step(3, "DPoP nonce, token, ticket and a device WebSocket streaming fin's edits");
    const fresh = new RemoteDevice(port, pin, key);
    const noNonce = await fresh.call("/auth/token", {}, { retry: false });
    assertEqual([noNonce.status, noNonce.body.error?.code], [401, "DPOP_NONCE"], "a token without the nonce");
    await fresh.token();
    const local = (args, cwd = project) => fin(sd, cwd, args);
    const projectId = local(["project", "create", project, "--name", "Remote"], tmp).project.projectId;
    let ws = await device.ws();
    const seen = [];
    const { value: snap } = await ws.exit("snapshot.get", { projectId, branch: "main" });
    ws.subscribe({ projectId, branch: "main", fromRev: 0, generation: snap.generation }, (tx) => seen.push(tx.rev));
    await writeFixture(join(project, "clip.mp4"));
    local(["asset", "add", "clip.mp4"]);
    local(["clip", "insert", "--no-audio", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "2s"]);
    local(["marker", "add", "--at", "1s", "--label", "remote"]);
    await waitFor("the device's stream", () => seen.length >= 3, 15_000, () => ` (saw ${JSON.stringify(seen)})`);
    assertEqual(seen, [...seen.keys()].map((i) => i + 1), "the device's revs, in order");

    step(4, "a device's human-only calls: an offer without a code, server.stop with its own");
    const deviceOffer = await ws.exit("auth.pair.offer", {});
    assertEqual(deviceOffer._tag, "Success", `a device's offer (${JSON.stringify(deviceOffer)})`);
    addLink(deviceOffer.value.link);
    assertEqual(errorCode(await ws.exit("server.stop", {})), "CONFIRMATION_REQUIRED", "server.stop without a code");
    const { value: challenge } = await ws.exit("console.challenge", {});
    needles.add(challenge.code);
    assertEqual((await ws.exit("server.stop", { challenge: challenge.code }))._tag, "Success", "server.stop with the device's code");
    await waitFor("the server to stop", () => server.child.exitCode !== null, 15_000);
    stderrs.push(server.stderr());
    server = await startServer(sd, REMOTE_ENV);
    servers.push(server);
    ({ port, pin } = await remoteOf(server));
    assertEqual(pin, known.body.spki, "the pin survives the restart");

    step(5, "allowed roots: outside refused, inside works");
    const back = new RemoteDevice(port, pin, key);
    ws = await back.ws();
    assertEqual(errorCode(await ws.exit("project.create", { dir: join(tmp, "outside"), name: "O", rate: "30" })), "PATH_NOT_ALLOWED", "a project outside the roots");
    assertEqual((await ws.exit("project.create", { dir: join(roots, "inside"), name: "I", rate: "30" }))._tag, "Success", "a project inside the roots");

    step(6, "revoking ends the live subscription within 100 ms; the next token is DEVICE_REVOKED");
    const { value: snap2 } = await ws.exit("snapshot.get", { projectId, branch: "main" });
    const live2 = [];
    ws.subscribe({ projectId, branch: "main", fromRev: 0, generation: snap2.generation }, (tx) => live2.push(tx.rev));
    await waitFor("the subscription to replay", () => live2.length >= 3, 10_000);
    // The console's connection and code first; the clock starts as the revoke is sent.
    const owner = await proveConnection(server, "console");
    const code = (await owner.call("console.challenge", {})).value.code;
    needles.add(code);
    const at = performance.now();
    const revoked = await owner.call("auth.device.revoke", { id: deviceId, challenge: code });
    owner.close();
    assertEqual(revoked.value, { revoked: true }, "the revoke");
    const tookMs = (await ws.closed) - at;
    assertEqual(tookMs >= 0 && tookMs < 100, true, `the subscription ended ${tookMs.toFixed(1)} ms after the revoke was sent`);
    const after = await back.call("/auth/token", {});
    assertEqual([after.status, after.body.error?.code], [401, "DEVICE_REVOKED"], "a revoked device's token request");

    step(7, "a copy of the server data: ENV_MOVED, no remote listener, env reset");
    const key2 = await newDeviceKey();
    const offer2 = await consoleRpc(server, "auth.pair.offer", {});
    addLink(offer2.link);
    const r2 = await redeem(port, pin, offer2.link, "Second tablet", key2);
    needles.add(r2.body.safetyCode);
    await consoleRpc(server, "auth.pair.approve", { pendingId: r2.body.pendingId });
    await stopServer(server);
    stderrs.push(server.stderr());
    const copy = join(tmp, "copy");
    cpSync(sd, copy, { recursive: true, filter: (src) => !src.endsWith("finisherd.json") && src !== join(sd, "run") && !src.startsWith(join(sd, "run") + sep) });
    server = await startServer(sd, REMOTE_ENV);
    servers.push(server);
    const twin = await startServer(copy, REMOTE_ENV);
    servers.push(twin);
    const { listeners } = await consoleRpc(twin, "server.status", {}, { confirm: false });
    assertEqual(listeners.some((l) => l.transport === "wss"), false, "a moved copy runs no remote listener");
    const moved = await new RemoteDevice(twin.info.wsPort, null, key2).call("/auth/token", {});
    assertEqual([moved.status, moved.body.error?.code], [403, "ENV_MOVED"], "a device at the copy");
    const reset = await consoleRpc(twin, "env.reset", {});
    assertEqual(reset.envId !== envId && reset.devices === 1, true, `env reset (${JSON.stringify({ ...reset, envId: "…" })})`);
    ({ port, pin } = await remoteOf(server));
    assertEqual((await new RemoteDevice(port, pin, key2).token()).length > 20, true, "the original still serves its device");

    step(8, "the leak grep");
    for (const s of deviceSecrets) needles.add(s);
    for (const s of observed) needles.add(s);
    // Every server stopped first: their last writes and shutdown output count too.
    for (const s of servers) await stopServer(s);
    const sources = [
      ...outputs,
      ...filesIn(sd, "original"),
      ...filesIn(copy, "copy"),
      ...[...stderrs, server.stderr(), twin.stderr()].map((text, i) => ({ name: `finisherd stderr ${i}`, text })),
    ];
    const hits = secretHits(sources, needles);
    assertEqual(hits, [], "pairing or device secrets found in the server's files or logs");
    log(`leak grep: ${needles.size} needles over ${sources.length} sources, no hit`);
    log(`PASS in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    for (const s of servers) await stopServer(s);
    removeTmp(tmp);
  }
}
