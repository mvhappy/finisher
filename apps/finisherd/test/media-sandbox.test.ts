// Media workers under 7C's Linux Landlock `worker` profile (6B-1 plan Task 18, spec 6 §3.5.1),
// through a real job engine, the real job host and `SandboxSpawner`, and a real `fin-workers
// media` (the test-hooks `FIN_TEST_SANDBOX_READY=worker` makes the profile ready, as 7C Task 16
// will by default). Each kind runs under the profile and is granted only its job's files:
// `media.look` (warm, its project's roots) and `media.capabilities` say `sandbox: landlock`;
// `media.proxy`, `media.thumbs` and `media.probeDeep` read their source file; a library file's
// proxy reads that file, not its library's root; a look listing a proxy runs cold with the entry's
// files (Task 18's decision). Where Landlock is disabled (7C's test hook) jobs run and the
// server warns (`SANDBOX_DISABLED {profile: worker}`); where the profile cannot be applied the job
// fails `SANDBOX_UNAVAILABLE`. What a confined media worker cannot read or write is proven by
// `crates/fin-workers/tests/media_sandbox.rs` with unconfined controls. Linux only; skipped
// without Landlock (or without a GPU adapter) unless `FIN_REQUIRE_LANDLOCK=1` (`FIN_REQUIRE_GPU=1`).
import { copyFileSync, mkdirSync, mkdtempSync, realpathSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import type { FinError, JobView } from "@finisher/contracts";
import type { JobSpawnRequest, LookParams, ProxyParams, ThumbsParams } from "@finisher/core-types";
import { Engine, MediaCache, mediaProxyPlan, mediaThumbsPlan } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, describe, expect, test } from "vitest";
import { toFinError } from "../src/errors.ts";
import type { KindRegistry, KindSpec } from "../src/jobs/kinds.ts";
import { registerMediaKinds } from "../src/jobs/kinds/media.look.ts";
import { MEDIA_PROXY, libraryFileReads } from "../src/jobs/kinds/media.proxy.ts";
import { mediaCapabilities } from "../src/media/caps.ts";
import { makeProxyService } from "../src/media/proxy.ts";
import { proxy540Stage } from "../src/media/stages.ts";
import type { StageFile } from "../src/library/api.ts";
import type { ProjectRegistryShape } from "../src/projects.ts";
import { currentSandboxStatus, sandboxWarnings } from "../src/sandbox/status.ts";
import { recordingHost } from "./support/fake-host.ts";
import { closeTestEngines, testEngine } from "./support/jobs.ts";
import { lookProject, makeAv1Fixture } from "./support/media.ts";

const LINUX = process.platform === "linux";
const GPU_REQUIRED = process.env.FIN_REQUIRE_GPU === "1";
const OWNER = { kind: "local", id: "local" } as const;
const HOOKS = ["FIN_TEST_SANDBOX_READY", "FIN_TEST_SANDBOX_STATUS"] as const;
const saved = Object.fromEntries(HOOKS.map((k) => [k, process.env[k]]));

let work: string;
let media: string;
const engines: Engine[] = [];

beforeAll(() => {
  process.env.FIN_GPU ??= "software";
  work = realpathSync(mkdtempSync(join(tmpdir(), "fin-media-sandbox-")));
  media = join(work, "clip.mp4");
  if (LINUX) makeAv1Fixture(media, 12);
}, 600_000);

afterEach(async () => {
  await closeTestEngines();
  for (const e of engines.splice(0)) await e.close();
  for (const k of HOOKS) {
    if (saved[k] === undefined) delete process.env[k];
    else process.env[k] = saved[k];
  }
});

afterAll(() => rmSync(work, { recursive: true, force: true }));

