// The `media` family and the `media.look` kind (6A plan Task 12; sub-project 6 spec §3.3).
// The worker is `fin-workers media` (Task 11). Around it:
// - `prepare` makes the job's `in/`, then asks the project's engine to write `snapshot.json`,
//   `manifest.json` and `params.json` there (`Engine.prepareMediaJob`), and passes the plan
//   hashes it computed to the worker as `extra.planSamples`;
// - `onSucceeded` puts each `out/<outName>-<i>.png` in its final place with `commitJobOutput`
//   (`fin-shm::commit_output_copy`) one file at a time, before 5C deletes the job directory.
//   The worker is warm: it has answered and is idle, but it is alive and may hold the file
//   open, so nothing is renamed out of its reach: the bytes of the checked handle are copied
//   into a temporary file the host creates beside the destination and that is renamed over it
//   (review B1). The name of every destination is built from the host's own params and plan
//   samples (`p.at[i]`, `extra.planSamples[i]`), never from what the worker's answer says
//   (review B2).
import { realpathSync } from "node:fs";
import { mkdir, realpath, stat } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { FinError, type JobView } from "@finisher/contracts";
import type { LookFrame, LookParams, LookResult, LookSource } from "@finisher/core-types";
import { commitJobOutput } from "@finisher/fin-node";
import { Effect } from "effect";
import { finError, toFinError } from "../../errors.ts";
import type { ProjectRegistryShape } from "../../projects.ts";
import { jobDirs } from "../files.ts";
import { mkdirSafe } from "../fsafe.ts";
import { type KindRegistry, type KindSpec, type MemoryRule, projectOfJob } from "../kinds.ts";
import { mediaCapabilitiesKind } from "./media.capabilities.ts";
import { mediaPcmKind } from "./media.pcm.ts";
import { mediaProbeDeepKind } from "./media.probeDeep.ts";
import { type ProxyKindDeps, mediaProxyKind } from "./media.proxy.ts";
import { mediaThumbsKind } from "./media.thumbs.ts";

export const MEDIA_FAMILY = "media";
export const MEDIA_LOOK = "media.look";

/**
 * The `interactive:media` group's limit (kept by 6B-1, Decision 18: a deviation from
 * spec §14's interactive row for the warm frame cache; batch kinds use `media/memory.ts`). A warm worker keeps up to 8 open sources, each with up
 * to 64 decoded frames (about 200 MB per 1080p source), the GPU's staging and the PNGs being
 * encoded: half the RAM, at most 6 GiB (Decision 20's rule shape).
 */
export const MEDIA_MEMORY: MemoryRule = { ramFraction: 0.5, capMiB: 6144 };

/** A look's longest side in pixels (`LOOK_MAX_SIDE` in `fin-media`). */
export const LOOK_MAX_SIDE = 8192;

/** The largest PNG `onSucceeded` moves: 8192 × 8192 RGBA8, uncompressed. */
export const MAX_FRAME_BYTES = LOOK_MAX_SIDE * LOOK_MAX_SIDE * 4 + (1 << 20);

/** Where a look's frames go when the request names no directory (Decision 7). */
export const LOOK_DIR = ["workspace", "shared", "look"] as const;

/** Directories of a project folder that no look or export may write into (spec §17). */
const PROTECTED_DIRS = [".finisher", "assets"] as const;

export interface MediaKindDeps {
  /** `<server-data>`: the jobs root and the font cache are below it. */
  readonly serverData: string;
  /** The project registry's reads: the engine call and the folder a project lives in. */
  readonly projects: Pick<ProjectRegistryShape, "prepareMediaJob" | "dirOf"> & Partial<Pick<ProjectRegistryShape, "onBranch">>;
  /** `<server-data>/jobs` (default: `join(serverData, "jobs")`, which `ensureJobsRoot` makes). */
  readonly jobsRoot?: string;
  /** The libraries' roots (5D; every library in v1): what a confined warm media worker may read beside its project's `sources/`. */
  readonly libraryRoots?: () => ReadonlyArray<string>;
  /** `media.proxy`'s cache and capability report (6B-1 Task 14); absent: the kind is not registered. */
  readonly proxy?: Pick<ProxyKindDeps, "cache" | "report">;
}

/** The job's params as the kind reads them: `LookParams`, whose pins the submitter must set. */
export type LookJobParams = LookParams;

