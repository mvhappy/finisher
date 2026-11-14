// A fake host for the SDK's tests: spawns a bundled plugin as a real `node` process and speaks
// the host side of the `fin-jobs` envelope over its stdio (checks the plugin's `v` and `seq`,
// numbers its own lines from 1), answering host calls from a table of handlers.
import { type ChildProcessWithoutNullStreams, spawn } from "node:child_process";
import { createInterface } from "node:readline";
import type { HostMsg, JobError, PluginContext, StartJob, WorkerMsg } from "@finisher/core-types";

type Msg = WorkerMsg & { readonly v: number; readonly seq: number };
/** Answers one host call: a value, or `{error}` for an error reply; `undefined` never replies. */
export type CallHandler = (params: any, id: number) => unknown | { readonly error: JobError } | undefined;

export const context = (over: Partial<PluginContext["limits"]> = {}): PluginContext => ({
  hostApi: "1.1.0",
  plugin: { id: "@test/probe", version: "1.0.0" },
  grants: [{ perm: "project:read" }],
  dirs: { plugin: "/pkg", tmp: "/tmp/p" },
  locale: "en",
  limits: { timeoutS: 10, messageBytes: 1 << 20, pageBytes: 756 * 1024, ...over },
});

let jobs = 0;

/** A `plugin.command` v1 job of `command` with `args`, pinned at `rev`. */
export function commandJob(command: string, args: unknown = {}, rev = 7, extra: unknown = context()): StartJob {
  return { id: `j${++jobs}`, kind: "plugin.command", kindVersion: 1, attempt: 1, params: { command, args, project: "p1", branch: "main", rev }, extra } as StartJob;
}

/** A `plugin.hook` v1 job calling `handler` with `request` (a `HookRun`). */
export function hookJob(handler: string, request: unknown, extra: unknown = context()): StartJob {
  return { id: `j${++jobs}`, kind: "plugin.hook", kindVersion: 1, attempt: 1, params: { handler, request }, extra } as StartJob;
}

export class FakeHost {
  readonly child: ChildProcessWithoutNullStreams;
  readonly got: Msg[] = [];
  readonly stderr: string[] = [];
  readonly exited: Promise<number | null>;
  calls: Record<string, CallHandler> = {};
  #seq = 0;
  #inSeq = 0;
  #waiters: Array<() => void> = [];
  #problems: string[] = [];

  constructor(args: ReadonlyArray<string>) {
    this.child = spawn(process.execPath, args, { stdio: ["pipe", "pipe", "pipe"] });
    this.exited = new Promise((resolve) => this.child.on("exit", (code) => resolve(code)));
    createInterface({ input: this.child.stderr }).on("line", (l) => this.stderr.push(l));
    createInterface({ input: this.child.stdout }).on("line", (line) => {
      const m = JSON.parse(line) as Msg;
      if (m.v !== 1) this.#problems.push(`v ${m.v}`);
      if (m.seq !== ++this.#inSeq) this.#problems.push(`seq ${m.seq}, expected ${this.#inSeq}`);
      this.got.push(m);
      if (m.t === "call") this.#answer(m);
      for (const w of this.#waiters.splice(0)) w();
    });
    this.child.stdin.on("error", () => {});
  }

  /** Writes one host line (with `v` and the next `seq` unless `raw`). */
  send(m: HostMsg | Record<string, unknown>, raw = false): void {
    this.child.stdin.write(`${JSON.stringify(raw ? m : { v: 1, ...m, seq: ++this.#seq })}\n`);
  }

  /** Writes `text` as it is (no `v`, no `seq`, no newline added). */
  sendRaw(text: string): void {
    this.child.stdin.write(text);
  }

  start(job: StartJob, aliveMs = 5000): void {
    this.send({ t: "start", dirs: { job: "/job", tmp: "/job/tmp" }, limits: { aliveMs }, job });
  }

  /** The first message after index `from` that `pred` accepts (default: any). */
  async next(pred: (m: Msg) => boolean = () => true, from = 0, timeoutMs = 10_000): Promise<Msg> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const m = this.got.slice(from).find(pred);
      if (m !== undefined) return m;
      if (Date.now() > deadline) throw new Error(`no matching message; got ${JSON.stringify(this.got)}; stderr ${this.stderr.join("\n")}`);
      await new Promise<void>((resolve) => {
        const t = setTimeout(resolve, 50);
        this.#waiters.push(() => {
          clearTimeout(t);
          resolve();
        });
      });
    }
  }

  /** The job's `result` or `error`. */
  async outcome(job: string): Promise<Msg> {
    return this.next((m) => (m.t === "result" || m.t === "error") && m.job === job);
  }

  /** Runs `job` and returns its outcome. */
  async run(job: StartJob, aliveMs?: number): Promise<Msg> {
    this.start(job, aliveMs);
    return this.outcome(job.id);
  }

  /** The calls the plugin made, by method. */
  callsOf(method: string): Array<Msg & { t: "call" }> {
    return this.got.filter((m): m is Msg & { t: "call" } => m.t === "call" && m.method === method);
  }

  /** Envelope problems seen on the plugin's lines (`v`, `seq`). */
  get problems(): ReadonlyArray<string> {
    return this.#problems;
  }

  kill(): void {
    this.child.kill("SIGKILL");
  }

  #answer(m: Msg & { t: "call" }): void {
    const h = this.calls[m.method];
    if (h === undefined) return this.send({ t: "reply", id: m.id, error: { code: "INVALID_REQUEST", message: `no ${m.method}` } });
    const r = h(m.params, m.id);
    if (r === undefined) return;
    if (typeof r === "object" && r !== null && "error" in r) return this.send({ t: "reply", id: m.id, error: (r as { error: JobError }).error });
    this.send({ t: "reply", id: m.id, value: r });
  }
}
