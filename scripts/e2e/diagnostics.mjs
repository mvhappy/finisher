// The "diagnostics" scenario (plan D7): the journal end to end, driven by `fin` with
// `FIN_SESSION`/`FIN_CALL` set (the WebSocket only for what `fin` cannot send: a stale edit, a
// retried txId and a conflicting edit, and `tx.get` for the join check).
//   1. diagnostics and training off by env: no `events/`, `show` says off;
//   1a. no env and no `privacy` settings: diagnostics on (`basis: default`), training
//       `undecided`, and an edit writes no content;
//   2. diagnostics on: edits, an undo, a retry key, a branch merge, `help`/`schema`/usage runs,
//      then `rebased`, `deduped` and `conflict` over the WebSocket; the outcomes, the undo's
//      links, `cli.command` txIds and actors, no content, every `cli.command` counted, and every
//      committed `log.commit` txId joined to the op log through `tx.get`;
//   3. the leak grep over `events/` (blobs included);
//   4. `project never` purges and drops, also on a second server following the folder;
//   5. `consent training on` is refused, `FIN_TRAINING=on` records an intent, a hand edit of
//      `settings.json` opts in and `consent training off` withdraws (both `consent.changed`),
//      the training upload view is then empty, and `purge --all` leaves no key and no blobs.
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { hostname, tmpdir, userInfo } from "node:os";
import { join } from "node:path";
import { writeFixture } from "./fixture.mjs";
import { assertEqual, connectWs, fin, finRaw, live, log, removeTmp, startServer, stopServer, waitFor } from "./lib.mjs";
import { contentProblems, exportEntries, filesUnder, keyer, leaks, loadContentFields } from "./diagnostics-lib.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);
/** The agent session every run claims (a ULID: stored raw). */
const SESSION = "01JC0000000000000000000D07";
/** Consent switches this process may have inherited: the scenario sets its own. */
const SWITCHES = ["FIN_DIAGNOSTICS", "FIN_TRAINING", "DO_NOT_TRACK"];
const COMMITTED = new Set(["accepted", "rebased", "deduped"]);

function hostName() {
  const out = spawnSync("uname", ["-n"], { encoding: "utf8" });
  return out.status === 0 && out.stdout.trim() ? out.stdout.trim() : hostname();
}

