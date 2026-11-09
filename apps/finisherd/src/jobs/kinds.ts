// Job kinds, worker families and helper programs (spec §6.1, 5C-1 plan Decisions 2, 5a,
// 20, 31–40). A family's crate registers its `FamilySpec`, its kinds' `KindSpec`s and its
// helpers' `HelperProgram`s here; the engine reads them. Params are validated by Rust
// (`validateJobParams`), never here.
import { type FinError, type FinErrorData, type HelperSpec, type JobOwner, type JobRights, type JobView, type JobVia, type MappedFile, type Priority, SCOPES } from "@finisher/contracts";
import type { FileHandle } from "node:fs/promises";
import type { Effect } from "effect";

export type PoolName = "cpu" | "gpu" | "hwenc" | "net" | `io:${string}`;

/** A memory limit, resolved at dispatch: `fixedMiB`, else `min(capMiB, ramFraction × RAM)` (Decision 20). */
export interface MemoryRule {
  readonly fixedMiB?: number;
  readonly ramFraction?: number;
  readonly capMiB?: number;
}

type OrFn<P, T> = T | ((params: P) => T);

export interface KindSpec<P = unknown, R = unknown> {
  readonly kind: string;
  readonly version: number;
  readonly family: string;
  /** What submitting it takes beside `jobs.submit` (5C-2: the chain checks them on `jobs.submit`). */
  readonly scopes: ReadonlyArray<string>;
  /**
   * Clients may submit it with `jobs.submit` (5C-2 Decision 6); any other kind is
   * `JOB_KIND_UNKNOWN` there and reached only through its own RPC or in-process callers. A kind
   * that opts in must accept only params whose paths and project agree with the job's `project`:
   * the chain checks the caller's reach over that `project`, never the params, so the kind
   * declares `paramsReach` (the registry refuses `rpc` without it, and from a plugin family:
   * review M3; sub-project 8 decides how plugins' kinds are submitted).
   */
  readonly rpc?: boolean;
  /**
   * What is wrong with `params` for a job of `project` (paths outside its folder, another
   * project named inside them), or `[]`: called on every submit, before the job is stored, and
   * answered as `JOB_PARAMS_INVALID {errors}`. Required for an `rpc` kind.
   */
  readonly paramsReach?: (params: P, job: { readonly project?: string | undefined }) => ReadonlyArray<string>;
  /** A job of this kind names its `project` (`INVALID_REQUEST {field: project}` otherwise; review M3). */
  readonly requiresProject?: boolean;
  readonly resources: OrFn<P, ReadonlyArray<PoolName>>;
  readonly warm?: boolean;
  readonly defaultPriority: Priority;
  readonly qos: "normal" | "utility" | "background";
  readonly timeoutS: number;
  readonly stallS?: number;
  readonly maxAttempts?: number;
  readonly resumable?: boolean;
  readonly batch?: BatchSpec<P>;
  readonly dedupe?: (params: P) => string | undefined;
  readonly rememberPoison?: boolean;
  /** Asked before any POISON_ITEM verdict (Decision 31). */
  readonly resourceHealthy?: (job: JobView & { params: P }) => Effect.Effect<boolean>;
  readonly files?: OrFn<P, ReadonlyArray<MappedFile>>;
  readonly helpers?: OrFn<P, ReadonlyArray<HelperSpec>>;
  /** Per group (Decision 20). */
  readonly memory?: OrFn<P, MemoryRule>;
  /**
   * Bounded by `hookTimeoutS`; interrupted by a stop request (9b review M3). Runs until it
   * succeeded once for the job's directory, unless `prepareEachAttempt`.
   */
  readonly prepare?: (ctx: PrepareCtx<P>) => Effect.Effect<{ readonly extra?: unknown } | void, FinError>;
  /**
   * `prepare` runs before every attempt (isolation runs and restarts included), not only
   * until it first succeeded: for inputs that must be measured at the attempt's start (5D's
   * scans take the catalog's `attemptStart` and its known directories there). It must then
   * rewrite every file it writes (`open(name, "w")` truncates).
   */
  readonly prepareEachAttempt?: boolean;
  /**
   * The files and directories outside the job directory a cold worker reads, from the job's
   * params and what `prepare` returned as `extra` (persisted, so every attempt gets the same):
   * each becomes a `read` grant, which the sandbox spawner turns into the worker profile's
   * sources and caches (plan 7C Task 7). Canonical absolute paths.
   */
  readonly reads?: (extra: unknown, params: P) => ReadonlyArray<string>;
  /** At least once; must be idempotent (Decision 40). */
  readonly onSucceeded?: (ctx: SucceededCtx<P, R>) => Effect.Effect<R | void, FinError>;
  readonly hookTimeoutS?: number;
  /** Extra `job.done` fields. */
  readonly journal?: (job: JobView) => Readonly<Record<string, unknown>>;
}

