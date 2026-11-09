// A cold job's helpers (spec §6.4, 5C-1 plan Decision 39). Each `HelperSpec` of the kind is
// spawned in the job's group from its registered `HelperProgram`, with its own directory
// (`helpers/<id>/tmp/`), awaited until its `hello`, then attached with only its own files.
// The worker starts after every helper is attached. A helper that exits or sends `error`
// before the worker's answer fails the job with `HELPER_FAILED {helper, code}`; after the
// answer the helpers get `shutdown` and 2 s before their group is killed.
import { join } from "node:path";
import type { FileGrant, HelperGrant, HostMsg, WorkerMsg } from "@finisher/core-types";
import { type FinError, type HelperSpec, isJobRights, type MappedFile } from "@finisher/contracts";
import { finError, toFinError } from "../errors.ts";
import { childEnv } from "../env.ts";
import { CallRouter } from "./calls.ts";
import type { WorkerExitEvent } from "./events.ts";
import { type JobDirs, mappedPath } from "./files.ts";
import { mkdirSafe } from "./fsafe.ts";
import { type JobHostExitInfo, type JobSpawnRequest, type ProcEvent, exitVerdict } from "./host.ts";
import { exitPluginId, type HelperProgram } from "./kinds.ts";
import type { RunnerEnv } from "./runner.ts";
import type { JobRow } from "./store.ts";

/** How long helpers get after `shutdown` before their group is killed. */
export const HELPER_SHUTDOWN_MS = 2000;
const ID = /^[A-Za-z0-9._-]{1,64}$/;

/** A helper's own directory and temp directory. */
export function helperDirs(job: JobDirs, id: string): { dir: string; tmp: string } {
  const dir = join(job.job, "helpers", id);
  return { dir, tmp: join(dir, "tmp") };
}

/** The names a reused job directory checks for links (`helpers/<id>/tmp/`). */
export function helperNames(specs: ReadonlyArray<HelperSpec>): string[] {
  return specs.map((h) => `helpers/${h.id}/tmp/`);
}

interface Proc {
  readonly spec: HelperSpec;
  readonly program: HelperProgram;
  readonly dirs: { dir: string; tmp: string };
  calls: CallRouter;
  proc?: number;
  attached: boolean;
  exited: boolean;
  onExit?: () => void;
}

const helperFailed = (spec: HelperSpec, code: string, message: string): FinError =>
  finError("HELPER_FAILED", `the ${spec.kind} helper ${spec.id} failed: ${message}`, { helper: spec.id, code });

export class Helpers {
  readonly #procs: Proc[] = [];
  readonly #env: RunnerEnv;
  readonly #row: JobRow;
  readonly #dirs: JobDirs;
  readonly #files: ReadonlyArray<MappedFile>;
  readonly #group: () => number;
  readonly #fail: (e: FinError) => void;
  readonly #qos: "normal" | "utility" | "background";
  #answered = false;
  #started = 0;

  /** `fail` is called once a helper fails before the worker's answer (the attempt kills its groups). */
  constructor(
    env: RunnerEnv,
    row: JobRow,
    dirs: JobDirs,
    files: ReadonlyArray<MappedFile>,
    opts: { readonly group: () => number; readonly qos: "normal" | "utility" | "background"; readonly fail: (e: FinError) => void },
  ) {
    this.#env = env;
    this.#row = row;
    this.#dirs = dirs;
    this.#files = files;
    this.#group = opts.group;
    this.#fail = opts.fail;
    this.#qos = opts.qos;
  }

  /** Helper processes started. */
  get started(): number {
    return this.#started;
  }

  /** Helpers still running. */
  get live(): number {
    return this.#procs.filter((p) => p.proc !== undefined && !p.exited).length;
  }

