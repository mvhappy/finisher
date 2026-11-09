// The "auth" scenario (5A Task 12): principals, the handshake, the console, tickets and
// credentials end to end, over `fin.sock` (or the Windows pipe) as well as the WebSocket.
//   1. an unproved raw connection: `snapshot.get` is `UNAUTHENTICATED`, `server.hello` answers,
//      `run/` holds only the socket;
//   2. `fin` (local): create, three edits and an undo; `fin whoami` says `local`;
//   3. human-only: `fin` is refused (exit 7 `HUMAN_ONLY`, fixes without a command line), the
//      console without a terminal is `CONSOLE_NEEDS_TTY`, training on is `CONSENT_UI_ONLY`, and
//      the console's challenge code (`consoleRpc`) purges; `finisherd audit` lists the denials;
//   4. a ticketed WebSocket streams step 2's ops; a reused ticket and a ticket in the query are
//      401, Host and Origin are 403 and spend nothing, another protocol is 426;
//   5. agent credentials minted through the console: read-only (`SCOPE_DENIED`), write with
//      `branch own` (`BRANCH_NOT_ALLOWED` on main, its own branch works, its journal entries carry
//      the credential id as `actor.session`), `TICKET_LIMIT`, revoking ends a live stream and its
//      tickets and the next run is `CREDENTIAL_REVOKED`;
//   6. a squatted socket is `SERVER_UNTRUSTED` and sees only the hello; a restart: the old
//      `local.key` no longer proves, `fin` runs across it never untrusted, a credential past its
//      expiry is `CREDENTIAL_EXPIRED`, a revoked one stays revoked;
//   7. the audit rows, and the leak grep over `finisherd.json`, `events/`, `control.db`, the
//      server's stderr, the audit export and every `fin` and console output: no key, ticket, MAC,
//      nonce, proof or challenge code.
// Windows: the run's TEMP and TMP are an owner-only folder. `peer.sid` in an audit row is a
// token user, not a secret, and is not among the needles.
import { randomBytes } from "node:crypto";
import { createServer } from "node:net";
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { writeFixture } from "./fixture.mjs";
import { keyer } from "./diagnostics-lib.mjs";
import {
  FIN,
  WIN,
  assertEqual,
  connectWs,
  consoleRpc,
  errorCode,
  fin,
  finRaw,
  live,
  log,
  observed,
  outputs,
  proveConnection,
  rawConnect,
  removeTmp,
  scenarioTmp,
  socketCall,
  startServer,
  stopServer,
  storedEntries,
  waitFor,
} from "./lib.mjs";
import { consoleCli, expireCredentialOffline, finAsync, privateTemp, secretHits, serverFiles, writeCredentialFile, writePrivate, wsUpgrade } from "./auth-lib.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
/** A fix text must not hand an agent a command line to run (5A Decision: fixes address the user). */
const COMMAND_IN_FIX = /`|\bfinisherd [a-z-]+|\bfin [a-z-]+/;

export async function run() {
  const started = Date.now();
  observed.clear();
  outputs.length = 0;
  const tmp = scenarioTmp("finisher-e2e-auth-");
  live.tmp = tmp;
  const temp = privateTemp(join(tmp, "t"));
  const tempEnv = WIN ? { TEMP: temp, TMP: temp } : {};
  const sd = join(tmp, "s");
  const project = join(tmp, "auth project");
  const servers = [];
  const sockets = [];
  const stderrs = [];
  let server;
  let squatter;
  try {
    server = await startServer(sd, tempEnv);
    servers.push(server);
    /** `fin` as `local` (no credential file), expecting success. */
    const local = (args, cwd = project) => fin(sd, cwd, args, tempEnv);
    /** `fin` with a credential file, whatever the exit. */
    // Under an agent credential `fin` reads no server data: the endpoint is FIN_SERVER alone
    // (7A Decision 14b) and the project folder FIN_PROJECT_DIR alone (the 8b review).
    const as = (file, args) => finRaw(sd, project, args, { ...tempEnv, FIN_CREDENTIAL: file, FIN_SERVER: server.info.socket, FIN_PROJECT_DIR: project });
    const refused = (r, code, what, status = 7) => {
      assertEqual([r.status, r.json?.error?.code], [status, code], what);
      if (r.json.error.fix !== undefined) assertEqual(COMMAND_IN_FIX.test(r.json.error.fix), false, `${what}: the fix names a command: ${r.json.error.fix}`);
      return r.json.error;
    };

    step(1, "an unproved connection can only say hello; run/ holds only the socket");
    {
      const raw = await rawConnect(server);
      sockets.push(raw);
      const denied = await raw.call("snapshot.get", { projectId: "none", branch: "main" });
      assertEqual(errorCode(denied), "UNAUTHENTICATED", "snapshot.get before the proof");
      const hello = await raw.call("server.hello", {});
      assertEqual([hello._tag, typeof hello.value?.protocolVersion], ["Success", "number"], "server.hello answers");
      // A MAC that is not the credential's proves nothing, and five of them end the connection.
      for (let i = 0; i < 5; i++) {
        const bad = await raw.call("auth.prove", { mac: Buffer.alloc(32).toString("base64url") });
        if (bad._tag === "Success") throw new Error("a zero MAC proved a credential");
      }
      await Promise.race([raw.closed, sleep(5_000).then(() => Promise.reject(new Error("five bad proofs did not close the connection")))]);
      if (!WIN) assertEqual(readdirSync(join(sd, "run")), ["fin.sock"], "run/ holds only the socket");
    }

    step(2, "fin as local: create, three edits and an undo");
    const created = local(["project", "create", project, "--name", "Auth"], tmp);
    const projectId = created.project.projectId;
    await writeFixture(join(project, "clip.mp4"));
    local(["asset", "add", "clip.mp4"]);
    local(["clip", "insert", "--no-audio", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "2s"]);
    local(["marker", "add", "--at", "1s", "--label", "auth"]);
    const undone = local(["undo"]);
    const who = local(["whoami"]);
    assertEqual([who.principal.kind, who.principal.human], ["local", false], "fin whoami");
    const headRev = (await socketCall(server, "snapshot.get", { projectId, branch: "main" }, "local")).rev;
    assertEqual(headRev >= 3, true, `rev after the edits and the undo (${undone.rev})`);

    step(3, "human-only: fin is refused, the console needs a terminal and a challenge code");
    const humanOnly = (args) => refused(finRaw(sd, project, args, tempEnv), "HUMAN_ONLY", `fin ${args.join(" ")}`);
    humanOnly(["diagnostics", "purge", "--all"]);
    humanOnly(["diagnostics", "consent", "diagnostics", "off"]);
    humanOnly(["project", "takeover", "--confirm"]);
    const training = consoleCli(sd, ["diagnostics", "consent", "training", "on"]);
    assertEqual([training.status, training.code], [2, "CONSENT_UI_ONLY"], "training on from the console");
    const noTty = consoleCli(sd, ["diagnostics", "purge", "--all"]);
    assertEqual([noTty.status, noTty.code], [7, "CONSOLE_NEEDS_TTY"], "a purge without a terminal");
    assertEqual((await storedEntries(server)).length > 0, true, "the journal holds entries before the purge");
    // Entries up to now (the install key stays: step 5 reads the journal by it; `all` ends the scenario).
    const purged = await consoleRpc(server, "diagnostics.purge", { selector: { before: Date.now() + 1 } });
    log(`console purge with a challenge code: ${JSON.stringify(purged)}`);
    assertEqual((await storedEntries(server)).length, 0, "the journal after the purge");
    const denials = JSON.parse(consoleCli(sd, ["audit", "--json", "--kind", "auth.denied"]).stdout).entries;
    assertEqual(
      denials.filter((e) => e.code === "HUMAN_ONLY" && e.principalKind === "local").length >= 3,
      true,
      `audit lists the three HUMAN_ONLY denials (${denials.map((e) => e.code)})`,
    );

    step(4, "a ticketed WebSocket streams the ops; tickets are one-use and bound to Host and Origin");
    const ws = await connectWs(server);
    const seen = [];
    const { generation } = await ws.call("snapshot.get", { projectId, branch: "main" });
    const sub = ws.subscribe({ projectId, branch: "main", fromRev: 0, generation }, (tx) => seen.push(tx.rev));
    await waitFor("the stream to catch up", () => seen.length >= headRev, 15_000, () => ` (saw ${JSON.stringify(seen)})`);
    assertEqual(seen, [...Array(seen.length).keys()].map((i) => i + 1), "the stream's revs, in order");
    ws.interrupt(sub.id);
    await sub.done;
    ws.close();
    const port = server.info.wsPort;
    const ticket = async () => (await socketCall(server, "auth.ticket", {}, "local")).ticket;
    const protocols = (t) => ["finisher.v2", `finisher.ticket.${t}`];
    const t1 = await ticket();
    assertEqual((await wsUpgrade(server, { protocols: protocols(t1) })).status, 101, "a fresh ticket upgrades");
    assertEqual((await wsUpgrade(server, { protocols: protocols(t1) })).status, 401, "the same ticket again");
    const t2 = await ticket();
    assertEqual((await wsUpgrade(server, { protocols: ["finisher.v2"], path: `/rpc?ticket=${t2}` })).status, 401, "a ticket in the query string");
    assertEqual((await wsUpgrade(server, { protocols: protocols(t2), host: `evil.example:${port}` })).status, 403, "a foreign Host");
    assertEqual((await wsUpgrade(server, { protocols: protocols(t2), host: `127.0.0.1:${port + 1}` })).status, 403, "a Host with another port");
    assertEqual((await wsUpgrade(server, { protocols: protocols(t2), origin: "http://evil.example" })).status, 403, "a foreign Origin");
    const old = await wsUpgrade(server, { protocols: ["finisher.v1", `finisher.ticket.${t2}`] });
    assertEqual([old.status, old.body.includes('"max":2')], [426, true], "another protocol");
    assertEqual((await wsUpgrade(server, { protocols: protocols(t2), origin: `http://localhost:${port}` })).status, 101, "the refusals spent nothing: t2 still upgrades, with a local Origin");
    assertEqual((await wsUpgrade(server, { protocols: protocols(t2) })).status, 401, "t2 is used up");

    step(5, "agent credentials: scopes, branch own, journal actors, TICKET_LIMIT, revocation");
    const mint = async (label, opts) => {
      const minted = await consoleRpc(server, "auth.credential.mint", { kind: "agent", label, ...opts });
      const path = writeCredentialFile(join(tmp, `${label}.cred`), minted.file);
      return { id: minted.credential.id, file: minted.file, path };
    };
    const reader = await mint("reader", { scopes: ["project.read"], branches: ["own"] });
    const writer = await mint("writer", { scopes: ["server.read", "project.read", "project.write"], branches: ["own"] });
    assertEqual(as(reader.path, ["whoami"]).json.principal.kind, "agent", "fin whoami with a credential");
    assertEqual(as(reader.path, ["view"]).status, 0, "the reader views");
    refused(as(reader.path, ["marker", "add", "--at", "0", "--label", "x"]), "SCOPE_DENIED", "a read-only credential edits");
    refused(as(reader.path, ["diagnostics", "purge", "--all"]), "HUMAN_ONLY", "an agent purges (human-only before scopes)");
    refused(as(writer.path, ["diagnostics", "consent", "diagnostics", "off"]), "HUMAN_ONLY", "an agent changes consent");
    const mainRev = async () => (await socketCall(server, "snapshot.get", { projectId, branch: "main" }, "local")).rev;
    const before = await mainRev();
    refused(as(writer.path, ["marker", "add", "--at", "0", "--label", "x"]), "BRANCH_NOT_ALLOWED", "an own-branch credential edits main");
    assertEqual(await mainRev(), before, "the refused edit wrote nothing to main");
    as(writer.path, ["branch", "new", "mine"]);
    const mineEdit = as(writer.path, ["--branch", "mine", "marker", "add", "--at", "1s", "--label", "mine"]);
    assertEqual(mineEdit.status, 0, `the writer edits its own branch: ${mineEdit.text}`);
    const beforeNamed = await mainRev();
    refused(as(writer.path, ["--branch", "main", "marker", "add", "--at", "0", "--label", "x"]), "BRANCH_NOT_ALLOWED", "…and main by name");
    assertEqual(await mainRev(), beforeNamed, "…and wrote nothing either");
    // The journal keys any id that is not an upper-case ULID (a credential's is not): its install key gives the reference.
    const keys = await keyer(sd, join(tmp, "keyer"));
    const refs = { [writer.id]: keys.idRef(writer.id), [reader.id]: keys.idRef(reader.id) };
    keys.close();
    const refOf = (id) => refs[id];
    const journalOf = async (id) => {
      for (let i = 0; i < 200; i++) {
        const all = await storedEntries(server, { kinds: ["log.commit", "cli.command"] });
        const mine = all.filter((e) => e.ctx?.actor?.session === refOf(id));
        if (mine.some((e) => e.kind === "log.commit")) return { all, mine };
        await sleep(50);
      }
      throw new Error(`no log.commit with actor.session ${id} (${refOf(id)}): ${JSON.stringify((await storedEntries(server, { kinds: ["log.commit"] })).map((e) => e.ctx?.actor))}`);
    };
    const { all: journal, mine } = await journalOf(writer.id);
    assertEqual(mine.every((e) => e.ctx.actor.kind === "agent"), true, "the writer's entries are agent actors");
    assertEqual(
      journal.filter((e) => e.kind === "log.commit" && e.ctx?.actor?.session === refOf(reader.id)).length,
      0,
      "the reader committed nothing",
    );

    // TICKET_LIMIT: one connection's mint rate (burst 16) is spent, and the next connection is fine.
    {
      const conn = await proveConnection(server, "local");
      let at;
      let limited;
      for (let i = 1; i <= 40 && limited === undefined; i++) {
        const exit = await conn.call("auth.ticket", {});
        if (exit._tag === "Success") observed.add(exit.value.ticket);
        else (limited = errorCode(exit)), (at = i);
      }
      conn.close();
      assertEqual(limited, "TICKET_LIMIT", "a burst of tickets on one connection");
      log(`TICKET_LIMIT at ticket ${at}`);
      assertEqual(typeof (await socketCall(server, "auth.ticket", {}, "local")).ticket, "string", "another connection still mints");
    }

    // Revocation ends a live stream and the credential's tickets.
    const wsW = await connectWs(server, writer.file);
    const { generation: g2 } = await wsW.call("snapshot.get", { projectId, branch: "main" });
    const seenW = [];
    const subW = wsW.subscribe({ projectId, branch: "main", fromRev: 0, generation: g2 }, (tx) => seenW.push(tx.rev));
    await waitFor("the writer's stream", () => seenW.length > 0, 15_000);
    const pending = await socketCall(server, "auth.ticket", {}, writer.file);
    assertEqual((await consoleRpc(server, "auth.credential.revoke", { id: writer.id })).revoked, true, "revoke");
    const ended = await Promise.race([subW.done, wsW.closed.then(() => ({ _tag: "Closed" })), sleep(10_000).then(() => "still open")]);
    if (ended === "still open") throw new Error("the revoked credential's stream is still open");
    log(`the live stream ended: ${JSON.stringify(ended).slice(0, 120)}`);
    assertEqual((await wsUpgrade(server, { protocols: protocols(pending.ticket) })).status, 401, "a revoked credential's ticket");
    refused(as(writer.path, ["view"]), "CREDENTIAL_REVOKED", "the next run after the revoke");
    assertEqual((await consoleRpc(server, "auth.credential.revoke", { id: writer.id })).revoked, false, "a second revoke");
    const expiring = await mint("expiring", { scopes: ["project.read"] });
    assertEqual(as(expiring.path, ["whoami"]).status, 0, "the credential to expire works");

    step("5b", "jobs: fin look --no-wait then fin wait and fin job; an agent sees and controls only its own jobs");
    const jobber = await mint("jobber", { scopes: ["server.read", "project.read", "jobs.read", "jobs.submit"], projects: [projectId] });
    {
      const id = local(["look", "--at", "0", "--size", "16x16", "--no-wait"]).job.id;
      const waited = finRaw(sd, project, ["wait", id, "--timeout", "120"], tempEnv);
      // Without a GPU adapter the look fails GPU_UNAVAILABLE, which is still a job that ended.
      if (waited.status !== 0) {
        assertEqual([waited.status, waited.json?.error?.code, waited.json?.error?.details?.failed?.[0]?.code], [4, "JOB_FAILED", "GPU_UNAVAILABLE"], `fin wait: ${waited.text}`);
      }
      assertEqual(local(["job", "show", id]).state, waited.status === 0 ? "succeeded" : "failed", "fin job show agrees with fin wait");
      assertEqual(local(["job", "pause", "--all"]), {}, "local pauses dispatch");
      const queued = local(["look", "--at", "0", "--size", "17x17", "--no-wait"]).job.id;
      assertEqual(local(["job", "ls", "--state", "queued"]).jobs.map((j) => j.id), [queued], "the queued look");
      refused(finRaw(sd, project, ["wait", queued, "--timeout", "1"], tempEnv), "WAIT_TIMEOUT", "a wait that times out", 5);
      refused(as(jobber.path, ["job", "show", queued]), "JOB_NOT_FOUND", "an agent reads local's job", 3);
      refused(as(jobber.path, ["job", "cancel", queued]), "JOB_NOT_FOUND", "an agent cancels local's job", 3);
      refused(as(jobber.path, ["wait", queued, "--timeout", "1"]), "JOB_NOT_FOUND", "an agent waits on local's job", 3);
      refused(as(jobber.path, ["job", "pause", "--all"]), "SCOPE_DENIED", "an agent pauses every job");
      assertEqual(as(jobber.path, ["job", "ls"]).json.jobs, [], "an agent lists none of local's jobs");
      const own = as(jobber.path, ["look", "--at", "0", "--size", "18x18", "--no-wait"]);
      assertEqual(own.status, 0, `the agent's own look: ${own.text}`);
      assertEqual(as(jobber.path, ["job", "show", own.json.job.id]).json.owner.kind, "agent", "the agent's job is its own");
      assertEqual(as(jobber.path, ["job", "cancel", own.json.job.id]).json.state, "cancelled", "the agent cancels its own job");
      assertEqual(local(["job", "cancel", queued]).state, "cancelled", "local cancels its job");
      refused(finRaw(sd, project, ["wait", queued], tempEnv), "JOB_FAILED", "waiting on a cancelled job", 4);
      assertEqual(local(["job", "resume", "--all"]), {}, "local resumes dispatch");
      const status = await consoleRpc(server, "server.status", {}, { confirm: false });
      assertEqual([status.jobs.caps.sandboxed, status.warnings.some((w) => w.code === "SANDBOX_DISABLED")], [false, true], "server.status: job caps and the sandbox warning");
    }

    step(6, "a squatted socket is untrusted; a restart rotates the keys; expiry; fin across the restart");
    if (!WIN) {
      // A squatter listens where a stale `finisherd.json` points and answers the hello without a proof.
      const squat = join(tmp, "squat");
      const seenTags = [];
      mkdirSync(join(squat, "auth"), { recursive: true, mode: 0o700 });
      copyFileSync(join(sd, "auth", "local.key"), join(squat, "auth", "local.key"));
      const squatSocket = join(tmp, "q.sock");
      squatter = createServer((sock) => {
        let buf = "";
        sock.setEncoding("utf8").on("data", (d) => {
          buf += d;
          let nl;
          while ((nl = buf.indexOf("\n")) >= 0) {
            const frame = JSON.parse(buf.slice(0, nl));
            buf = buf.slice(nl + 1);
            if (frame._tag !== "Request") continue;
            seenTags.push(frame.tag);
            const value = { protocolVersion: 2, serverVersion: "0", capabilities: [], envId: server.info.envId, endpoint: squatSocket, serverNonce: randomBytes(32).toString("base64url") };
            sock.write(`${JSON.stringify({ _tag: "Exit", requestId: frame.id, exit: { _tag: "Success", value } })}\n`);
          }
        });
        sock.on("error", () => {});
      });
      await new Promise((resolve) => squatter.listen(squatSocket, resolve));
      writePrivate(join(squat, "finisherd.json"), `${JSON.stringify({ ...server.info, socket: squatSocket })}\n`);
      // Asynchronous: the squatter lives in this process, which a blocking spawn would freeze.
      refused(await finAsync(FIN, squat, project, ["whoami"], tempEnv), "SERVER_UNTRUSTED", "fin against a squatted socket");
      assertEqual(
        seenTags.every((t) => t === "server.hello") && seenTags.length >= 1,
        true,
        `the squatter saw only hellos: ${JSON.stringify(seenTags)}`,
      );
    }
    const oldLocalKey = readFileSync(join(sd, "auth", "local.key"), "utf8");
    const stale = join(tmp, "stale");
    mkdirSync(join(stale, "auth"), { recursive: true, mode: 0o700 });
    await stopServer(server);
    // Across the restart: `fin` runs back to back; none may be untrusted or unauthenticated.
    expireCredentialOffline(sd, expiring.id, Date.now() - 1_000);
    let stop = false;
    const results = [];
    const loop = (async () => {
      while (!stop) {
        results.push(await finAsync(FIN, sd, project, ["whoami"], tempEnv));
        await sleep(5);
      }
    })();
    await sleep(100);
    stderrs.push(server.stderr());
    server = await startServer(sd, tempEnv);
    servers.push(server);
    await sleep(300);
    stop = true;
    await loop;
    const codes = results.map((r) => (r.status === 0 ? "ok" : (r.json?.error?.code ?? `exit ${r.status}`)));
    const bad = codes.filter((c) => c !== "ok" && c !== "SERVER_UNAVAILABLE");
    assertEqual(bad, [], `fin across the restart: ${JSON.stringify(codes)}`);
    assertEqual(codes.at(-1), "ok", "fin after the restart");
    log(`${results.length} fin runs across the restart: ${codes.filter((c) => c === "ok").length} ok, ${results.filter((r) => r.json?.retried).length} through the retry`);
    if (!WIN) assertEqual(readdirSync(join(sd, "run")), ["fin.sock"], "run/ after the restart");

    // The old `local.key` (and the old file's key) no longer proves.
    const staleKeyPath = join(stale, "auth", "local.key");
    writePrivate(staleKeyPath, oldLocalKey);
    const newLocal = readFileSync(join(sd, "auth", "local.key"), "utf8");
    assertEqual(newLocal === oldLocalKey, false, "local.key was rotated at the restart");
    await socketCall(server, "server.status", {}, "local", { keyFile: staleKeyPath }).then(
      () => Promise.reject(new Error("the old local.key proved")),
      (e) => assertEqual(/did not prove itself|failed/.test(String(e)), true, `the old key: ${e}`),
    );
    writePrivate(join(stale, "finisherd.json"), `${JSON.stringify(server.info)}\n`);
    refused(finRaw(stale, project, ["whoami"], tempEnv), "SERVER_UNTRUSTED", "fin with the old key");
    assertEqual(local(["whoami"]).principal.kind, "local", "fin after the restart");
    assertEqual(as(reader.path, ["whoami"]).json.principal.kind, "agent", "a live credential survives the restart");
    refused(as(writer.path, ["view"]), "CREDENTIAL_REVOKED", "a revoked credential stays revoked");
    refused(as(expiring.path, ["view"]), "CREDENTIAL_EXPIRED", "an expired credential");

    step(7, "audit rows and the leak grep");
    const audit = [];
    for (let cursor; ; ) {
      const page = await consoleRpc(server, "auth.audit.list", { limit: 1000, ...(cursor ? { cursor } : {}) }, { confirm: false });
      audit.push(...page.entries);
      cursor = page.next;
      if (cursor === undefined) break;
    }
    const kinds = (k, outcome) => audit.filter((e) => e.kind === k && (outcome === undefined || e.outcome === outcome));
    assertEqual(kinds("auth.fail").length >= 1, true, "the bad proofs are auth.fail rows");
    assertEqual(kinds("credential.mint").length, 4, "credential.mint rows");
    assertEqual(kinds("credential.revoke").length >= 1, true, "a credential.revoke row");
    assertEqual(kinds("auth.denied").filter((e) => e.code === "HUMAN_ONLY").length >= 3, true, "HUMAN_ONLY rows");
    for (const code of ["SCOPE_DENIED", "BRANCH_NOT_ALLOWED"]) {
      assertEqual(kinds("auth.denied").some((e) => e.code === code && e.principalKind === "agent"), true, `an agent ${code} row`);
    }
    assertEqual(kinds("auth.ticket").length >= 10, true, "auth.ticket rows");
    assertEqual(kinds("diagnostics.purge").length, 1, "the console's purge row");
    assertEqual(audit.some((e) => e.kind === "credential.mint" && e.details?.label !== undefined), false, "mint rows carry no label");
    log(`audit: ${audit.length} rows (${[...new Set(audit.map((e) => e.kind))].sort().join(", ")})`);

    const needles = new Set(observed);
    for (const name of ["local.key", "console.key", "credential.master"]) {
      const file = join(sd, "auth", name);
      if (!existsSync(file)) continue;
      const bytes = readFileSync(file);
      needles.add(bytes.toString("utf8").trim());
      needles.add(bytes.toString("base64url"));
    }
    needles.add(oldLocalKey.trim());
    for (const c of [reader, writer, expiring, jobber]) needles.add(c.file.key);
    const sources = [
      ...outputs,
      ...serverFiles(sd),
      ...["control.db", "control.db-wal"].filter((f) => existsSync(join(sd, f))).map((f) => ({ name: f, text: readFileSync(join(sd, f), "latin1") })),
      { name: "audit export", text: JSON.stringify(audit) },
      { name: "journal export", text: JSON.stringify(await storedEntries(server)) },
      ...[...stderrs, server.stderr()].map((text, i) => ({ name: `finisherd stderr ${i}`, text })),
    ];
    const hits = secretHits(sources, needles);
    assertEqual(hits, [], "secrets found in logs, the journal or the audit export");
    log(`leak grep: ${needles.size} needles over ${sources.length} sources, no hit`);
    const all = await consoleRpc(server, "diagnostics.purge", { selector: "all" });
    assertEqual((await storedEntries(server)).length, 0, `the journal after purge --all (${JSON.stringify(all)})`);
    log(`PASS in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    for (const s of sockets) s.close();
    squatter?.close();
    for (const s of servers) await stopServer(s);
    removeTmp(tmp);
  }
}