export interface BatchSpec<P> {
  items(params: P): ReadonlyArray<string>;
  restrict(params: P, items: ReadonlyArray<string>): P;
  combine?(results: ReadonlyArray<unknown>, items: ReadonlyMap<string, ItemOutcome>): unknown;
}

export interface PrepareCtx<P> {
  readonly job: JobView & { params: P };
  /**
   * Opens one of the job's mapped files (9b review B1): `r` reads, `rw` reads and writes an
   * existing file, `w` creates or truncates it (0600, its parents made). The name must be a
   * mapped file or lie under a mapped directory; a link at any segment, a FIFO or other
   * special file and a hard-linked file are refused (`JOB_FAILED`) before any byte moves.
   * Hooks get no raw paths: a worker may have planted anything in its writable files.
   */
  open(name: string, how?: "r" | "w" | "rw"): Promise<FileHandle>;
  readonly attempt: number;
}

export interface SucceededCtx<P, R> extends PrepareCtx<P> {
  /** What `prepare` returned as `extra` (the host's own, persisted with the job). */
  readonly extra: unknown;
  readonly result: R;
  readonly items: ReadonlyMap<string, ItemOutcome>;
  readonly rerun: boolean;
}

/** An item's outcome for a kind's hooks; `error` is a `FinError` (a view's `itemErrors` hold its data, `FinErrorData`). */
export type ItemOutcome = { readonly value?: unknown } | { readonly error: FinError };

export type ProgramSpec =
  | { readonly binary: "fin-workers" | "fin-analyze"; readonly args: ReadonlyArray<string> }
  /** Plugins only (`Program::Path`). */
  | { readonly path: string; readonly args: ReadonlyArray<string>; readonly process?: ProcessSpec };

/**
 * How a plugin family's process is started, in place of the engine's own (sp8b Task 4): its
 * whole environment (the engine adds nothing, not even its `TMPDIR`), its `cwd`, its grants
 * (the engine adds only the job's mapped files and `reads` on a cold spawn, never its job or
 * warm directory) and the limits the job host applies whatever the spawner does.
 */
export interface ProcessSpec {
  readonly env: Readonly<Record<string, string>>;
  readonly cwd: string;
  readonly grants: ReadonlyArray<{ readonly path: string; readonly mode: "read" | "readWrite" }>;
  readonly limits?: { readonly processes?: number; readonly openFiles?: number };
}

export interface CallCtx {
  readonly job: JobView;
  readonly role: "worker" | "helper";
  readonly helper?: { kind: string; id: string };
  readonly owner: JobOwner;
  readonly via?: JobVia;
  readonly rights?: JobRights;
  /** The warm worker's key (`FamilySpec.warmKey`, `<project>/`-prefixed when confined); absent for a cold process. */
  readonly workerKey?: string;
}

/**
 * A host call (Decision 5a). `params` are exactly what the process sent: validating them is the
 * family's trust boundary (a plugin process is untrusted; 8B decodes each method's params
 * before acting on them, and checks `rights`).
 */
export type OnCall = (ctx: CallCtx, method: string, params: unknown) => Effect.Effect<unknown, FinError>;

