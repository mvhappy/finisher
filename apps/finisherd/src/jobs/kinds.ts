// Job kinds, worker families and helper programs (spec §6.1, 5C-1 plan Decisions 2, 5a,
// 20, 31–40). A family's crate registers its `FamilySpec`, its kinds' `KindSpec`s and its
// helpers' `HelperProgram`s here; the engine reads them. Params are validated by Rust
// (`validateJobParams`), never here.
import type { FinError, JobOwner, JobRights, JobView, JobVia, MappedFile, Priority, HelperSpec } from "@finisher/contracts";
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
  readonly dir: string;
  path(name: string): string;
  readonly attempt: number;
}

export interface SucceededCtx<P, R> extends PrepareCtx<P> {
  readonly result: R;
  readonly items: ReadonlyMap<string, ItemOutcome>;
  readonly rerun: boolean;
}

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

export type OnCall = (ctx: CallCtx, method: string, params: unknown) => Effect.Effect<unknown, FinError>;

export interface FamilySpec {
  readonly family: string;
  readonly profile: "worker" | "plugin";
  readonly program: ProgramSpec | ((job: JobView) => ProgramSpec);
  readonly memory?: { readonly interactive?: MemoryRule; readonly batch?: MemoryRule };
  readonly warm?: { readonly slots?: number; readonly idleS?: number; readonly maxJobs?: number; readonly maxAgeS?: number };
  /** Decision 35. */
  readonly warmKey?: (job: JobView) => string;
  /** Decision 5a. */
  readonly onCall?: OnCall;
  readonly diagKinds?: ReadonlyArray<string>;
  readonly pluginId?: string;
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

/** What is wrong with one mapped-file name (Decision 38), or `undefined`. */
export function mappedFileNameError(name: string): string | undefined {
  const path = name.endsWith("/") ? name.slice(0, -1) : name;
  if (path === "") return "an empty name";
  if (path.startsWith("/") || /^[A-Za-z]:/.test(path) || path.includes("\\")) return "not a relative path";
  const segments = path.split("/");
  if (segments.length > MAX_DEPTH) return `deeper than ${MAX_DEPTH} segments`;
  for (const s of segments) {
    if (s === "." || s === "..") return `a "${s}" segment`;
    if (!SEGMENT.test(s)) return `a segment outside [A-Za-z0-9._-]: ${JSON.stringify(s)}`;
  }
  return undefined;
}

/** Every problem of a kind's file list (Decision 38): names, sizes, duplicates and the per-job cap. */
export function validateMappedFiles(files: ReadonlyArray<MappedFile>): string[] {
  const errors: string[] = [];
  const seen = new Set<string>();
  let total = 0;
  for (const f of files) {
    const bad = mappedFileNameError(f.name);
    if (bad !== undefined) errors.push(`${JSON.stringify(f.name)}: ${bad}`);
    if (seen.has(f.name)) errors.push(`${JSON.stringify(f.name)}: listed twice`);
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
  };
}
