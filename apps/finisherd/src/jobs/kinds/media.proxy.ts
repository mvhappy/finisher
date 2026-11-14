// The `media.proxy` kind (6B-1 plan Task 14; sub-project 6 spec §7.1, §3.3): a proxy of one
// source by `fin-workers media`, cold, committed into the media cache.
// - A batch of one item, the fingerprint, so the `proxy540` library stage (5D, whose kinds are
//   batches of fingerprints) and a project's `media.proxy` share it. The source is a project
//   asset (`asset`: `prepare` writes the one-asset manifest and fails `ASSET_CHANGED` when the
//   fingerprint moved) or a library file (`file`: the worker opens it through the catalogue's
//   raw chain and `Expect`).
// - Priority `background`, `qos: background`; resources `["gpu", "hwenc"]` when the cached
//   capability report names a hardware encoder for the codec, else `["gpu", "cpu"]`
//   (`proxyResources`); memory by the source's display size (Decision 18).
// - Identity (`dedupe`): the cache key (Decision 11). `rememberPoison`: an item that crashes the
//   worker stays failed until its fingerprint or `RENDER_VERSION` changes, i.e. its key.
// - `onSucceeded`: the item's error fails the job with it (so a waiting caller sees the cause,
//   and a poisoned item fails the job `POISON_ITEM`, which `rememberPoison` keeps); otherwise the
//   entry `out/<key>/` is committed by `MediaCache.commit` (rename, then insert) for the project
//   that asked. Other projects with the same content reach it through `serve` on their lookup.
import { realpath } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import type { FinError } from "@finisher/contracts";
import type { ProxyParams } from "@finisher/core-types";
import type { MediaCache } from "@finisher/fin-node";
import { Effect } from "effect";
import { finError, toFinError } from "../../errors.ts";
import { mediaBatchMemory } from "../../media/memory.ts";
import type { ProjectRegistryShape } from "../../projects.ts";
import { jobDirs } from "../files.ts";
import { mkdirSafe } from "../fsafe.ts";
import type { KindSpec, PoolName } from "../kinds.ts";

export const MEDIA_PROXY = "media.proxy";

/** The longest a proxy may take: a long source on a slow machine, in software. */
export const PROXY_TIMEOUT_S = 12 * 3600;

export interface ProxyKindDeps {
  readonly serverData: string;
  readonly projects: Pick<ProjectRegistryShape, "onBranch">;
  /** The media cache (`undefined` when it did not open: the commit is then `CACHE_IO`). */
  readonly cache: () => MediaCache | undefined;
  /** The cached `media.capabilities` answer, when there is one. */
  readonly report: () => unknown;
  /** `<server-data>/jobs` (default: `join(serverData, "jobs")`). */
  readonly jobsRoot?: string;
}

interface EncoderRow {
  readonly codec?: unknown;
  readonly hw?: unknown;
}