export interface FamilySpec {
  readonly family: string;
  readonly profile: "worker" | "plugin";
  /**
   * The family's program for a cold spawn. Never ends in `--warm`: the engine decides whether
   * a process is warm and appends it (`KindRegistry.program`), so a family declares one argv.
   */
  readonly program: ProgramSpec | ((job: JobView, key?: string) => ProgramSpec);
  readonly memory?: { readonly interactive?: MemoryRule; readonly batch?: MemoryRule };
  readonly warm?: { readonly slots?: number; readonly idleS?: number; readonly maxJobs?: number; readonly maxAgeS?: number };
  /** Decision 35. */
  readonly warmKey?: (job: JobView) => string;
  /**
   * Whether `job` may still run at all, checked when a warm job is handed to a ready worker
   * (sp8b review: a plugin disabled, revoked or in a crash loop while its process is idle):
   * the error the job fails with, or `undefined`. The worker is retired and never sees the job.
   */
  readonly usable?: (job: JobView) => FinError | undefined;
  /**
   * A plugin family's check of a job's rights at submit (sp8b Task 6), after the engine's own
   * shape check: e.g. that their principal class is the owner's. The error refuses the submit.
   */
  readonly checkRights?: (owner: JobOwner, rights: JobRights) => FinError | undefined;
  /**
   * Under a sandbox (plan 7C Decision 19): the roots a warm worker spawned for `job` may read,
   * fixed at its start and shared by every job of its key (one project): canonical, outside
   * server data. A job whose `reads` fall outside them runs cold.
   */
  readonly warmReads?: (job: JobView) => ReadonlyArray<string>;
  /** Decision 5a. */
  readonly onCall?: OnCall;
  /**
   * The family's processes' stderr lines (sp8b Task 7: plugins scrub and cap theirs); the
   * engine's debug log otherwise. `job`: the job a line came during, `key`: a warm worker's.
   */
  readonly onStderr?: (line: string, from: { readonly job?: string; readonly key?: string }) => void;
  readonly diagKinds?: ReadonlyArray<string>;
  readonly pluginId?: string;
  /**
   * Names of variables copied from `finisherd`'s own environment into the family's processes,
   * beside the `childEnv()` allow-list (`media`: `FIN_GPU`, which picks the adapter). A fixed
   * list written in code; a variable that is not set is not passed.
   */
  readonly env?: ReadonlyArray<string>;
  /**
   * The code a failed job of this family ends with, from the engine's own (sp8b Task 4: the
   * `plugin` family names `PLUGIN_CRASHED`, `PLUGIN_TIMEOUT`, `PLUGIN_PROTOCOL`). Applied when
   * the job ends `failed`, after the retry policy read the engine's code.
   */
  readonly mapError?: (e: FinErrorData) => FinErrorData;
}

export interface HelperProgram {
  readonly kind: "codec" | "plugin";
  readonly id: string;
  readonly profile: "worker" | "plugin";
  resolve(job: JobView): ProgramSpec;
  readonly onCall?: OnCall;
  readonly pluginId?: string;
}

/** A process's stderr line: the family's `onStderr`, else the debug log under `tag`. */
export function familyStderr(
  f: FamilySpec,
  line: string,
  from: { readonly job?: string; readonly key?: string },
  log: (level: "debug" | "error", message: string) => void,
  tag: string,
): void {
  if (f.onStderr === undefined) return log("debug", `${tag} ${line}`);
  try {
    f.onStderr(line, from);
  } catch (e) {
    log("error", `${tag} the stderr hook failed: ${e instanceof Error ? e.message : String(e)}`);
  }
}

export interface KindRegistry {
  registerFamily(f: FamilySpec): void;
  registerKind<P, R>(k: KindSpec<P, R>): void;
  registerHelper(h: HelperProgram): void;
  kind(name: string): KindSpec | undefined;
  family(name: string): FamilySpec | undefined;
  /** The helper program of `(kind, id)`. */
  helper(kind: string, id: string): HelperProgram | undefined;
  /** Every registered kind. */
  kinds(): ReadonlyArray<KindSpec>;
  /**
   * The program to spawn for `job` of `family` (Decision 35, coordinator decision of the
   * batch A review): the family's program, with `--warm` appended for a warm spawn. This holds
   * for `Program::Path` plugin processes too. Throws for an unknown family.
   */
  program(family: string, job: JobView, mode: SpawnMode, key?: string): ProgramSpec;
}

/** A job's project: its `project`, else its params' `projectId` (`media.look`): what a confined warm key and its read roots name. */
export function projectOfJob(job: JobView): string | undefined {
  if (job.project !== undefined) return job.project;
  const p = (job.params as { projectId?: unknown } | null)?.projectId;
  return typeof p === "string" ? p : undefined;
}

/** How a process is spawned: cold (one job, then exit) or warm (`ready`, then the next job). */
export type SpawnMode = "cold" | "warm";

/** The argument the engine appends to a warm spawn's argv. */
export const WARM_ARG = "--warm";

/** `program` with `--warm` appended when `mode` is warm. */
export function programFor(program: ProgramSpec, mode: SpawnMode): ProgramSpec {
  if (program.args.includes(WARM_ARG)) refuse(`a family program already holds ${WARM_ARG}; the engine appends it`);
  if (mode === "cold") return program;
  return "path" in program ? { ...program, args: [...program.args, WARM_ARG] } : { binary: program.binary, args: [...program.args, WARM_ARG] };
}

/**
 * The plugin a `worker.exit` names (sp8b Decision 3b): the family's own, else the job's
 * `via.plugin` (`<id>@<version>`) without its version, so one family serves many plugins.
 */
