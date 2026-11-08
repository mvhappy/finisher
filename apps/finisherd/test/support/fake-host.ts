// A fake `JobHost` (9a review): no process, events scripted by the test, delivered exactly as
// fin-node's host promises (`spawned` first, after `spawn` resolved and its `await` ran).
import type { HostMsg, JobGroupSpec, JobHostCaps, JobHostEvent, JobHostExitInfo, JobHostKillReason, JobSpawnRequest, JobWatch } from "@finisher/core-types";
import type { JobHostFactory, JobHostLike } from "../../src/jobs/host.ts";

export type Script = (host: FakeHost, proc: number, req: JobSpawnRequest) => void;

export class FakeHost implements JobHostLike {
  onEvent: (e: JobHostEvent) => void = () => {};
  readonly spawns: JobSpawnRequest[] = [];
  readonly sent: Array<{ proc: number; msg: HostMsg }> = [];
  readonly #procs = new Map<number, { group: number; alive: boolean }>();
  #nextProc = 1;
  #nextGroup = 1;
  script: Script;

  constructor(script: Script) {
    this.script = script;
  }

  /** The engine option that starts this host (marked `fake`: its processes have no files to check). */
  get factory(): JobHostFactory & { readonly fake: true } {
    return Object.assign(
      async (_opts: Parameters<JobHostFactory>[0], onEvent: Parameters<JobHostFactory>[1]) => {
        this.onEvent = onEvent;
        return this;
      },
      { fake: true as const },
    );
  }

  /** What `caps()` reports (tests may turn the sandbox on). */
  capsOverride: Partial<JobHostCaps> = {};

  caps(): JobHostCaps {
    return { killTree: "processGroup", watchdog: false, cgroupMemory: false, leafLimits: false, sandbox: "none", sandboxed: false, profiles: { worker: "disabled", plugin: "disabled" }, perJobGrants: false, ...this.capsOverride };
  }

  openGroup(_spec: JobGroupSpec): number {
    return this.#nextGroup++;
  }

  async spawn(group: number, req: JobSpawnRequest): Promise<number> {
    const proc = this.#nextProc++;
    this.#procs.set(proc, { group, alive: true });
    this.spawns.push(req);
    // After the caller's `await` has run, as fin-node's host does.
    setImmediate(() => {
      this.emit({ type: "spawned", proc, group });
      this.script(this, proc, req);
    });
    return proc;
  }

  send(proc: number, msg: HostMsg): "ok" | "full" | "gone" {
    this.sent.push({ proc, msg });
    const alive = this.#procs.get(proc)?.alive === true;
    if (alive && this.onSend !== undefined) {
      const on = this.onSend;
      setImmediate(() => on(this, proc, msg));
    }
    return alive ? "ok" : "gone";
  }

  /** Called (after the send returned) with each message to a live process: a scripted worker's reactions. */
  onSend: ((host: FakeHost, proc: number, msg: HostMsg) => void) | undefined;

  /** Whether the process is alive. */
  alive(proc: number): boolean {
    return this.#procs.get(proc)?.alive === true;
  }

  watch(_proc: number, _w: JobWatch | null): void {}

  closeStdin(_proc: number): void {}

  async killGroup(group: number, reason: JobHostKillReason): Promise<void> {
    for (const [proc, p] of this.#procs) if (p.group === group && p.alive) this.exit(proc, { reason: "killed", kill: reason });
  }

  closeGroup(_group: number): void {}

  async shutdown(_graceMs: number): Promise<void> {
    for (const [proc, p] of this.#procs) if (p.alive) this.exit(proc, { reason: "killed", kill: "shutdown" });
  }

  emit(e: JobHostEvent): void {
    this.onEvent(e);
  }

  /** The process exits; `groupEmpty` follows its group's last exit. */
  exit(proc: number, reason: Pick<JobHostExitInfo, "reason"> & Partial<JobHostExitInfo>): void {
    const p = this.#procs.get(proc);
    if (p === undefined || !p.alive) return;
    p.alive = false;
    this.emit({ type: "exit", proc, info: { stderrTail: "", lifetimeMs: 1, ...reason } as JobHostExitInfo });
    if (![...this.#procs.values()].some((q) => q.group === p.group && q.alive)) this.emit({ type: "groupEmpty", group: p.group });
  }
}

/** fin-node's real host, recording every spawn request in `reqs` and every message sent in `sent`. */
export function recordingHost(reqs: JobSpawnRequest[], sent: Array<{ proc: number; msg: HostMsg }> = []): JobHostFactory {
  return async (opts, onEvent) => {
    const { JobHost } = await import("@finisher/fin-node");
    const h = await JobHost.start(opts, onEvent);
    return {
      caps: () => h.caps(),
      openGroup: (s) => h.openGroup(s),
      spawn: (g, r) => {
        reqs.push(r);
        return h.spawn(g, r);
      },
      send: (p, m) => {
        sent.push({ proc: p, msg: m });
        return h.send(p, m);
      },
      watch: (p, w) => h.watch(p, w),
      closeStdin: (p) => h.closeStdin(p),
      killGroup: (g, r) => h.killGroup(g, r),
      closeGroup: (g) => h.closeGroup(g),
      shutdown: (ms) => h.shutdown(ms),
      killProcForTest: h.killProcForTest,
      abandonForTest: h.abandonForTest,
    };
  };
}