interface CodecRow {
  readonly codec?: unknown;
  readonly encode?: unknown;
  readonly backends?: ReadonlyArray<unknown>;
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** The report's codec rows: v2 `codecs[]`, v1 `codecs.video[]`. */
function codecRows(report: unknown): ReadonlyArray<CodecRow> {
  if (!isObj(report)) return [];
  const c = report.codecs;
  if (Array.isArray(c)) return c.filter(isObj);
  if (isObj(c) && Array.isArray(c.video)) return c.video.filter(isObj);
  return [];
}

/**
 * Whether the report names a hardware encoder for `codec`: v2's `encoders[]` (each passed the
 * probe's test clip, Decision 19), v1's `encode: "hardware"`.
 */
export function hardwareEncoder(report: unknown, codec: string): boolean {
  const encoders = isObj(report) && Array.isArray(report.encoders) ? (report.encoders as ReadonlyArray<EncoderRow>) : undefined;
  if (encoders !== undefined) return encoders.some((e) => isObj(e) && e.codec === codec && e.hw === true);
  return codecRows(report).some((r) => r.codec === codec && r.encode === "hardware");
}

/** Decision 5: H.264 where a hardware encoder makes it, else AV1 (SVT-AV1). */
export function proxyCodec(report: unknown): "h264" | "av1" {
  return hardwareEncoder(report, "h264") ? "h264" : "av1";
}

/** The kind's pools: `hwenc` for a hardware encoder of the codec, else `cpu`; the GPU scales either way. */
export function proxyResources(report: unknown, codec: string): ReadonlyArray<PoolName> {
  return hardwareEncoder(report, codec) ? ["gpu", "hwenc"] : ["gpu", "cpu"];
}

/** The backends the report lists for decoding `codec`, in the order they are tried (v2: decode rows; v1: the row's list). */
export function decodersOf(report: unknown, codec: string): string[] {
  const out: string[] = [];
  for (const r of codecRows(report)) {
    if (r.codec !== codec) continue;
    for (const b of r.backends ?? []) {
      const name = typeof b === "string" ? b : isObj(b) && b.direction === "decode" && typeof b.name === "string" ? b.name : undefined;
      if (name !== undefined && !out.includes(name)) out.push(name);
    }
  }
  return out;
}

/** `["media.proxy", key]`: one job per cache entry (per owner, 5C-2 Decision 7). */
export function proxyDedupe(p: ProxyParams): string {
  return JSON.stringify([MEDIA_PROXY, p.key]);
}

const invalid = (field: string, why: string): FinError =>
  finError("JOB_PARAMS_INVALID", `params.${field}: ${why}`, { field }, "ask for a proxy through media.proxy or the proxy540 stage");

async function canonical(paths: ReadonlyArray<string>): Promise<string[]> {
  const out: string[] = [];
  for (const p of paths) {
    if (!isAbsolute(p)) continue;
    try {
      out.push(await realpath(p));
    } catch {
      // Missing or unreadable: not granted (the worker reports it).
    }
  }
  return out.sort();
}

function readsOf(extra: unknown): ReadonlyArray<string> {
  const reads = (extra as { reads?: unknown } | null | undefined)?.reads;
  return Array.isArray(reads) ? reads.filter((r): r is string => typeof r === "string") : [];
}

/**
 * The item's error, which fails the job, or `undefined` for its `ProxyResult`. A worker's error
 * carries its guidance in `details.fix` (fin-workers' `job_error`): it becomes the error's `fix`.
 */
function outcomeOf(items: ReadonlyMap<string, { readonly value?: unknown } | { readonly error: FinError }>, fp: string): FinError | undefined {
  const o = items.get(fp);
  if (o === undefined) return finError("JOB_FAILED", "the worker answered nothing for the proxy", { item: fp });
  if (!("error" in o)) return undefined;
  const e = o.error;
  const details = e.details as { fix?: unknown } | null | undefined;
  if (e.fix !== undefined || typeof details?.fix !== "string") return e;
  const { fix, ...rest } = details as { fix: string };
  return finError(e.code, e.message, rest, fix);
}

/** `media.proxy` v1. */
export function mediaProxyKind(deps: ProxyKindDeps): KindSpec<ProxyParams, unknown> {
  const root = deps.jobsRoot ?? join(deps.serverData, "jobs");
  return {
    kind: MEDIA_PROXY,
    version: 1,
    family: "media",
    scopes: ["project.read"],
    resources: (p) => proxyResources(deps.report(), p.codec),
    warm: false,
    defaultPriority: "background",
    qos: "background",
    timeoutS: PROXY_TIMEOUT_S,
    stallS: 600,
    maxAttempts: 2,
    batch: { items: (p) => [p.fingerprint], restrict: (p) => p },
    dedupe: proxyDedupe,
    rememberPoison: true,
    memory: (p) => mediaBatchMemory(p.display ?? undefined),
    files: (p) => [...(p.asset == null ? [] : [{ name: "in/manifest.json", mode: "read" as const }]), { name: "out/", mode: "write" as const }],
    prepare: (ctx) =>
      Effect.gen(function* () {
        const p = ctx.job.params;
        if (p.file != null) {
          const reads = yield* Effect.promise(() => canonical([p.file!.root]));
          return { extra: { reads } };
        }
        if (p.projectId == null) return yield* Effect.fail(invalid("projectId", "required for an asset"));
        if (p.asset == null) return yield* Effect.fail(invalid("asset", "required"));
        const dirs = jobDirs(root, ctx.job.id);
        yield* Effect.tryPromise({ try: () => mkdirSafe(dirs.job, "in"), catch: toFinError });
        const done = yield* deps.projects.onBranch<{ rev: number; fingerprint: string; sources: ReadonlyArray<string> }>(p.projectId, p.branch ?? "main", (engine, branch) =>
          engine.prepareAssetJob(branch, p.asset!, join(dirs.job, "in")),
        );
        if (done.fingerprint !== p.fingerprint) {
          return yield* Effect.fail(finError("ASSET_CHANGED", `asset ${p.asset} changed since the proxy was asked for`, { asset: p.asset, reason: "fingerprint" }, "ask for the proxy again"));
        }
        const reads = yield* Effect.promise(() => canonical(done.sources));
        return { extra: { rev: done.rev, reads } };
      }),
    reads: readsOf,
    onSucceeded: (ctx) =>
      Effect.gen(function* () {
        const p = ctx.job.params;
        const failed = outcomeOf(ctx.items, p.fingerprint);
        if (failed !== undefined) return yield* Effect.fail(failed);
        const cache = deps.cache();
        if (cache === undefined) {
          return yield* Effect.fail(finError("CACHE_IO", "the media cache is not open", { reason: "io" }, "check the volume that holds the server data, then restart finisherd"));
        }
        const out = join(jobDirs(root, ctx.job.id).job, "out");
        yield* Effect.tryPromise({ try: () => cache.commit(out, p.key, p.projectId == null ? [] : [p.projectId]), catch: toFinError });
        return ctx.result;
      }),
  };
}