const invalid = (field: string, why: string): FinError => finError("JOB_PARAMS_INVALID", `params.${field}: ${why}`, { field }, "submit a media.look with its project, branch and rev");

/** The `(project, branch, rev, timeline, at, size, out, originals)` a look is: equal looks coalesce. */
export function lookDedupe(p: LookJobParams): string | undefined {
  if (p.projectId == null || p.branch == null || p.rev == null) return undefined;
  return JSON.stringify([p.projectId, p.branch, p.rev, p.timeline, p.at, p.size ?? null, p.outName, p.outDir ?? null, p.originals === true]);
}

const RATIONAL = /^-?\d{1,38}(?:\/\d{1,38})?$/;
const PLAN_HASH = /^[0-9a-f]{64}$/;

/**
 * `<timeline>-<n>_<d>-<w>x<h>-<plan hash, 8 hex>.png` (Decision 7), `….-proxy.png` for a frame
 * read from a proxy. The size and the source are in the name because the plan hash covers
 * neither: two looks of the same times at different sizes, or from a proxy and from the
 * original, into one folder must keep both files.
 */
export function finalName(timeline: string, at: string, size: { readonly width: number; readonly height: number }, plan: string, source: LookSource = "original"): string {
  if (!RATIONAL.test(at)) throw finError("JOB_FAILED", "a frame time is not a rational number", { at });
  for (const side of [size.width, size.height]) {
    if (!Number.isSafeInteger(side) || side < 1 || side > LOOK_MAX_SIDE) throw finError("JOB_FAILED", "a frame size is not a pixel count", { width: size.width, height: size.height });
  }
  if (!PLAN_HASH.test(plan)) throw finError("JOB_FAILED", "a frame's plan hash is not a BLAKE3 digest", { plan });
  const [n, d] = at.split("/");
  const safe = timeline.replace(/[^A-Za-z0-9_-]/g, "_");
  return `${safe}-${n}_${d ?? "1"}-${size.width}x${size.height}-${plan.slice(0, 8)}${source === "proxy" ? "-proxy" : ""}.png`;
}

const FOLD = process.platform === "win32" || process.platform === "darwin";
const fold = (p: string): string => (FOLD ? p.toLowerCase() : p);

/** Whether `path` is `dir` or below it (both absolute, the same spelling of the volume). */
function inside(path: string, dir: string): boolean {
  const rel = relative(fold(dir), fold(path));
  return rel === "" || (!rel.startsWith("..") && !isAbsolute(rel));
}

const exportDestination = (dir: string, why: string): FinError =>
  finError("EXPORT_DESTINATION", `cannot write frames to ${dir}: ${why}`, { dir, reason: why }, "name a folder of the project's workspace, outside .finisher/ and assets/");

/** The deepest ancestor of `path` that exists, resolved through links, with the rest appended. */
async function resolveThroughLinks(path: string): Promise<string> {
  const rest: string[] = [];
  let cur = path;
  for (;;) {
    try {
      return join(await realpath(cur), ...rest.reverse());
    } catch (e) {
      const parent = resolve(cur, "..");
      if ((e as { code?: unknown }).code !== "ENOENT" || parent === cur) throw e;
      rest.push(basename(cur));
      cur = parent;
    }
  }
}

/** `dev:ino` of `path` (not following a final link), or `undefined` when it does not exist. */
async function identity(path: string): Promise<string | undefined> {
  try {
    const st = await stat(path, { bigint: true });
    return `${st.dev}:${st.ino}`;
  } catch (e) {
    if ((e as { code?: unknown }).code === "ENOENT" || (e as { code?: unknown }).code === "ENOTDIR") return undefined;
    throw e;
  }
}

/** `dir` and every directory above it, nearest first. */
function ancestors(dir: string): string[] {
  const out: string[] = [];
  for (let cur = dir; ; cur = resolve(cur, "..")) {
    out.push(cur);
    if (resolve(cur, "..") === cur) return out;
  }
}

