// Helpers of the "history" e2e scenario.
import { waitFor } from "./lib.mjs";

/** Runs `fin` expecting a failure with `code` and exit status `status`; returns the error. */
export function failure(r, code, status, what) {
  if (r.status !== status || r.json?.error?.code !== code) {
    throw new Error(`${what}: expected exit ${status} ${code}, got exit ${r.status}: ${r.text}`);
  }
  return r.json.error;
}

/** The clips of every track (of every timeline) of a `snapshot.get` answer: `{t1: ["c1:0+5", ...]}`. */
export function clipsOf(snapshot) {
  return Object.fromEntries(
    snapshot.project.timelines.flatMap((tl) => tl.tracks).map((t) => [t.id, t.items.map((i) => `${i.id}:${i.start}+${i.duration}`)]),
  );
}

/** A subscription on `branch` from `fromRev` that collects every committed tx it receives. */
export function watch(ws, projectId, branch, fromRev, generation) {
  const seen = [];
  const sub = ws.subscribe({ projectId, branch, fromRev, generation }, (tx) => seen.push(tx));
  const w = { seen, ended: undefined, stop: () => ws.interrupt(sub.id) };
  sub.done.then((exit) => (w.ended = exit));
  w.txIds = () => seen.map((t) => t.tx.txId);
  w.branches = () => [...new Set(seen.map((t) => t.tx.branch))];
  w.waitFor = (what, pred) =>
    waitFor(
      `${what} on the ${branch} subscriber`,
      () => {
        if (w.ended) throw new Error(`${branch} subscription ended early: ${JSON.stringify(w.ended)}`);
        return pred(seen);
      },
      10_000,
      () => ` (saw ${JSON.stringify(seen.map((t) => [t.rev, t.tx.branch, t.tx.txId]))})`,
    );
  return w;
}
