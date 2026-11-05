// Analyser for one round of the cross-host storage harness (see README.md).
//
// Input: the JSONL events of every process of a round, each tagged with the node's logical host.
// Evidence that needs no synchronised clocks comes first: the verifier's resolved chain, segment
// epochs and writers, each server's own accepted-commit list and final tx statuses. Time-based
// checks compare monotonic clocks between processes of one host only; across hosts they use
// share time (wall + the offset each node measured against the share's mtimes) with a stated
// tolerance.

/** Lease timings of spec §3.5, ms. */
export const SPEC = { staleAfterMs: 45_000, minUnchangedMs: 25_000, selfFenceMs: 20_000 };

const ms = (ns) => Number(ns) / 1e6;
const median = (xs) => {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.floor((s.length - 1) / 2)];
};
export const pct = (xs, p) => {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.ceil((p / 100) * s.length) - 1)];
};

/** Per-process clocks: `mono` (same host only) and share time (`wall + offset`). */
function clocks(procs) {
  const offsets = {};
  for (const p of procs) {
    const samples = p.events.filter((e) => e.ev === "clock" && e.offsetNs !== undefined);
    const off = samples.map((e) => ms(e.offsetNs));
    const rtt = samples.map((e) => ms(e.rttNs));
    offsets[p.id] = {
      host: p.host,
      offsetMs: median(off),
      spreadMs: off.length ? Math.max(...off) - Math.min(...off) : null,
      rttMaxMs: rtt.length ? Math.max(...rtt) : null,
      samples: off.length,
    };
  }
  // A process without samples (create, verify) takes its host's median.
  const byHost = {};
  for (const o of Object.values(offsets)) if (o.offsetMs !== null) (byHost[o.host] ??= []).push(o.offsetMs);
  for (const o of Object.values(offsets)) if (o.offsetMs === null) o.offsetMs = median(byHost[o.host] ?? []) ?? 0;
  const hosts = Object.fromEntries(Object.entries(byHost).map(([h, xs]) => [h, median(xs)]));
  return { offsets, hosts };
}

/**
 * Time of event `e` of process `p` against event `f` of process `q`: `[ta, tb, tolMs]`, in ms.
 * Same host: monotonic, tolerance 0. Otherwise share time, tolerance `tolMs`.
 */
function timer(clk, tolMs) {
  return (p, e, q, f) => {
    if (p.host === q.host) return [ms(e.mono), ms(f.mono), 0];
    const a = ms(e.wall) + (clk.offsets[p.id]?.offsetMs ?? 0);
    const b = ms(f.wall) + (clk.offsets[q.id]?.offsetMs ?? 0);
    return [a, b, tolMs];
  };
}

/** Hold windows: each run of events with `lease: "held"` at one epoch. */
function windows(p) {
  const out = [];
  let cur = null;
  for (const e of p.events) {
    if (e.lease === undefined && !["end", "closed"].includes(e.ev)) continue;
    const held = e.lease === "held";
    if (cur && (!held || e.epoch !== cur.epoch || e.ev === "closed")) {
      cur.endUpper = e;
      out.push(cur);
      cur = null;
    }
    if (held && !cur && e.ev !== "closed") cur = { proc: p, epoch: e.epoch, start: e, lastHeld: e, endUpper: null };
    else if (held && cur) cur.lastHeld = e;
  }
  if (cur) {
    cur.endUpper = p.events.at(-1);
    out.push(cur);
  }
  return out;
}