/**
 * Where a look's frames go: `outDir` (absolute, or relative to the project folder), else
 * `workspace/shared/look/`; created if need be. Refused (`EXPORT_DESTINATION`) when:
 * - it lies inside `.finisher/` or `assets/` of the project, spelled or resolved through links
 *   (compared by name, and by `(dev, ino)` against the protected folders and **every ancestor** of
 *   the resolved destination, so a case-folded name on a case-insensitive volume, an 8.3 name, a
 *   junction or a bind mount cannot get around the names);
 * - any ancestor of the resolved destination is named `.finisher` (case-folded, whatever the
 *   volume), or is one of `protect` (the server's data);
 * - it cannot be made (review M1).
 */
export async function lookDestination(projectDir: string, outDir: string | null | undefined, protect: ReadonlyArray<string> = []): Promise<string> {
  const base = outDir == null ? join(projectDir, ...LOOK_DIR) : resolve(projectDir, outDir);
  const check = async (): Promise<string> => {
    const root = await realpath(projectDir);
    const real = await resolveThroughLinks(base);
    for (const name of PROTECTED_DIRS) {
      if (inside(base, join(projectDir, name)) || inside(real, join(root, name))) throw exportDestination(base, `inside ${name}/`);
    }
    const guarded = new Map<string, string>();
    for (const [what, dir] of [...PROTECTED_DIRS.map((n) => [`${n}/`, join(root, n)] as const), ...protect.map((p) => ["the server's data", p] as const)]) {
      const id = await identity(dir);
      if (id !== undefined) guarded.set(id, what);
    }
    for (const a of ancestors(real)) {
      if (basename(a).toLowerCase() === ".finisher") throw exportDestination(base, "inside a .finisher folder");
      const id = await identity(a);
      const what = id === undefined ? undefined : guarded.get(id);
      if (what !== undefined) throw exportDestination(base, `inside ${what}`);
    }
    return real;
  };
  try {
    await check();
    await mkdir(base, { recursive: true });
    return await check();
  } catch (e) {
    if (e instanceof FinError) throw e;
    const cause = (e as { code?: unknown }).code;
    throw exportDestination(base, typeof cause === "string" ? cause : "not writable");
  }
}

/** A file `fin-shm` refused, with the job directory's absolute path made job-relative. */
function refusal(e: unknown, jobDir: string): FinError {
  const err = toFinError(e);
  const file = (err.details as { file?: unknown } | undefined)?.file;
  if (typeof file !== "string") return err;
  const rel = inside(file, jobDir) ? relative(jobDir, file).split(sep).join("/") : "<outside the job directory>";
  return finError(err.code, err.message.split(file).join(rel), { ...(err.details as object), file: rel }, err.fix);
}

const exists = (path: string): Promise<boolean> =>
  stat(path).then(
    (s) => s.isFile(),
    () => false,
  );

/**
 * A confined warm media worker's read roots (plan 7C Decision 19): its project's `sources/` and
 * the libraries' roots, canonical (synchronous: once per warm worker spawned). A root that does
 * not resolve is left out; a job reading below it then runs cold.
 */
export function mediaWarmReads(deps: Pick<MediaKindDeps, "projects" | "libraryRoots" | "serverData">, job: JobView): ReadonlyArray<string> {
  const project = projectOfJob(job);
  const dir = project === undefined ? undefined : deps.projects.dirOf(project);
  // The font cache too: `prepare` lists it in every job's reads (Task 12 review S1).
  const roots = [...(dir === undefined ? [] : [join(dir, "sources")]), ...(deps.libraryRoots?.() ?? []), join(deps.serverData, "fonts")];
  return roots.flatMap((r) => {
    try {
      return [realpathSync(r)];
    } catch {
      return [];
    }
  });
}

/** The `media` family (`fin-workers media`; `--warm` is appended by the engine). */
export function registerMediaFamily(registry: KindRegistry, deps?: Pick<MediaKindDeps, "projects" | "libraryRoots" | "serverData">): void {
  registry.registerFamily({
    family: MEDIA_FAMILY,
    profile: "worker",
    ...(deps === undefined ? {} : { warmReads: (job: JobView) => mediaWarmReads(deps, job) }),
    program: { binary: "fin-workers", args: [MEDIA_FAMILY] },
    memory: { interactive: MEDIA_MEMORY, batch: MEDIA_MEMORY },
    // 5C clears a worker's environment; `FIN_GPU` picks the adapter (`software` for CI), and the
    // Vulkan loader's ICD variables the driver (a host with only a user-space one, 6B-1 Task 14;
    // the worker profile grants the files they name).
    env: ["FIN_GPU", "VK_ICD_FILENAMES", "VK_DRIVER_FILES", "VK_ADD_DRIVER_FILES"],
    // The kinds the worker may send as `diag` (Task 14); the journal sink records them.
    diagKinds: ["media.capabilities", "render.stats"],
  });
}