export function exitPluginId(family: { readonly pluginId?: string | undefined }, via: JobVia | undefined): string | undefined {
  if (family.pluginId !== undefined) return family.pluginId;
  const p = via?.plugin;
  if (p === undefined) return undefined;
  const at = p.lastIndexOf("@");
  return at > 0 ? p.slice(0, at) : p;
}

/** A value or a function of the params. */
export function resolveOrFn<P, T>(v: OrFn<P, T>, params: P): T {
  return typeof v === "function" ? (v as (params: P) => T)(params) : v;
}

/** The warm slots a family of profile `plugin` may declare (sp8b Decision 3a); others 1 or 2. */
export const MAX_PLUGIN_WARM_SLOTS = 16;

/** The variables of `finisherd`'s environment a family may pass to its processes (`FamilySpec.env`). */
export const FAMILY_ENV: ReadonlySet<string> = new Set(["FIN_GPU"]);

const KNOWN_SCOPES: ReadonlySet<string> = new Set(SCOPES);
const FAMILY = /^[a-z][a-z0-9]{0,31}$/;
const KIND = /^[a-z][A-Za-z0-9]*\.[a-z][A-Za-z0-9]*$/;
const SEGMENT = /^[A-Za-z0-9._-]+$/;
export const MAX_FILE_BYTES = 1 << 30;
export const MAX_JOB_FILE_BYTES = 4 * (1 << 30);
const MAX_DEPTH = 4;

/** Names Windows reserves for devices, with or without an extension (`CON`, `nul.txt`). */
const WINDOWS_RESERVED = /^(con|prn|aux|nul|com[1-9¹²³]|lpt[1-9¹²³]|conin\$|conout\$)(\..*)?$/i;

/**
 * What is wrong with one mapped-file name (Decision 38), or `undefined`. Names must mean the
 * same file on every OS: no Windows device name, and no segment ending in a dot or a space
 * (Windows drops them).
 */
export function mappedFileNameError(name: string): string | undefined {
  const path = name.endsWith("/") ? name.slice(0, -1) : name;
  if (path === "") return "an empty name";
  if (path.startsWith("/") || /^[A-Za-z]:/.test(path) || path.includes("\\")) return "not a relative path";
  const segments = path.split("/");
  if (segments.length > MAX_DEPTH) return `deeper than ${MAX_DEPTH} segments`;
  for (const s of segments) {
    if (s === "." || s === "..") return `a "${s}" segment`;
    if (!SEGMENT.test(s)) return `a segment outside [A-Za-z0-9._-]: ${JSON.stringify(s)}`;
    if (s.endsWith(".") || s.endsWith(" ")) return `a segment ending in a dot or a space: ${JSON.stringify(s)}`;
    if (WINDOWS_RESERVED.test(s)) return `a Windows device name: ${JSON.stringify(s)}`;
  }
  return undefined;
}

/**
 * Every problem of a kind's file list (Decision 38): names, sizes, duplicates, names (or
 * directories on their paths) that differ only in case, a file that is also a directory on
 * another's path, and the per-job cap.
 */
export function validateMappedFiles(files: ReadonlyArray<MappedFile>): string[] {
  const errors: string[] = [];
  const seen = new Set<string>();
  /** Lower-cased path (file or directory) → how it was spelled, and whether it is a file. */
  const paths = new Map<string, { spelled: string; file: boolean }>();
  const note = (spelled: string, file: boolean, owner: string): void => {
    const key = spelled.toLowerCase();
    const known = paths.get(key);
    if (known === undefined) {
      paths.set(key, { spelled, file });
      return;
    }
    if (known.spelled !== spelled) errors.push(`${JSON.stringify(owner)}: ${JSON.stringify(spelled)} differs from ${JSON.stringify(known.spelled)} only in case`);
    else if (known.file !== file) errors.push(`${JSON.stringify(owner)}: ${JSON.stringify(spelled)} is both a file and a directory`);
  };
  let total = 0;
  for (const f of files) {
    const bad = mappedFileNameError(f.name);
    if (bad !== undefined) errors.push(`${JSON.stringify(f.name)}: ${bad}`);
    if (seen.has(f.name)) errors.push(`${JSON.stringify(f.name)}: listed twice`);
    else if (bad === undefined) {
      const isDir = f.name.endsWith("/");
      const segments = (isDir ? f.name.slice(0, -1) : f.name).split("/");
      segments.forEach((_, i) => note(segments.slice(0, i + 1).join("/"), !isDir && i === segments.length - 1, f.name));
    }
    seen.add(f.name);
    if (f.bytes !== undefined) {
      if (f.name.endsWith("/")) errors.push(`${JSON.stringify(f.name)}: a directory has no size`);
      else if (!Number.isSafeInteger(f.bytes) || f.bytes < 0) errors.push(`${JSON.stringify(f.name)}: a bad size`);
      else if (f.bytes > MAX_FILE_BYTES) errors.push(`${JSON.stringify(f.name)}: over 1 GiB`);
      else total += f.bytes;
    }
  }
  if (total > MAX_JOB_FILE_BYTES) errors.push("the files pass 4 GiB per job");
  return errors;
}

