// The stages sub-projects 6 and 8 register (plan 5D Decisions 39, 42): their specs, checked
// once at registration, and the `StageFile` each job is given. `probe` is built in (keyed by
// file, `library.probe`); every other stage is keyed by fingerprint and implicitly depends on
// `probe` (only fingerprinted files are pending).
//
// A library runs every registered stage unless its `stages` list names some, then only those
// (`library.set {stages}`).
import type { StageFile, StageSpec } from "./api.ts";
import type { StagePendingFile } from "./catalog/core/stage-rows.ts";
import { type LibraryRow, STAGE_NAME } from "./registry.ts";

export { STAGE_NAME };

export const PROBE_STAGE = "probe";
export const STAGE_BATCH_MAX = 256;
export const DEFAULT_MAX_IN_FLIGHT = 2;
/** A failed job's or item's row is retried this long after it failed (Decision 40)… */
export const RETRY_AFTER_MS = 60 * 60_000;
/** …this many attempts in all. */
export const MAX_ATTEMPTS = 3;

const invalid = (why: string): Error => new Error(`registerStage: ${why}`);

/** Every registered stage, by name. */
export class StageRegistry {
  readonly #specs = new Map<string, StageSpec>();

  /** Checks and registers `spec`; throws on an invalid spec, a duplicate or a dependency cycle. */
  register(spec: StageSpec): void {
    if (typeof spec.name !== "string" || !STAGE_NAME.test(spec.name)) throw invalid(`a stage name matches ${STAGE_NAME}`);
    if (spec.name === PROBE_STAGE) throw invalid("`probe` is built in");
    if (this.#specs.has(spec.name)) throw invalid(`a stage ${spec.name} is registered already`);
    if (typeof spec.version !== "string" || spec.version.length === 0 || spec.version.length > 256) throw invalid("a version is 1 to 256 characters");
    if (typeof spec.jobKind !== "string" || spec.jobKind.length === 0) throw invalid("jobKind names a batch kind");
    if (!Number.isSafeInteger(spec.batch) || spec.batch < 1 || spec.batch > STAGE_BATCH_MAX) throw invalid(`batch is 1 to ${STAGE_BATCH_MAX}`);
    if (spec.maxInFlight !== undefined && (!Number.isSafeInteger(spec.maxInFlight) || spec.maxInFlight < 1 || spec.maxInFlight > 64)) {
      throw invalid("maxInFlight is 1 to 64");
    }
    if (typeof spec.appliesTo !== "function" || typeof spec.params !== "function") throw invalid("appliesTo and params are functions");
    const deps = (spec.dependsOn ?? []).filter((d) => d !== PROBE_STAGE);
    if (deps.length > 16 || deps.some((d) => !STAGE_NAME.test(d) || d === spec.name)) throw invalid("dependsOn names up to 16 other stages");
    // A cycle through stages registered so far (a dependency registered later closes no cycle
    // with this one unless it names it, which its own registration then refuses).
    const seen = new Set<string>();
    const visit = (name: string): void => {
      if (name === spec.name) throw invalid(`a dependency cycle through ${spec.name}`);
      if (seen.has(name)) return;
      seen.add(name);
      for (const d of this.#specs.get(name)?.dependsOn ?? []) visit(d);
    };
    for (const d of deps) visit(d);
    this.#specs.set(spec.name, { ...spec, dependsOn: deps });
  }

  get(name: string): StageSpec | undefined {
    return this.#specs.get(name);
  }

  /** Stages in registration order. */
  list(): StageSpec[] {
    return [...this.#specs.values()];
  }

  /** The stages `lib` runs: all, or the ones its `stages` list names. */
  forLibrary(lib: Pick<LibraryRow, "stages">): StageSpec[] {
    const only = lib.stages.length === 0 ? undefined : new Set(lib.stages);
    return this.list().filter((s) => only === undefined || only.has(s.name));
  }

  /** `spec`'s dependencies with their versions; `undefined` when one is not registered (nothing is pending until it is). */
  dependencies(spec: StageSpec): Array<{ stage: string; version: string }> | undefined {
    const out: Array<{ stage: string; version: string }> = [];
    for (const d of spec.dependsOn ?? []) {
      const dep = this.#specs.get(d);
      if (dep === undefined) return undefined;
      out.push({ stage: d, version: dep.version });
    }
    return out;
  }
}

/** The `StageFile` of a pending row of `lib` (Decision 39): the raw chain and `Expect` to open it with. */
export function stageFile(lib: LibraryRow, f: StagePendingFile): StageFile {
  return {
    library: lib.id,
    fileId: f.fileId,
    root: lib.root,
    rel: f.rel,
    display: f.display,
    expect: {
      size: f.size,
      mtimeNs: f.mtimeNs,
      ...(f.fileIdBytes === null ? {} : { fileId: f.fileIdBytes }),
      ...(f.dev === null ? {} : { dev: f.dev }),
    },
    hydrate: lib.hydrate,
    fp: f.fp,
    size: f.size,
    ...(f.kind === null ? {} : { kind: f.kind }),
    ...(f.probe == null ? {} : { probe: f.probe }),
    storage: lib.storage,
  };
}