export function analyseRound(procs, { tolMs = 1000 } = {}) {
  const clk = clocks(procs);
  const at = timer(clk, tolMs);
  const verify = procs.find((p) => p.role === "verify");
  const vChain = verify?.events.find((e) => e.ev === "chain");
  const vFolder = verify?.events.find((e) => e.ev === "folder");
  const vReport = verify?.events.find((e) => e.ev === "verify");
  const r = { problems: [], notes: [] };
  if (!vChain) r.problems.push("no verifier chain: the round's resolved history is unknown");

  // The resolved chain (a fresh reader's view) and the folder's records.
  const chain = new Map(); // txId -> {seq, txHash, epoch, author}
  const chainHashes = new Set();
  const seqSeen = new Map();
  let chainDupSeq = 0;
  for (const [seq, txId, txHash, epoch, , author] of vChain?.rows ?? []) {
    chain.set(txId, { seq, txHash, epoch, author });
    chainHashes.add(txHash);
    if (seqSeen.has(seq)) chainDupSeq++;
    seqSeen.set(seq, txId);
  }
  const folderRec = new Map(); // txHash -> {epochs, writers}
  const segs = vFolder?.scan?.segments ?? [];
  const epochWriters = new Map();
  for (const s of segs) {
    (epochWriters.get(s.epoch) ?? epochWriters.set(s.epoch, new Set()).get(s.epoch)).add(s.writer);
    for (const [, , h] of s.records) {
      const f = folderRec.get(h) ?? { epochs: new Set(), writers: new Set() };
      f.epochs.add(s.epoch);
      f.writers.add(s.writer);
      folderRec.set(h, f);
    }
  }

  // Servers, accepted commits, epochs.
  const nodes = procs.filter((p) => ["holder", "contender", "follower"].includes(p.role));
  const accepted = [];
  const refused = {};
  const epochServers = new Map();
  const addEpoch = (ep, sid, why) => {
    if (ep === null || ep === undefined || ep === 0) return;
    const m = epochServers.get(ep) ?? new Map();
    (m.get(sid) ?? m.set(sid, new Set()).get(sid)).add(why);
    epochServers.set(ep, m);
  };
  for (const p of nodes) {
    for (const e of p.events) {
      if (e.ev === "commit" && e.ok) {
        accepted.push({ proc: p, e });
        addEpoch(e.rowEpoch, e.server, "commit");
      } else if (e.ev === "commit") {
        const k = `${e.code}${e.reason ? `/${e.reason}` : ""}`;
        refused[k] = (refused[k] ?? 0) + 1;
      }
      if (e.ev === "lease" && e.lease === "held") addEpoch(e.epoch, e.server, "lease");
    }
  }
  for (const [ep, ws] of epochWriters) for (const w of ws) addEpoch(ep, w, "segment");
  for (const c of vFolder?.lease?.claims ?? []) addEpoch(c.epoch, c.owner, "claim");
  const epochReuse = [...epochServers].filter(([, m]) => m.size > 1).map(([ep, m]) => ({ epoch: ep, servers: Object.fromEntries([...m].map(([s, w]) => [s, [...w]])) }));

  // Double holders: hold windows of different servers that overlap.
  const wins = nodes.flatMap(windows);
  const doubles = [];
  const suspects = [];
  for (let i = 0; i < wins.length; i++)
    for (let j = i + 1; j < wins.length; j++) {
      const a = wins[i], b = wins[j];
      if (a.proc.server === b.proc.server) continue;
      // Definite: both still believed they held (last held samples) at a common instant.
      const [aS, bS, tol] = at(a.proc, a.start, b.proc, b.start);
      const [aL, bL] = at(a.proc, a.lastHeld, b.proc, b.lastHeld);
      const [aU, bU] = at(a.proc, a.endUpper, b.proc, b.endUpper);
      const def = Math.min(aL, bL) - Math.max(aS, bS);
      const up = Math.min(aU, bU) - Math.max(aS, bS);
      const item = { a: a.proc.label, ea: a.epoch, b: b.proc.label, eb: b.epoch, overlapMs: Math.round(def), upperMs: Math.round(up), tolMs: tol };
      if (def > tol) doubles.push(item);
      else if (up > -tol && up > 0) suspects.push(item);
    }

  // Accepted commits of an epoch after another server's commit of a newer epoch.
  const staleAccepts = [];
  for (const x of accepted)
    for (const y of accepted) {
      if (x.e.server === y.e.server || !(x.e.rowEpoch < y.e.rowEpoch)) continue;
      const [tx, ty, tol] = at(x.proc, x.e, y.proc, y.e);
      if (tx > ty + tol) {
        staleAccepts.push({ txId: x.e.txId, epoch: x.e.rowEpoch, after: y.e.txId, newer: y.e.rowEpoch, byMs: Math.round(tx - ty) });
        break;
      }
    }

  // Early takeovers: a gain replacing another server's record sooner than the spec allows after
  // that server's last stamp (its acquisition or last successful heartbeat).
  const stamps = []; // {proc, e, epoch}
  for (const p of procs)
    for (const e of p.events) {
      if ((e.ev === "beat" || (e.ev === "lease" && e.lease === "held") || e.ev === "created") && e.server) stamps.push({ proc: p, e, epoch: e.epoch ?? e.beatEpoch });
    }
  const takeovers = [];
  for (const p of nodes)
    for (const e of p.events) {
      if (!(e.ev === "lease" && e.lease === "held")) continue;
      const prev = e.previous?.record;
      if (!prev || prev === e.server) continue;
      const theirs = stamps.filter((s) => s.e.server === prev);
      if (!theirs.length) {
        takeovers.push({ by: p.label, epoch: e.epoch, from: prev, gapMs: null, early: null, note: "previous holder's stamps not logged" });
        continue;
      }
      let gap = Infinity, tolUsed = 0;
      for (const s of theirs) {
        const [tg, ts, tol] = at(p, e, s.proc, s.e);
        if (ts <= tg + tol && tg - ts < gap) [gap, tolUsed] = [tg - ts, tol];
      }
      // Early: before stale-after by server time (45 s); expected under a skewed holder clock.
      // Unsafe: before the 25 s the record must be seen unchanged, which outlasts the old
      // holder's self-fence (20 s) whatever the clocks say.
      const early = gap + tolUsed < SPEC.staleAfterMs;
      const unsafe = gap + tolUsed < SPEC.minUnchangedMs;
      takeovers.push({ by: p.label, epoch: e.epoch, from: prev, gapMs: Math.round(gap), tolMs: tolUsed, early, unsafe });
    }

  // Stale and lost writes.
  const lost = [];
  for (const { proc, e } of accepted) {
    const c = chain.get(e.txId);
    if (c && c.txHash === e.txHash) continue;
    const fin = proc.events.find((x) => x.ev === "final")?.txStatus?.[e.txId];
    lost.push({ txId: e.txId, by: proc.label, epoch: e.rowEpoch, seq: e.seq, exported: folderRec.has(e.txHash), status: fin ?? null });
  }
  const staleFolder = [...folderRec].filter(([h]) => !chainHashes.has(h)).map(([h, f]) => ({ txHash: h, epochs: [...f.epochs], writers: [...f.writers] }));
  const applied = [];
  const leftApplied = [];
  let rewinds = 0, rewoundRecords = 0;
  for (const p of nodes) {
    for (const e of p.events) {
      if (e.ev === "applied" && e.txHash && !chainHashes.has(e.txHash)) applied.push({ by: p.label, txId: e.txId, seq: e.seq, recEpoch: e.recEpoch });
      if (e.ev === "rewound") {
        rewinds++;
        rewoundRecords += e.quarantined ?? 0;
      }
    }
    const fc = p.events.find((x) => x.ev === "chain");
    if (vChain && fc) {
      for (const [seq, txId, txHash, epoch, exported] of fc.rows) {
        if (!chainHashes.has(txHash)) leftApplied.push({ node: p.label, seq, txId, epoch, exported });
      }
    }
  }

  // Id reuse.
  const txIdServers = new Map();
  for (const { e } of accepted) (txIdServers.get(e.txId) ?? txIdServers.set(e.txId, new Set()).get(e.txId)).add(e.server);
  const txIdReuse = [...txIdServers].filter(([, s]) => s.size > 1).map(([t]) => t);
  const ids = new Map();
  for (const { e } of accepted) {
    if (!(chain.get(e.txId)?.txHash === e.txHash)) continue;
    for (const id of e.ids ?? []) (ids.get(id) ?? ids.set(id, []).get(id)).push(e.txId);
  }
  const entityReuse = [...ids].filter(([, t]) => t.length > 1).map(([id, t]) => ({ id, txIds: t }));
  const seqAcc = new Map();
  for (const { e } of accepted) (seqAcc.get(e.seq) ?? seqAcc.set(e.seq, new Set()).get(e.seq)).add(e.server);
  const seqCollisions = [...seqAcc].filter(([, s]) => s.size > 1).length;

  // Follow latency: commit on one server -> applied on another (commits after it opened).
  const commitsById = new Map(accepted.map((x) => [x.e.txId, x]));
  const lat = [];
  for (const p of nodes) {
    const opened = p.events.find((e) => e.ev === "opened");
    for (const e of p.events) {
      if (e.ev !== "applied") continue;
      const c = commitsById.get(e.txId);
      if (!c || c.e.server === e.server) continue;
      const [tc, to] = at(c.proc, c.e, p, opened);
      if (tc < to) continue; // committed before this node opened: catch-up, not following
      const [tApplied, tCommit] = at(p, e, c.proc, c.e);
      lat.push(tApplied - tCommit);
    }
  }

  const finals = Object.fromEntries(
    nodes.map((p) => {
      const f = p.events.find((e) => e.ev === "final");
      const end = p.events.find((e) => e.ev === "end");
      return [p.label, { role: p.role, accepted: f?.accepted ?? 0, attempts: f?.attempts ?? 0, readOnly: f?.readOnly ?? null, lease: f?.lease ?? null, epoch: f?.epoch ?? null, quarantine: f?.quarantine ?? null, ok: end?.ok ?? false, error: end?.error }];
    }),
  );
  for (const p of procs) {
    const end = p.events.find((e) => e.ev === "end");
    if (!end?.ok) r.problems.push(`${p.label} (${p.role}) did not end ok: ${end?.error ?? `exit ${p.exit}`}`);
    for (const w of p.events.filter((e) => e.ev === "warning")) r.notes.push(`${p.label}: ${w.what}`);
  }
  const vFailed = (vReport?.checks ?? []).filter((c) => !c.ok).map((c) => c.name);

  const lostSetAside = lost.filter((l) => l.status?.state === "setAside");
  const lostVanished = lost.filter((l) => l.status?.state !== "setAside");
  return {
    ...r,
    counts: {
      doubleHolders: doubles.length + epochReuse.length,
      doubleHolderSuspects: suspects.length,
      earlyTakeovers: takeovers.filter((t) => t.early).length,
      unsafeTakeovers: takeovers.filter((t) => t.unsafe).length,
      takeovers: takeovers.length,
      staleAcceptedAfterNewerEpoch: staleAccepts.length,
      staleExported: staleFolder.length,
      staleApplied: applied.length,
      staleLeftApplied: leftApplied.length,
      rewinds,
      rewoundRecords,
      idReuse: txIdReuse.length + entityReuse.length + chainDupSeq,
      seqCollisions,
      accepted: accepted.length,
      lostAcked: lost.length,
      lostAckedSetAside: lostSetAside.length,
      lostAckedVanished: lostVanished.length,
      verifyFailed: vFailed.length,
      chain: chain.size,
      holders: new Set(wins.map((w) => w.proc.server)).size,
    },
    critical: lostVanished.length + doubles.length + epochReuse.length + leftApplied.length + takeovers.filter((t) => t.unsafe).length > 0,
    latencyMs: lat,
    detail: { doubles, suspects, epochReuse, takeovers, staleAccepts, lost, staleFolder, applied, leftApplied, txIdReuse, entityReuse, refused, finals, verifyFailed: vFailed, verifyOk: vReport?.ok ?? null, clocks: clk },
  };
}

/** Totals over rounds. */
export function summarise(rounds) {
  const counts = {};
  const lat = [];
  const refused = {};
  let critical = 0;
  for (const r of rounds) {
    for (const [k, v] of Object.entries(r.counts)) counts[k] = (counts[k] ?? 0) + v;
    lat.push(...r.latencyMs);
    for (const [k, v] of Object.entries(r.detail.refused)) refused[k] = (refused[k] ?? 0) + v;
    if (r.critical) critical++;
  }
  const round = (x) => (x === null ? null : Math.round(x));
  return {
    rounds: rounds.length,
    criticalRounds: critical,
    counts,
    refused,
    latencyMs: { n: lat.length, p50: round(pct(lat, 50)), p95: round(pct(lat, 95)), max: round(lat.length ? Math.max(...lat) : null), min: round(lat.length ? Math.min(...lat) : null) },
    problems: rounds.flatMap((r, i) => r.problems.map((p) => `round ${i + 1}: ${p}`)),
  };
}