/** `kind`'s spec with `reads` replaced (a host whose grants the profile cannot apply). */
function withReads(registry: KindRegistry, kind: string, reads: (extra: unknown) => ReadonlyArray<string>): KindRegistry {
  return new Proxy(registry, {
    get(target, prop, receiver) {
      if (prop !== "registerKind") return Reflect.get(target, prop, receiver);
      return (k: KindSpec) => target.registerKind(k.kind === kind ? { ...k, reads } : k);
    },
  });
}

/**
 * A project whose clip is in its `sources/` (inside the warm worker's roots), the media cache, the
 * media kinds on the real job host (recording each spawn), a proxy service; `status` is 7C's test
 * hook for the mechanism (`disabled`), `proxyReads` replaces `media.proxy`'s reads.
 */
async function setup(opts: { status?: "disabled"; proxyReads?: (extra: unknown) => ReadonlyArray<string> } = {}) {
  process.env.FIN_TEST_SANDBOX_READY = "worker";
  if (opts.status !== undefined) process.env.FIN_TEST_SANDBOX_STATUS = opts.status;
  else delete process.env.FIN_TEST_SANDBOX_STATUS;
  const serverData = realpathSync(mkdtempSync(join(work, "sd-")));
  const dir = join(realpathSync(mkdtempSync(join(work, "proj-"))), "p");
  mkdirSync(join(dir, "sources"), { recursive: true });
  const clip = join(dir, "sources", "clip.mp4");
  copyFileSync(media, clip);
  const p = await lookProject(serverData, dir, clip);
  engines.push(p.engine);
  const pid = p.engine.projectId;
  const cache = await MediaCache.open(serverData);
  let caps: ReturnType<typeof mediaCapabilities> | undefined;
  const run = <A>(f: () => Promise<string>) => Effect.tryPromise({ try: async () => JSON.parse(await f()) as A, catch: toFinError });
  const unknown = () => Effect.fail(toFinError(new Error("unknown project")));
  const projects: Pick<ProjectRegistryShape, "prepareMediaJob" | "dirOf" | "onBranch"> = {
    onBranch: (id, branch, f) => (id === pid ? run(() => f(p.engine, branch)) : unknown()),
    prepareMediaJob: (id, branch, timeline, at, inputDir, fontCache, paramsJson) =>
      id === pid ? run(() => p.engine.prepareMediaJob(branch, timeline, [...at], inputDir, { fontCache: fontCache ?? null, paramsJson: paramsJson ?? null })) : unknown(),
    dirOf: (id) => (id === pid ? dir : undefined),
  };
  const reqs: JobSpawnRequest[] = [];
  const deps = { serverData, projects, proxy: { cache: () => cache, report: () => caps?.current() } };
  const t = await testEngine({
    serverData,
    jobHost: recordingHost(reqs),
    register: (r) => registerMediaKinds(opts.proxyReads === undefined ? r : withReads(r, MEDIA_PROXY, opts.proxyReads), deps),
  });
  caps = mediaCapabilities({ engine: t.engine });
  const registry = {
    ensure: () => Effect.void,
    resolveBranch: (_: string, b: string) => Effect.succeed(b),
    snapshot: (_: string, b: string) => run<{ rev: number; project: unknown }>(() => p.engine.snapshot(b)),
    dirOf: projects.dirOf,
  };
  const proxies = makeProxyService({ registry, engine: t.engine, capabilities: caps, cache: () => cache });
  const lookParams: LookParams = { timeline: "tl1", at: ["0", "1/10"] as never, size: { width: 80, height: 45 }, outName: "look", projectId: pid, branch: "main", rev: 1 };
  /** The spawn of a cold job (its `cwd` is the job's directory). */
  const spawnOf = (v: JobView) => reqs.find((r) => basename(r.cwd) === v.id);
  return { t, reqs, cache, proxies, serverData, dir, clip, pid, lookParams, spawnOf };
}