/** `media.look` v1: frames of a timeline at some times, as PNGs in the project's workspace. */
export function mediaLookKind(deps: MediaKindDeps): KindSpec<LookJobParams, LookResult> {
  const root = deps.jobsRoot ?? join(deps.serverData, "jobs");
  return {
    kind: MEDIA_LOOK,
    version: 1,
    family: MEDIA_FAMILY,
    scopes: ["project.read"],
    resources: [],
    warm: true,
    defaultPriority: "interactive",
    qos: "normal",
    timeoutS: 60,
    stallS: 30,
    maxAttempts: 2,
    files: [
      { name: "in/snapshot.json", mode: "read" },
      { name: "in/manifest.json", mode: "read" },
      { name: "out/", mode: "write" },
    ],
    dedupe: lookDedupe,
    journal: lookJournal,
    prepare: (ctx) =>
      Effect.gen(function* () {
        const p = ctx.job.params;
        if (p.projectId == null) return yield* Effect.fail(invalid("projectId", "required"));
        if (p.branch == null) return yield* Effect.fail(invalid("branch", "required"));
        const dirs = jobDirs(root, ctx.job.id);
        yield* Effect.tryPromise({ try: () => mkdirSafe(dirs.job, "in"), catch: toFinError });
        const { planSamples: _, ...request } = p;
        const done = yield* deps.projects.prepareMediaJob(
          p.projectId,
          p.branch,
          p.timeline,
          p.at.map(String),
          join(dirs.job, "in"),
          join(deps.serverData, "fonts"),
          JSON.stringify(request),
        );
        // The proxies the cache holds for the manifest's assets (6B-1 Task 16), pinned for this
        // job's life (released when it ends: `releaseLookPins`) and serving the project (a hit:
        // it reaches them and pins them while open), and the files the worker reads from them.
        // A look of originals lists none; a cache that is down lists none.
        const projectId = p.projectId;
        const cache = p.originals === true ? undefined : deps.proxy?.cache();
        const proxyFiles = cache === undefined ? [] : yield* Effect.tryPromise({ try: () => cache.attachProxies(join(dirs.job, "in"), `job:${ctx.job.id}`, [projectId]), catch: toFinError });
        const reads = yield* Effect.promise(() => canonicalReads([...(done.sources ?? []), ...proxyFiles, join(deps.serverData, "fonts")]));
        return { extra: { planSamples: done.planSamples, rev: done.rev, reads } };
      }),
    // The sources and the font cache, granted to the worker per file (plan 7C Task 7).
    reads: readsOf,
    onSucceeded: (ctx) =>
      Effect.tryPromise({
        try: async (): Promise<LookResult> => {
          const p = ctx.job.params;
          const dir = p.projectId == null ? undefined : deps.projects.dirOf(p.projectId);
          if (dir === undefined) throw finError("PROJECT_NOT_FOUND", `project ${p.projectId ?? ""} is not open`, { projectId: p.projectId ?? null }, "open the project and look again");
          const dest = await lookDestination(dir, p.outDir, [deps.serverData]);
          const jobDir = jobDirs(root, ctx.job.id).job;
          const samples = planSamplesOf(ctx.extra);
          const name = new RegExp(`^${p.outName}-(\\d+)\\.png$`);
          const frames: LookFrame[] = [];
          for (const [position, f] of ctx.result.frames.entries()) {
            // The worker's own name for the file, `<outName>-<i>.png`, is all of its answer that
            // is believed: the time, the plan hash, the size (when named) and the destination come from the host's.
            const i = Number(name.exec(f.file)?.[1] ?? Number.NaN);
            if (!Number.isSafeInteger(i) || i !== position || i >= p.at.length) throw finError("JOB_FAILED", "the worker named an output file oddly", { file: f.file });
            const at = String(p.at[i]);
            const want = samples[i];
            if (!PLAN_HASH.test(f.plan) || f.plan !== want) {
              throw finError("PLAN_MISMATCH", "the worker's frame does not carry the plan the host sampled", { frame: i, reason: PLAN_HASH.test(f.plan) ? "hash" : "malformed" });
            }
            // The size drawn is the named one (the host's own param), else the timeline's
            // resolution, which only the worker's frame says: it is checked as a pixel count.
            if (p.size != null && (f.width !== p.size.width || f.height !== p.size.height)) {
              throw finError("JOB_FAILED", "the worker drew a frame at another size than the one asked for", { frame: i });
            }
            if (f.source !== "proxy" && f.source !== "original") throw finError("JOB_FAILED", "the worker did not say where a frame was read from", { frame: i });
            const to = join(dest, finalName(p.timeline, at, { width: f.width, height: f.height }, f.plan, f.source));
            if (dirname(to) !== dest) throw finError("JOB_FAILED", "a frame's destination is outside its folder", { frame: i });
            try {
              await commitJobOutput(join(jobDir, "out", f.file), to, MAX_FRAME_BYTES, true);
            } catch (e) {
              const err = refusal(e, jobDir);
              // A rerun after a crash: this file was moved by the run before.
              if (!(ctx.rerun && (err.details as { reason?: unknown })?.reason === "missing" && (await exists(to)))) throw err;
            }
            frames.push({ ...f, at, file: to });
          }
          return { ...ctx.result, frames };
        },
        catch: toFinError,
      }),
  };
}