function refuse(what: string): never {
  throw new Error(`job registry: ${what}`);
}

/** A new, empty registry. Registering twice or out of order is a bug and throws. */
export function makeKindRegistry(): KindRegistry {
  const families = new Map<string, FamilySpec>();
  const kinds = new Map<string, KindSpec>();
  const helpers = new Map<string, HelperProgram>();
  return {
    registerFamily(f) {
      if (!FAMILY.test(f.family)) refuse(`family name ${JSON.stringify(f.family)} must match ${FAMILY}`);
      if (families.has(f.family)) refuse(`family ${f.family} is already registered`);
      // A plugin family's slots are its keys' processes (one per plugin, project and class), not
      // cpu: up to 16 (sp8b Decision 3a), so two keys do not thrash every other plugin's.
      const slots = f.warm?.slots;
      const most = f.profile === "plugin" ? MAX_PLUGIN_WARM_SLOTS : 2;
      if (slots !== undefined && (!Number.isInteger(slots) || slots < 1 || slots > most)) refuse(`family ${f.family}: warm slots must be ${most === 2 ? "1 or 2" : `1 to ${most}`}`);
      if (typeof f.program !== "function" && f.program.args.includes(WARM_ARG)) refuse(`family ${f.family}: the engine appends ${WARM_ARG}`);
      // `env` copies variables of `finisherd`'s own environment into the family's processes: a
      // list written in code for built-in families, never a plugin's (review L4).
      if (f.env !== undefined) {
        if (f.profile === "plugin" || f.pluginId !== undefined) refuse(`family ${f.family}: a plugin family cannot take variables from finisherd's environment`);
        const bad = f.env.filter((n) => !FAMILY_ENV.has(n));
        if (bad.length > 0) refuse(`family ${f.family}: env ${bad.join(", ")} is not one of ${[...FAMILY_ENV].join(", ")}`);
      }
      families.set(f.family, f);
    },
    registerKind(k) {
      if (!KIND.test(k.kind)) refuse(`kind name ${JSON.stringify(k.kind)} must match ${KIND}`);
      if (kinds.has(k.kind)) refuse(`kind ${k.kind} is already registered`);
      if (!families.has(k.family)) refuse(`kind ${k.kind}: unknown family ${k.family}`);
      if (!Number.isInteger(k.version) || k.version < 1) refuse(`kind ${k.kind}: version must be a positive integer`);
      if (!(k.timeoutS > 0)) refuse(`kind ${k.kind}: timeoutS must be positive`);
      // A scope outside the vocabulary is held by no principal: a typo that would lock the kind (review L2).
      const unknown = k.scopes.filter((sc) => !KNOWN_SCOPES.has(sc));
      if (unknown.length > 0) refuse(`kind ${k.kind}: unknown scopes ${unknown.join(", ")}`);
      if (k.rpc === true) {
        const f = families.get(k.family) as FamilySpec;
        if (f.profile === "plugin" || f.pluginId !== undefined) refuse(`kind ${k.kind}: a plugin family's kind cannot be submitted by RPC (sub-project 8)`);
        if (typeof k.paramsReach !== "function") refuse(`kind ${k.kind}: an rpc kind declares paramsReach`);
      }
      if (k.files !== undefined && typeof k.files !== "function") {
        const errors = validateMappedFiles(k.files);
        if (errors.length > 0) refuse(`kind ${k.kind}: ${errors.join("; ")}`);
      }
      kinds.set(k.kind, k as KindSpec);
    },
    registerHelper(h) {
      const key = `${h.kind}:${h.id}`;
      if (helpers.has(key)) refuse(`helper ${key} is already registered`);
      helpers.set(key, h);
    },
    kind: (name) => kinds.get(name),
    family: (name) => families.get(name),
    helper: (kind, id) => helpers.get(`${kind}:${id}`),
    kinds: () => [...kinds.values()],
    program(name, job, mode, key) {
      const f = families.get(name) ?? refuse(`unknown family ${name}`);
      return programFor(typeof f.program === "function" ? f.program(job, key) : f.program, mode);
    },
  };
}