/** Whether the profile is enforced here; else skip (or fail under `FIN_REQUIRE_LANDLOCK=1`). */
function enforced(s: Awaited<ReturnType<typeof setup>>, name: string): boolean {
  const caps = s.t.engine.workers().caps;
  if (caps.profiles.worker === "enforced" && caps.perJobGrants) return true;
  expect(process.env.FIN_REQUIRE_LANDLOCK === "1", `${name}: FIN_REQUIRE_LANDLOCK=1 but the worker profile is ${caps.profiles.worker}`).toBe(false);
  console.warn(`${name}: skipped, the worker profile is ${caps.profiles.worker} here`);
  return false;
}

/** The job's success, or `false` (the test stops) where this host has no GPU adapter. */
function ok(v: JobView, what: string): boolean {
  if (v.state === "failed" && v.error?.code === "GPU_UNAVAILABLE" && !GPU_REQUIRED) {
    console.warn(`${what}: no GPU adapter here; set FIN_REQUIRE_GPU=1 to fail instead`);
    return false;
  }
  expect(v.state, `${what}: ${JSON.stringify(v.error)}`).toBe("succeeded");
  return true;
}

/** A spawn's `read` grants. */
const readsOf = (r: JobSpawnRequest | undefined) => (r?.grants ?? []).filter((g) => g.mode === "read").map((g) => g.path);

/** A library holding the clip and another file, as the `proxy540` stage hands one to `media.proxy`. */
function libraryFile(s: Awaited<ReturnType<typeof setup>>): { params: ProxyParams; root: string; file: string } {
  const root = realpathSync(mkdtempSync(join(work, "lib-")));
  const file = join(root, "shots", "clip.mp4");
  mkdirSync(dirname(file), { recursive: true });
  copyFileSync(media, file);
  writeFileSync(join(root, "shots", "other.mp4"), "another file of the library");
  const st = statSync(file, { bigint: true });
  const f = {
    library: "l1",
    fileId: 1,
    root,
    rel: { components: ["shots", "clip.mp4"].map((c) => Buffer.from(c).toString("base64")) },
    display: "shots/clip.mp4",
    expect: { size: Number(st.size), mtimeNs: st.mtimeNs.toString(), fileId: Buffer.from(new BigUint64Array([st.ino]).buffer).reverse().toString("base64") },
    hydrate: false,
    fp: "fp-lib",
    size: Number(st.size),
    kind: "media",
    probe: { media: { video: { codec: "av1", display: { width: 160, height: 90 } } } },
    storage: "local",
  } as unknown as StageFile;
  const report = { codecs: { video: [{ codec: "av1", decode: "software", encode: "software", backends: ["dav1d", "svt-av1"] }] } };
  const params = proxy540Stage({ report: () => report, cache: () => s.cache }).params([f], { library: "l1", pools: {} as never }) as ProxyParams;
  return { params, root, file };
}

