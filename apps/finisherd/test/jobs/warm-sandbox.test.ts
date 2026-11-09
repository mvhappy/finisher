// Warm workers under a sandbox (plan 7C Task 12, Decision 19): a warm worker is granted its own
// directory, never the jobs root; each job visits its `jobs/` for the attempt and leaves after;
// under an applied profile a warm key is per project and the worker reads only that project's
// roots. Real `fin-workers test --warm` under the Landlock `worker` profile (the test-hooks
// `FIN_TEST_SANDBOX_READY=worker`) for what a worker can and cannot read, each case beside its
// unsandboxed control; the scripted `FakeHost` for the pool's choices. `FIN_REQUIRE_LANDLOCK=1`
// turns a skip on a host without Landlock into a failure.
import { existsSync, mkdirSync, mkdtempSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { HostMsg, JobSpawnRequest, TestWarmResult } from "@finisher/core-types";
import { afterEach, describe, expect, test } from "vitest";
import { FakeHost, recordingHost } from "../support/fake-host.ts";
import { closeTestEngines, testEngine, until } from "../support/jobs.ts";

const EACCES = 13;
const scratch: string[] = [];
const savedReady = process.env.FIN_TEST_SANDBOX_READY;
afterEach(async () => {
  await closeTestEngines();
  for (const d of scratch.splice(0)) rmSync(d, { recursive: true, force: true });
  if (savedReady === undefined) delete process.env.FIN_TEST_SANDBOX_READY;
  else process.env.FIN_TEST_SANDBOX_READY = savedReady;
});

const tmp = () => {
  const d = realpathSync(mkdtempSync(join(tmpdir(), "warm-sandbox-")));
  scratch.push(d);
  return d;
};
const warmDirs = (root: string) => (existsSync(root) ? readdirSync(root).filter((n) => n.startsWith("warm-")) : []);
const isWarm = (r: JobSpawnRequest) => r.args?.includes("--warm") === true;

/** Two projects' sources, and a file in another job's directory. */
function layout(base: string, jobsRoot: string) {
  const src = (p: string) => {
    const d = join(base, "projects", p, "sources");
    mkdirSync(d, { recursive: true });
    writeFileSync(join(d, "clip.mov"), p);
    return d;
  };
  const other = join(jobsRoot, "01OTHERJOB0000000000000000", "in");
  mkdirSync(other, { recursive: true });
  writeFileSync(join(other, "x"), "not yours");
  return { a: src("A"), b: src("B"), otherJob: join(other, "x") };
}

describe("a real warm worker under the worker profile", () => {
  async function engine(sandboxed: boolean) {
    if (sandboxed) process.env.FIN_TEST_SANDBOX_READY = "worker";
    else delete process.env.FIN_TEST_SANDBOX_READY;
    const base = tmp();
    const serverData = join(base, "data");
    mkdirSync(serverData, { recursive: true, mode: 0o700 });
    const reqs: JobSpawnRequest[] = [];
    const projects = join(base, "projects");
    const t = await testEngine({
      serverData,
      jobHost: recordingHost(reqs),
      testKinds: { families: { test: { warmReads: (job) => [join(projects, job.project ?? "none", "sources")] } } },
    });
    const root = join(serverData, "jobs");
    return { t, reqs, root, files: layout(base, root) };
  }

  /** Whether this host enforces the profile; else skip (or fail under `FIN_REQUIRE_LANDLOCK=1`). */
  function enforced(caps: { profiles: { worker: string }; perJobGrants: boolean }, name: string): boolean {
    if (caps.profiles.worker === "enforced" && caps.perJobGrants) return true;
    expect(process.env.FIN_REQUIRE_LANDLOCK === "1", `${name}: FIN_REQUIRE_LANDLOCK=1 but the worker profile is ${caps.profiles.worker}`).toBe(false);
    return false;
  }

  const look = async (t: Awaited<ReturnType<typeof engine>>["t"], project: string, probe: string[]) => {
    const v = await t.runJob("test.warm", { value: project, probe }, { project });
    expect(v.state, JSON.stringify(v.error)).toBe("succeeded");
    expect(v.warm).toBe(true);
    return v.result as TestWarmResult;
  };

  test("a_warm_worker_reads_only_its_own_job_and_project", async () => {
    const e = await engine(true);
    if (!enforced(e.t.engine.workers().caps, "a_warm_worker_reads_only_its_own_job_and_project")) return;
    const { a, b, otherJob } = e.files;
    const probe = [join(a, "clip.mov"), join(b, "clip.mov"), otherJob, e.root, "jobs"];
    const one = await look(e.t, "A", probe);
    expect(one.probes).toEqual([null, EACCES, EACCES, EACCES, null]);
    // The spawn: its own directory and `jobs/`, its project's sources, never the jobs root.
    const spawn = e.reqs.find(isWarm)!;
    expect(spawn.grants).not.toContainEqual(expect.objectContaining({ path: e.root }));
    expect(spawn.grants).toContainEqual({ path: join(spawn.cwd, "jobs"), mode: "readWrite" });
    expect(spawn.grants).toContainEqual({ path: a, mode: "read" });
    // After `ready` nothing of the job is left in the worker's directory.
    const [dir] = warmDirs(e.root);
    expect(readdirSync(join(e.root, dir!, "jobs"))).toEqual([]);
    // The next job of the project runs on the same worker; it sees only itself.
    const two = await look(e.t, "A", ["jobs"]);
    expect([two.pid, two.served]).toEqual([one.pid, 2]);
    // Another project gets its own worker, which reads only its own sources.
    const other = await look(e.t, "B", [join(a, "clip.mov"), join(b, "clip.mov")]);
    expect(other.pid).not.toBe(one.pid);
    expect(other.probes).toEqual([EACCES, null]);
  });

  test("control_unsandboxed_the_same_probes_read", async () => {
    const e = await engine(false);
    expect(e.t.engine.workers().caps.profiles.worker).not.toBe("enforced");
    const { a, b, otherJob } = e.files;
    const one = await look(e.t, "A", [join(a, "clip.mov"), join(b, "clip.mov"), otherJob, e.root, "jobs"]);
    expect(one.probes).toEqual([null, null, null, null, null]);
    // Unconfined, the key is not per project: one worker serves both.
    const other = await look(e.t, "B", []);
    expect(other.pid).toBe(one.pid);
  });
});

describe("the pool under a sandbox (scripted host)", () => {
  /** A scripted `test` worker: answers each `start` (with what `fail` says), then `ready`; or exits. */
  function host(caps: { perJobGrants: boolean }, crash = false) {
    const reqs = new Map<number, JobSpawnRequest>();
    const fake = new FakeHost((h, proc, req) => {
      reqs.set(proc, req);
      h.emit({ type: "hello", proc, hello: { v: 1, minor: 0, role: "worker", family: "test", kinds: [{ kind: "test.warm", version: 1 }], warm: isWarm(req), pid: 1000 + proc } });
    });
    fake.capsOverride = { profiles: { worker: "enforced", plugin: "disabled" }, perJobGrants: caps.perJobGrants };
    const visits: string[][] = [];
    fake.onSend = (h, proc, msg: HostMsg) => {
      if (msg.t !== "start") return;
      const req = reqs.get(proc)!;
      if (isWarm(req)) visits.push([msg.dirs.job, ...readdirSync(join(req.cwd, "jobs"))]);
      if (crash && isWarm(req)) return h.exit(proc, { reason: "exited", code: 139 });
      h.emit({ type: "message", proc, msg: { t: "result", job: msg.job.id, value: { value: null, pid: 1000 + proc, served: 1 } } });
      if (isWarm(req)) h.emit({ type: "message", proc, msg: { t: "ready" } });
      else h.exit(proc, { reason: "exited", code: 0 });
    };
    return { fake, visits };
  }

  test("per_job_grants_admit_warm_kinds_one_worker_per_project", async () => {
    const { fake, visits } = host({ perJobGrants: true });
    const t = await testEngine({ jobHost: fake.factory, testKinds: { families: { test: { warmReads: (job) => [`/srv/${job.project}`] } } } });
    const root = join(t.serverData, "jobs");
    const runs = [];
    for (const p of ["A", "A", "B"]) runs.push(await t.runJob("test.warm", { value: p }, { project: p }));
    expect(runs.map((v) => [v.state, v.warm])).toEqual([
      ["succeeded", true],
      ["succeeded", true],
      ["succeeded", true],
    ]);
    const warm = fake.spawns.filter(isWarm);
    expect(warm.map((r) => r.label)).toEqual(["warm test:A/", "warm test:B/"]);
    expect(warm.map((r) => r.grants?.filter((g) => g.mode === "read").map((g) => g.path))).toEqual([["/srv/A"], ["/srv/B"]]);
    // Each job visited its worker's `jobs/` alone, then went home (its directory removed at its end).
    for (const [i, v] of runs.entries()) {
      expect(visits[i]![0]).toBe(join(warm[i === 2 ? 1 : 0]!.cwd, "jobs", v.id));
      expect(visits[i]!.slice(1)).toEqual([v.id]);
    }
    for (const d of warmDirs(root)) expect(readdirSync(join(root, d, "jobs"))).toEqual([]);
  });

  test("a_crash_mid_job_leaves_nothing_in_a_workers_directory", async () => {
    const { fake } = host({ perJobGrants: true }, true);
    const t = await testEngine({ jobHost: fake.factory });
    const root = join(t.serverData, "jobs");
    const v = await t.runJob("test.warm", { value: 1 }, { project: "A" });
    expect(v.state).not.toBe("running");
    // The crashed workers' directories go, and no job directory is left in any warm one.
    await until(() => warmDirs(root).every((d) => !existsSync(join(root, d, "jobs", v.id))) || undefined);
    await until(() => warmDirs(root).length === 0 || undefined);
  });

  test("without_per_job_grants_warm_kinds_run_cold", async () => {
    const { fake } = host({ perJobGrants: false });
    const t = await testEngine({ jobHost: fake.factory });
    const v = await t.runJob("test.warm", { value: 1 }, { project: "A" });
    expect([v.state, v.warm]).toEqual(["succeeded", false]);
    expect(fake.spawns.filter(isWarm)).toEqual([]);
  });

  test("a_job_reading_outside_its_workers_roots_runs_cold", async () => {
    const { fake, visits } = host({ perJobGrants: true });
    const t = await testEngine({
      jobHost: fake.factory,
      testKinds: { families: { test: { warmReads: () => ["/srv/A"] } }, overrides: { "test.warm": { reads: () => ["/srv/B/clip.mov"] } } },
    });
    const v = await t.runJob("test.warm", { value: 1 }, { project: "A" });
    expect([v.state, v.warm]).toEqual(["succeeded", false]);
    // Never started on the warm worker, which stays for the next job.
    expect(visits).toEqual([]);
    expect(fake.spawns.filter((r) => !isWarm(r) && r.role === "worker")).toHaveLength(1);
    const next = await t.runJob("test.warm", { value: 2 }, { project: "A" });
    expect(next.state).toBe("succeeded");
  });
});
