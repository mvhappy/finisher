// Job kinds, worker families and helper programs (spec §6.1, 5C-1 plan Decisions 2, 5a,
// 20, 31–40). A family's crate registers its `FamilySpec`, its kinds' `KindSpec`s and its
// helpers' `HelperProgram`s here; the engine reads them. Params are validated by Rust
// (`validateJobParams`), never here.
import type { FinError, JobOwner, JobRights, JobView, JobVia, MappedFile, Priority, HelperSpec } from "@finisher/contracts";
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
  /** Checked by 5C-2. */
  readonly scopes: ReadonlyArray<string>;
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
  /** Bounded by `hookTimeoutS`; interrupted by a stop request (9b review M3). */
  readonly prepare?: (ctx: PrepareCtx<P>) => Effect.Effect<{ readonly extra?: unknown } | void, FinError>;
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
  readonly result: R;
  readonly items: ReadonlyMap<string, ItemOutcome>;
  readonly rerun: boolean;
}

/** An item's outcome for a kind's hooks; `error` is a `FinError` (a view's `itemErrors` hold its data, `FinErrorData`). */
export type ItemOutcome = { readonly value?: unknown } | { readonly error: FinError };

export type ProgramSpec =
  | { readonly binary: "fin-workers" | "fin-analyze"; readonly args: ReadonlyArray<string> }
  /** Plugins only (`Program::Path`). */
  | { readonly path: string; readonly args: ReadonlyArray<string> };

export interface CallCtx {
  readonly job: JobView;
  readonly role: "worker" | "helper";
  readonly helper?: { kind: string; id: string };
  readonly owner: JobOwner;
  readonly via?: JobVia;
  readonly rights?: JobRights;
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
  readonly program: ProgramSpec | ((job: JobView) => ProgramSpec);
  readonly memory?: { readonly interactive?: MemoryRule; readonly batch?: MemoryRule };
  readonly warm?: { readonly slots?: number; readonly idleS?: number; readonly maxJobs?: number; readonly maxAgeS?: number };
  /** Decision 35. */
  readonly warmKey?: (job: JobView) => string;
  /** Decision 5a. */
  readonly onCall?: OnCall;
  readonly diagKinds?: ReadonlyArray<string>;
  readonly pluginId?: string;
  /**
   * Names of variables copied from `finisherd`'s own environment into the family's processes,
   * beside the `childEnv()` allow-list (`media`: `FIN_GPU`, which picks the adapter). A fixed
   * list written in code; a variable that is not set is not passed.
   */
  readonly env?: ReadonlyArray<string>;
}

export interface HelperProgram {
  readonly kind: "codec" | "plugin";
  readonly id: string;
  readonly profile: "worker" | "plugin";
  resolve(job: JobView): ProgramSpec;
  readonly onCall?: OnCall;
  readonly pluginId?: string;
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
  program(family: string, job: JobView, mode: SpawnMode): ProgramSpec;
}

/** How a process is spawned: cold (one job, then exit) or warm (`ready`, then the next job). */
export type SpawnMode = "cold" | "warm";

/** The argument the engine appends to a warm spawn's argv. */
export const WARM_ARG = "--warm";

/** `program` with `--warm` appended when `mode` is warm. */
export function programFor(program: ProgramSpec, mode: SpawnMode): ProgramSpec {
  if (program.args.includes(WARM_ARG)) refuse(`a family program already holds ${WARM_ARG}; the engine appends it`);
  if (mode === "cold") return program;
  return "path" in program ? { path: program.path, args: [...program.args, WARM_ARG] } : { binary: program.binary, args: [...program.args, WARM_ARG] };
}

/** A value or a function of the params. */
export function resolveOrFn<P, T>(v: OrFn<P, T>, params: P): T {
  return typeof v === "function" ? (v as (params: P) => T)(params) : v;
}

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
      const slots = f.warm?.slots;
      if (slots !== undefined && (!Number.isInteger(slots) || slots < 1 || slots > 2)) refuse(`family ${f.family}: warm slots must be 1 or 2`);
      if (typeof f.program !== "function" && f.program.args.includes(WARM_ARG)) refuse(`family ${f.family}: the engine appends ${WARM_ARG}`);
      families.set(f.family, f);
    },
    registerKind(k) {
      if (!KIND.test(k.kind)) refuse(`kind name ${JSON.stringify(k.kind)} must match ${KIND}`);
      if (kinds.has(k.kind)) refuse(`kind ${k.kind} is already registered`);
      if (!families.has(k.family)) refuse(`kind ${k.kind}: unknown family ${k.family}`);
      if (!Number.isInteger(k.version) || k.version < 1) refuse(`kind ${k.kind}: version must be a positive integer`);
      if (!(k.timeoutS > 0)) refuse(`kind ${k.kind}: timeoutS must be positive`);
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
    program(name, job, mode) {
      const f = families.get(name) ?? refuse(`unknown family ${name}`);
      return programFor(typeof f.program === "function" ? f.program(job) : f.program, mode);
    },
  };
}