describe.runIf(LINUX)("media workers under the worker profile", () => {
  test("each_kind_runs_confined_to_its_job_s_files", async () => {
    const name = "each_kind_runs_confined_to_its_job_s_files";
    const s = await setup();
    if (!enforced(s, name)) return;
    const project = { project: s.pid };
    // The capabilities worker and a look: warm, in the project's warm worker, sandbox landlock.
    const caps = await s.t.runJob("media.capabilities", {}, project);
    expect(caps.state, JSON.stringify(caps.error)).toBe("succeeded");
    expect((caps.result as { worker: { sandbox: string } }).worker.sandbox).toBe("landlock");
    const look = await s.t.runJob("media.look", s.lookParams, project);
    if (!ok(look, "media.look")) return;
    expect(look.warm).toBe(true);
    // A proxy of the asset reads its source and nothing else (besides its job's in/, out/, tmp/).
    const job = (v: JobView) => join(s.serverData, "jobs", v.id);
    const proxied = await Effect.runPromise(Effect.result(s.proxies.request(OWNER, { projectId: s.pid, asset: "a1" })));
    if (proxied._tag === "Failure") throw proxied.failure;
    const proxyJob = s.reqs.find((r) => (r.grants ?? []).some((g) => g.path === join(r.cwd, "in", "manifest.json")) && readsOf(r).includes(s.clip));
    expect(readsOf(proxyJob).sort()).toEqual([join(proxyJob!.cwd, "in", "manifest.json"), s.clip].sort());
    // Thumbnails and the deep probe: the same.
    const planned = mediaThumbsPlan({ fingerprint: "fp-a1", mode: { thumbs: { times: ["0", "1/10"], maxSide: 64 } }, sourceCodec: "av1", decoders: ["dav1d"] });
    const thumbs: ThumbsParams = { fingerprint: "fp-a1", asset: "a1", mode: planned.mode, decoderClass: planned.decoderClass, key: planned.key, projectId: s.pid, branch: "main" };
    const th = await s.t.runJob("media.thumbs", thumbs, project);
    if (!ok(th, "media.thumbs")) return;
    expect(readsOf(s.spawnOf(th))).toEqual([join(job(th), "in", "manifest.json"), s.clip]);
    const deep = await s.t.runJob("media.probeDeep", { asset: "a1", fingerprint: "fp-a1", projectId: s.pid, branch: "main" }, project);
    expect(deep.state, JSON.stringify(deep.error)).toBe("succeeded");
    expect(readsOf(s.spawnOf(deep))).toEqual([join(job(deep), "in", "manifest.json"), s.clip]);
    // Every spawn under the profile.
    for (const r of s.reqs) expect(r.profile).toBe("worker");
  }, 300_000);

  test("a_library_file_s_proxy_reads_that_file_not_its_library", async () => {
    const s = await setup();
    if (!enforced(s, "a_library_file_s_proxy_reads_that_file_not_its_library")) return;
    const { params, root, file } = libraryFile(s);
    const v = await s.t.runJob(MEDIA_PROXY, params, { library: "l1" });
    if (!ok(v, "media.proxy of a library file")) return;
    expect(readsOf(s.spawnOf(v))).toEqual([file]);
    expect(readsOf(s.spawnOf(v))).not.toContain(root);
  }, 300_000);

  test("a_look_listing_a_proxy_runs_cold_with_the_entry_s_files", async () => {
    const s = await setup();
    if (!enforced(s, "a_look_listing_a_proxy_runs_cold_with_the_entry_s_files")) return;
    const made = await Effect.runPromise(Effect.result(s.proxies.request(OWNER, { projectId: s.pid, asset: "a1", tier: "1080" })));
    if (made._tag === "Failure") {
      if ((made.failure as FinError).code === "GPU_UNAVAILABLE" && !GPU_REQUIRED) return;
      throw made.failure;
    }
    const entry = s.cache.entryDir(made.success.key)!;
    const look = await s.t.runJob("media.look", s.lookParams, { project: s.pid });
    if (!ok(look, "media.look from a proxy")) return;
    expect((look.result as { frames: Array<{ source: string }> }).frames.map((f) => f.source)).toEqual(["proxy", "proxy"]);
    // The entry is outside the project's warm roots: the look ran cold (Decision 15 as amended).
    expect(look.warm).not.toBe(true);
    const reads = readsOf(s.spawnOf(look));
    for (const f of ["meta.json", "proxy.mp4", "table.bin"]) expect(reads).toContain(join(entry, f));
    expect(reads).not.toContain(entry);
    expect(reads).not.toContain(dirname(entry));
  }, 300_000);

  test("landlock_disabled_runs_with_warning", async () => {
    const s = await setup({ status: "disabled" });
    const caps = s.t.engine.workers().caps;
    expect(caps.profiles.worker).toBe("disabled");
    // The server's persistent warning names the worker profile and how to turn Landlock on.
    const warnings = sandboxWarnings(currentSandboxStatus(), ["worker"]);
    expect(warnings).toEqual([expect.objectContaining({ code: "SANDBOX_DISABLED", details: expect.objectContaining({ profile: "worker", reason: "landlockDisabled" }) })]);
    // The jobs run, unconfined, and the capability answer says so.
    const project = { project: s.pid };
    const c = await s.t.runJob("media.capabilities", {}, project);
    expect(c.state, JSON.stringify(c.error)).toBe("succeeded");
    expect((c.result as { worker: { sandbox: string } }).worker.sandbox).toBe("none");
    const look = await s.t.runJob("media.look", s.lookParams, project);
    ok(look, "media.look with Landlock disabled");
  }, 300_000);

  test("apply_failure_fails_the_job", async () => {
    // A read the profile cannot apply as given (a link: Landlock would grant its target).
    const link = join(realpathSync(mkdtempSync(join(work, "link-"))), "clip.mp4");
    symlinkSync(media, link);
    const s = await setup({ proxyReads: () => [link] });
    if (!enforced(s, "apply_failure_fails_the_job")) return;
    const { params } = libraryFile(s);
    const v = await s.t.runJob(MEDIA_PROXY, params, { library: "l1" });
    expect([v.state, v.error?.code]).toEqual(["failed", "SANDBOX_UNAVAILABLE"]);
    expect(v.error?.details).toMatchObject({ profile: "worker", os: "linux" });
    // Deterministic: not tried again.
    expect(s.reqs.filter((r) => r.family === "media")).toHaveLength(1);
  }, 300_000);
});