  /** The worker answered: helper exits no longer fail the job; new host calls are refused. */
  answered(): void {
    this.#answered = true;
    for (const p of this.#procs) p.calls.refuse("after the job's answer");
  }

  /**
   * Spawns and attaches every helper in turn; resolves the worker's `start.helpers`. Rejects
   * with `HELPER_FAILED` (or the spawn's error) when one does not come up.
   */
  async start(specs: ReadonlyArray<HelperSpec>): Promise<HelperGrant[]> {
    const grants: HelperGrant[] = [];
    for (const spec of specs) {
      const program = this.#env.registry.helper(spec.kind, spec.id);
      if (program === undefined || !ID.test(spec.id)) throw helperFailed(spec, "WORKER_UNAVAILABLE", "no such helper is registered");
      const dirs = helperDirs(this.#dirs, spec.id);
      await mkdirSafe(this.#dirs.job, `helpers/${spec.id}/tmp`);
      const p: Proc = { spec, program, dirs, calls: undefined as unknown as CallRouter, attached: false, exited: false };
      p.calls = this.#router(p);
      this.#procs.push(p);
      await this.#spawn(p);
      // Only the kind's own mapped files are advertised to the worker (9b review L4).
      grants.push({ id: spec.id, kind: spec.kind, files: spec.files.filter((name) => this.#files.some((f) => f.name === name)) });
    }
    return grants;
  }

  #router(p: Proc): CallRouter {
    const env = this.#env;
    const row = this.#row;
    return new CallRouter(
      p.program.onCall,
      () => ({
        job: env.store.get(row.id) ?? row,
        role: "helper",
        helper: { kind: p.spec.kind, id: p.spec.id },
        owner: row.owner,
        ...(row.via === undefined ? {} : { via: row.via }),
        ...(isJobRights(row.rights) ? { rights: row.rights } : {}),
      }),
      (msg) => (p.proc === undefined ? "gone" : env.host.send(p.proc, msg)),
      env.run,
    );
  }

  #request(p: Proc): JobSpawnRequest {
    const program = p.program.resolve(this.#row);
    const tmp = p.dirs.tmp;
    const mine = this.#files.filter((f) => p.spec.files.includes(f.name));
    return {
      role: "helper",
      helper: { kind: p.spec.kind, id: p.spec.id },
      profile: p.spec.profile,
      family: this.#row.family,
      program: "path" in program ? { path: program.path } : { bundled: program.binary },
      args: [...program.args],
      env: { ...childEnv(), TMPDIR: tmp, TEMP: tmp, TMP: tmp },
      cwd: p.dirs.dir,
      // Its own temp directory and its own files at their modes (9a review H1); not the job
      // directory, which holds the other helpers' directories (9b review L4).
      grants: [
        { path: tmp, mode: "readWrite" },
        ...mine.map((f) => ({ path: mappedPath(this.#dirs.job, f.name), mode: f.mode })),
      ],
      qos: this.#qos,
      label: `${p.spec.kind}:${p.spec.id} ${this.#row.id}`,
    };
  }

  #spawn(p: Proc): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      let settled = false;
      const settle = (e?: FinError): void => {
        if (settled) return;
        settled = true;
        if (e === undefined) resolve();
        else reject(e);
      };
      const listener = (e: ProcEvent, proc: number): void => {
        p.proc ??= proc;
        if (e.type === "hello") {
          const h = e.hello;
          // A helper's hello names no family and no worker kinds (9b review L4).
          if (h.role !== "helper" || h.id !== p.spec.id || h.kind !== p.spec.kind || (h.family ?? "") !== "" || (h.kinds ?? []).length > 0) {
            const what = h.family === undefined || h.family === "" ? "" : ` of family ${h.family}`;
            const err = helperFailed(p.spec, "WORKER_PROTOCOL_MISMATCH", `its hello names ${h.role} ${h.kind ?? "?"}:${h.id ?? "?"}${what}`);
            this.#failed(err);
            settle(err);
            return;
          }
          this.#attach(p);
          settle();
        } else if (e.type === "message") this.#onMessage(p, e.msg);
        else if (e.type === "stderr") this.#env.log("debug", `[job ${this.#row.id} helper ${p.spec.id}] ${e.line}`);
        else {
          void this.#onExit(p, e.info).then(() => {
            if (!settled) settle(helperFailed(p.spec, exitVerdict(e.info, false).error.code, `it exited before it was attached (${e.info.reason})`));
          });
        }
      };
      this.#env.host.spawn(this.#group(), this.#request(p), listener, this.#row.id).then(
        (proc) => {
          p.proc ??= proc;
          this.#started++;
        },
        (e) => settle(toFinError(e)),
      );
    });
  }

  #attach(p: Proc): void {
    if (p.proc === undefined) return;
    const files: FileGrant[] = this.#files
      .filter((f) => p.spec.files.includes(f.name))
      .map((f) => ({ name: f.name, path: mappedPath(this.#dirs.job, f.name), mode: f.mode, ...(f.bytes === undefined ? {} : { bytes: f.bytes }) }));
    const attach: HostMsg = { t: "attach", job: this.#row.id, files, dirs: { job: this.#dirs.job, tmp: p.dirs.tmp } };
    this.#env.host.send(p.proc, attach);
    p.attached = true;
  }

  #onMessage(p: Proc, msg: WorkerMsg): void {
    if (msg.t === "call") {
      p.calls.call(msg.id, msg.method, msg.params);
      return;
    }
    if (msg.t === "error") {
      this.#failed(helperFailed(p.spec, msg.error.code, msg.error.message));
      return;
    }
    this.#env.log("debug", `job ${this.#row.id}: helper ${p.spec.id} sent ${msg.t}, which helpers do not send`);
  }

  #failed(e: FinError): void {
    if (this.#answered) return;
    this.#fail(e);
  }

  async #onExit(p: Proc, info: JobHostExitInfo): Promise<void> {
    p.exited = true;
    if (info.reason === "abandoned" && p.proc !== undefined) this.#env.abandoned(p.proc, this.#row.id);
    const verdict = exitVerdict(info, this.#answered);
    // The helper's plugin, else the job's (sp8b Decision 3b).
    const plugin = exitPluginId(p.program, this.#row.via);
    try {
      const exit: WorkerExitEvent = {
        role: "helper",
        family: this.#row.family,
        helperKind: p.spec.kind,
        reason: verdict.reason,
        jobsServed: p.attached ? 1 : 0,
        lifetimeS: info.lifetimeMs / 1000,
        sandboxed: this.#env.host.caps().sandboxed,
        ...(info.code === undefined ? {} : { exitCode: info.code }),
        ...(info.signal === undefined ? {} : { signal: info.signal }),
        ...(plugin === undefined ? {} : { pluginId: plugin }),
      };
      this.#env.sink.workerExit(exit);
    } catch (e) {
      this.#env.log("error", `job ${this.#row.id}: worker.exit of a helper failed: ${toFinError(e).message}`);
    }
    if (p.attached) this.#failed(helperFailed(p.spec, verdict.error.code, `it exited (${info.reason})`));
    await p.calls.close();
    p.onExit?.();
  }

  /** After the worker's answer: `shutdown` to each live helper; resolves when they exited or after 2 s. */
  async shutdown(): Promise<void> {
    const live = this.#procs.filter((p) => p.proc !== undefined && !p.exited);
    if (live.length === 0) return;
    await new Promise<void>((resolve) => {
      let left = live.length;
      const cancel = this.#env.timers.after(HELPER_SHUTDOWN_MS, resolve);
      for (const p of live) {
        p.onExit = () => {
          if (--left === 0) {
            cancel();
            resolve();
          }
        };
        this.#env.host.send(p.proc as number, { t: "shutdown" });
      }
    });
  }

  /** Every helper's open host calls ended (their processes are gone). */
  async closeCalls(): Promise<void> {
    await Promise.all(this.#procs.map((p) => p.calls.close()));
  }
}