export async function run() {
  const started = Date.now();
  const saved = Object.fromEntries(SWITCHES.map((k) => [k, process.env[k]]));
  for (const k of SWITCHES) delete process.env[k];
  const tmp = realpathSync(mkdtempSync(join(tmpdir(), "finisher-e2e-diag-")));
  live.tmp = tmp;
  const sd = join(tmp, "server");
  const sdB = join(tmp, "server-b");
  const project = join(tmp, "diag project");
  const events = join(sd, "events");
  const marker = `argmark${Date.now().toString(36)}`;
  const fields = loadContentFields();
  const servers = [];
  const sockets = [];
  let key;
  try {
    let server = await startServer(sd, { FIN_DIAGNOSTICS: "off", FIN_TRAINING: "off" });
    servers.push(server);
    // Every run claims the agent session, each with a call id of its own (keyed: not a ULID).
    let calls = 0;
    let runs = 0;
    const lastCall = () => `call-${calls}`;
    const agent = (env = {}) => ({ FIN_SESSION: SESSION, FIN_CALL: `call-${++calls}`, ...env });
    const f = (args, env) => (runs++, fin(sd, project, args, agent(env)));
    const raw = (args, env) => (runs++, finRaw(sd, project, args, agent(env)));
    const restart = async (env) => {
      await stopServer(server);
      const at = Date.now(); // after the old server's last entry, before the new one's first
      server = await startServer(sd, env);
      servers.push(server);
      return at;
    };
    const show = () => f(["diagnostics", "show"]);
    const exported = (args = []) => (runs++, exportEntries(sd, project, args, agent()));

    step(1, "diagnostics and training off by env: three edits and an undo leave no events/");
    const created = fin(sd, tmp, ["project", "create", project, "--name", "Diag"]);
    const projectId = created.project.projectId;
    await writeFixture(join(project, "clip.mp4"));
    f(["asset", "add", "clip.mp4"]);
    const insert = (at, dur = "1s") => f(["clip", "insert", "--no-audio", "--track", "t1", "--at", at, "--asset", "a1", "--dur", dur]);
    insert("0", "2s");
    insert("end");
    f(["marker", "add", "--at", "1s", "--label", `${marker}-off`]);
    f(["undo"]);
    const off = show();
    assertEqual([off.state, off.consent.diagnostics, off.consent.training], ["off", "off", "off"], "show while off");
    assertEqual(existsSync(events), false, "no events/ while diagnostics are off");

    step("1a", "no env, no privacy settings: diagnostics on by default, training undecided, no content");
    await restart({});
    const byDefault = show();
    assertEqual(
      [byDefault.state, byDefault.consent.diagnostics, byDefault.consent.basis, byDefault.consent.training],
      ["on", "on", "default", "undecided"],
      "show by default",
    );
    const plain = f(["marker", "add", "--at", "2s", "--label", `${marker}-default`]);
    const first = await waitFor(
      "the default edit's log.commit",
      () => exported(["--kind", "log.commit"]).find((e) => e.ctx?.txId === plain.txId),
      5_000,
    );
    assertEqual([first.payload.outcome, first.consent.basis, first.consent.purposes], ["accepted", "default", ["diagnostics"]], "default log.commit");
    const defaultProblems = exported().flatMap((e) => contentProblems(e, fields));
    assertEqual(defaultProblems, [], "content recorded by default");

    step(2, "diagnostics on: edits, undo, retry key, merge, help/schema/usage, then rebased/deduped/conflict");
    const since = await restart({ FIN_DIAGNOSTICS: "on" });
    key = await keyer(sd, join(tmp, "key-copy"));
    const idRef = (raw) => key.idRef(raw);
    runs = 0;
    // Each fin write: its answer, the call id it ran with, and the stored txIds it names.
    const writes = [];
    const write = (args, env) => {
      const doc = f(args, env);
      const ids = doc.merge ? [`${doc.merge.mergeId}-1`] : [doc.txId];
      writes.push({ args, doc, call: lastCall(), raw: ids, ids: ids.map(idRef) });
      return doc;
    };
    const e1 = write(["clip", "insert", "--no-audio", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "1s"]);
    write(["clip", "insert", "--no-audio", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "1s"]);
    write(["marker", "add", "--at", "3s", "--label", `${marker}-on`]);
    const undo = write(["undo"]);
    const retryKey = `${marker}-idem`;
    write(["--idem-key", retryKey, "marker", "add", "--at", "4s", "--label", `${marker}-idem`]);
    write(["branch", "new", "side"]);
    write(["--branch", "side", "marker", "add", "--at", "5s", "--label", `${marker}-side`]);
    const merge = write(["merge", "side"]);
    const doomed = write(["clip", "delete", "c1"]);
    for (const args of [["--help"], ["schema"], ["clip", "insert"]]) raw(args);

    const ws = await connectWs(server);
    sockets.push(ws);
    const { rev: head, generation } = await ws.call("snapshot.get", { projectId, branch: "main" });
    const tx = (txId, baseRev, ops) => ({ txId, author: "e2e", branch: "main", baseRev, schemaVersion: 2, ops });
    const stale = tx(`${marker}-stale`, head - 1, [
      { op: "clip.insert", id: "$c", track: "t1", at: "30", asset: "a1", sourceIn: "0", duration: "1" },
    ]);
    const rebased = await ws.call("tx.commit", { projectId, tx: stale, generation });
    const retried = (await ws.call("tx.get", { projectId, txId: e1.txId })).tx;
    await ws.call("tx.commit", { projectId, tx: retried, generation });
    const clash = tx(`${marker}-clash`, doomed.rev - 1, [{ op: "clip.delete", clip: "c1" }]);
    const refused = await ws.call("tx.commit", { projectId, tx: clash, generation }).then(
      () => undefined,
      (e) => String(e.message),
    );
    if (!refused?.includes("CONFLICT")) throw new Error(`the conflicting edit was not refused CONFLICT: ${refused}`);

    // Every fin run since the restart sends one `cli.command` as it exits (help, schema and
    // usage errors too, `online: false`); each poll is a run of its own, counted before it.
    let all = [];
    await waitFor(
      "one cli.command per fin run",
      () => {
        const want = runs;
        all = exported().filter((e) => e.ts >= since);
        return all.filter((e) => e.kind === "cli.command").length === want;
      },
      5_000,
      () => ` (${all.filter((e) => e.kind === "cli.command").length} cli.command entries, ${runs} runs)`,
      200,
    );
    const commands = all.filter((e) => e.kind === "cli.command");
    const commits = all.filter((e) => e.kind === "log.commit");
    if (!all.some((e) => e.kind === "session.start")) throw new Error("no session.start after the restart");
    const offline = commands.filter((e) => !e.payload.online).map((e) => e.payload.name);
    if (offline.length < 3) throw new Error(`help, schema and the usage error are not offline cli.command entries: ${JSON.stringify(offline)}`);

    const commitOf = (id) => commits.filter((e) => e.ctx?.txId === id);
    const one = (list, what) => {
      if (list.length !== 1) throw new Error(`${what}: ${list.length} entries`);
      return list[0];
    };
    for (const w of writes) {
      // The WebSocket retry of the first edit is a second entry with its txId: not fin's.
      const c = one(
        commitOf(w.ids[0]).filter((e) => e.ctx.actor?.kind === "agent"),
        `log.commit of fin ${w.args.join(" ")}`,
      );
      if (c.payload.outcome !== "accepted") throw new Error(`fin ${w.args.join(" ")}: outcome ${c.payload.outcome}`);
      const callRef = idRef(w.call);
      const cmd = one(
        commands.filter((e) => e.ctx?.actor?.call === callRef),
        `cli.command of fin ${w.args.join(" ")}`,
      );
      assertEqual(cmd.payload.txIds, w.ids, `cli.command txIds of fin ${w.args.join(" ")}`);
      assertEqual(c.ctx.actor, { kind: "agent", session: SESSION, call: callRef }, `log.commit actor of fin ${w.args.join(" ")}`);
      assertEqual(cmd.ctx.actor, c.ctx.actor, `cli.command and log.commit actors of fin ${w.args.join(" ")}`);
    }
    assertEqual(one(commitOf(undo.txId), "undo").payload.links, undo.undone.map(idRef), "the undo's links name the undone tx");
    assertEqual(one(commitOf(idRef(`${merge.merge.mergeId}-1`)), "merge").payload.record, "merge", "merge record");
    const r = one(commitOf(idRef(stale.txId)), "stale edit").payload;
    assertEqual([r.outcome, r.baseRev, r.rebasedOver, rebased.rev], ["rebased", head - 1, 1, head + 1], "stale edit rebased");
    const d = commitOf(e1.txId).map((e) => e.payload.outcome);
    assertEqual(d, ["accepted", "deduped"], "first edit, then its retry");
    const c = one(commitOf(idRef(clash.txId)), "conflicting edit").payload;
    assertEqual([c.outcome, c.code], ["conflict", "CONFLICT"], "conflicting edit");
    assertEqual(all.flatMap((e) => contentProblems(e, fields)), [], "content recorded without training");

    // The join: every committed log.commit names a txId the op log holds.
    const known = new Map(
      [...writes.flatMap((w) => w.raw), stale.txId, ...undo.undone].map((raw) => [idRef(raw), raw]),
    );
    const joined = commits.filter((e) => COMMITTED.has(e.payload.outcome));
    for (const e of joined) {
      const raw = known.get(e.ctx.txId);
      if (raw === undefined) throw new Error(`log.commit ${e.id} names txId ${e.ctx.txId}, which no run committed`);
      const got = await ws.call("tx.get", { projectId, txId: raw });
      if (got?.tx?.txId !== raw) throw new Error(`tx.get ${raw} (log.commit ${e.id}): ${JSON.stringify(got)}`);
    }
    for (const e of commits) {
      for (const link of e.payload.links ?? []) {
        if (!known.has(link)) throw new Error(`log.commit ${e.id} links ${link}, which no run committed`);
      }
    }
    log(`${commands.length} cli.command (${offline.length} offline), ${commits.length} log.commit, ${joined.length} joined by tx.get`);

    step(3, "leak grep over events/ (blobs included)");
    const user = process.env.USER ?? userInfo().username;
    const needles = [tmp, process.env.HOME, user && user.length >= 3 ? user : undefined, hostName(), marker];
    const found = leaks(filesUnder(events), needles);
    assertEqual(found, [], "leaks in events/");

    step(4, "project never: earlier entries purged, new ones dropped, also on a following server");
    assertEqual(f(["diagnostics", "project", "never"]), { project: projectId, collect: "never" }, "project never");
    f(["marker", "add", "--at", "6s", "--label", `${marker}-never`]);
    assertEqual(exported(["--project", projectId]), [], "entries of a never project");
    const follower = await startServer(sdB, {});
    servers.push(follower);
    fin(sdB, project, ["project", "open", project]);
    assertEqual(fin(sdB, project, ["project", "status"]).status.readOnly?.reason, "LEASE_HELD", "the second server follows");
    assertEqual(fin(sdB, project, ["diagnostics", "show"]).projects[projectId], "never", "the follower reads never");
    assertEqual(exportEntries(sdB, project, ["--project", projectId]), [], "the follower's entries of a never project");
    if (!exportEntries(sdB, project).some((e) => e.kind === "session.start")) throw new Error("the follower's journal is not recording");
    await stopServer(follower);
    assertEqual(f(["diagnostics", "project", "default"]).collect, "default", "project default again");

    step(5, "training: fin cannot opt in; FIN_TRAINING=on records an intent; withdrawal and purge --all");
    const refusedOn = raw(["diagnostics", "consent", "training", "on"]);
    assertEqual([refusedOn.status, refusedOn.json?.error?.code], [2, "USAGE"], "consent training on");
    if (!String(refusedOn.json.error.fix).includes("FIN_TRAINING=on")) throw new Error(`the fix names no way in: ${refusedOn.text}`);
    await restart({ FIN_TRAINING: "on" });
    assertEqual(show().consent.training, "on", "training on by env");
    const taught = f(["marker", "add", "--at", "7s", "--label", `${marker}-train`]);
    const trained = await waitFor(
      "the training edit's log.commit",
      () => exported(["--kind", "log.commit"]).find((e) => e.ctx?.txId === taught.txId),
      5_000,
    );
    if (!Array.isArray(trained.payload.intent) || !trained.consent.purposes.includes("training")) {
      throw new Error(`the training edit has no intent: ${JSON.stringify(trained)}`);
    }
    // Without the env training is `undecided` again. A restart journals no `consent.changed`
    // (the stamps and `rev` show it), so the person's other way in, a hand edit of
    // `settings.json` while the server runs, opts in where the journal sees it.
    const changedSince = await restart({});
    assertEqual(show().consent.training, "undecided", "training without the env");
    const settings = join(sd, "settings.json");
    const current = existsSync(settings) ? JSON.parse(readFileSync(settings, "utf8")) : {};
    writeFileSync(settings, `${JSON.stringify({ ...current, privacy: { ...current.privacy, training: "on" } }, null, 2)}\n`);
    await waitFor("the hand edit to opt in", () => show().consent.training === "on", 5_000, () => "", 250);
    assertEqual(f(["diagnostics", "consent", "training", "off"]).training, "off", "consent training off");
    const changes = exported(["--kind", "consent.changed"])
      .filter((e) => e.ts >= changedSince)
      .map((e) => [e.payload.from, e.payload.to]);
    assertEqual(
      changes,
      [
        [["diagnostics"], ["diagnostics", "training"]],
        [["diagnostics", "training"], ["diagnostics"]],
      ],
      "consent.changed for the opt-in and the withdrawal",
    );
    assertEqual(exported(["--as-upload", "training"]), [], "training upload view after the withdrawal");
    // `fin` records no `cli.command` for a purge, so nothing writes a new key after it.
    const purged = f(["diagnostics", "purge", "--all"]);
    if (!(purged.removed >= 1)) throw new Error(`purge --all removed nothing: ${JSON.stringify(purged)}`);
    assertEqual([existsSync(join(events, "key")), existsSync(join(events, "blobs"))], [false, false], "key and blobs/ after purge --all");
    log(`PASS diagnostics in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    for (const ws of sockets) ws.close();
    await key?.close();
    for (const s of servers.reverse()) await stopServer(s);
    for (const [k, v] of Object.entries(saved)) if (v !== undefined) process.env[k] = v;
    removeTmp(tmp);
  }
}