describe.skipIf(process.platform === "win32")("libraryFileReads", () => {
  const b64 = (s: string | Buffer) => Buffer.from(s).toString("base64");
  test("the file itself, never its root; nothing through a link or for a name this process cannot write", async () => {
    const root = realpathSync(mkdtempSync(join(work, "reads-")));
    mkdirSync(join(root, "a", "b"), { recursive: true });
    writeFileSync(join(root, "a", "b", "f.mp4"), "f");
    symlinkSync(join(root, "a"), join(root, "via"));
    symlinkSync(join(root, "a", "b", "f.mp4"), join(root, "a", "b", "l.mp4"));
    const rel = (...c: Array<string | Buffer>) => ({ components: c.map(b64) });
    expect(await libraryFileReads({ root, rel: rel("a", "b", "f.mp4") })).toEqual([join(root, "a", "b", "f.mp4")]);
    // A link at the file or on the way, a directory, a missing file, an escape: nothing.
    expect(await libraryFileReads({ root, rel: rel("a", "b", "l.mp4") })).toEqual([]);
    expect(await libraryFileReads({ root, rel: rel("via", "b", "f.mp4") })).toEqual([]);
    expect(await libraryFileReads({ root, rel: rel("a", "b") })).toEqual([]);
    expect(await libraryFileReads({ root, rel: rel("a", "b", "gone.mp4") })).toEqual([]);
    expect(await libraryFileReads({ root, rel: rel("..", "x") })).toEqual([]);
    expect(await libraryFileReads({ root, rel: rel(Buffer.from([0xff]), "..") })).toEqual([]);
    expect(await libraryFileReads({ root, rel: { components: [] } })).toEqual([]);
    // A name that is not UTF-8: nothing (never an ancestor, which would grant its siblings).
    expect(await libraryFileReads({ root, rel: rel("a", "b", Buffer.from([0xff, 0x2e])) })).toEqual([]);
    expect(await libraryFileReads({ root, rel: rel(Buffer.from([0xff]), "b", "f.mp4") })).toEqual([]);
    // A leading U+FEFF is part of the name: the BOM file is granted, never its stripped sibling.
    writeFileSync(join(root, "a", "b", "\uFEFFf.mp4"), "x");
    expect(await libraryFileReads({ root, rel: rel("a", "b", "\uFEFFf.mp4") })).toEqual([join(root, "a", "b", "\uFEFFf.mp4")]);
    expect(await libraryFileReads({ root: join(root, "missing"), rel: rel("f.mp4") })).toEqual([]);
  });
});