/**
 * `job.done`'s `media` object (Task 14): the worker's account of the job, `stats.media` of its
 * result (present only when diagnostics were on at the worker). It is passed on as it came; the
 * journal decodes it against the registered `job.done` kind, which has `meta` fields only, so
 * nothing else can reach an entry.
 */
export function lookJournal(job: JobView): Readonly<Record<string, unknown>> {
  const media = (job.result as { stats?: { media?: unknown } } | null | undefined)?.stats?.media;
  return typeof media === "object" && media !== null && !Array.isArray(media) ? { media } : {};
}

/** The plan hashes `prepare` computed, in `at` order (`extra.planSamples`). */
/** Each existing path resolved (a missing one is left out: the worker reports it missing). */
async function canonicalReads(paths: ReadonlyArray<string>): Promise<string[]> {
  const out = new Set<string>();
  for (const p of paths) {
    if (!isAbsolute(p)) continue;
    try {
      out.add(await realpath(p));
    } catch {
      // Missing or unreadable: not granted.
    }
  }
  return [...out].sort();
}

function readsOf(extra: unknown): ReadonlyArray<string> {
  const reads = (extra as { reads?: unknown } | null | undefined)?.reads;
  return Array.isArray(reads) ? reads.filter((r): r is string => typeof r === "string") : [];
}

function planSamplesOf(extra: unknown): ReadonlyArray<string> {
  const samples = (extra as { planSamples?: ReadonlyArray<{ blake3?: unknown }> } | null | undefined)?.planSamples;
  if (!Array.isArray(samples)) throw finError("JOB_FAILED", "the job lost the plan samples its prepare computed", {});
  return samples.map((s) => String(s.blake3));
}

/**
 * Registers the family and its kinds (`media.look`, `media.capabilities`; `media.probeDeep` given
 * `projects.onBranch`; `media.proxy` and `media.thumbs` given that and `proxy`, 6B-1 Tasks 14 and 15;
 * `media.pcm` likewise, 6C Task 18).
 */
export function registerMediaKinds(registry: KindRegistry, deps: MediaKindDeps): void {
  registerMediaFamily(registry, deps);
  registry.registerKind(mediaLookKind(deps));
  registry.registerKind(mediaCapabilitiesKind());
  const { onBranch } = deps.projects;
  if (onBranch !== undefined) registry.registerKind(mediaProbeDeepKind({ ...deps, projects: { onBranch } }));
  if (onBranch !== undefined && deps.proxy !== undefined) registry.registerKind(mediaProxyKind({ ...deps, ...deps.proxy, projects: { onBranch } }));
  if (onBranch !== undefined && deps.proxy !== undefined) registry.registerKind(mediaThumbsKind({ ...deps, cache: deps.proxy.cache, projects: { onBranch } }));
  if (onBranch !== undefined && deps.proxy !== undefined) registry.registerKind(mediaPcmKind({ ...deps, cache: deps.proxy.cache, projects: { onBranch } }));
}
