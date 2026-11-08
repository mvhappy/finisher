// The stage framework's test kind and stages (plan 5D Task 11): `test.stage`, a batch kind of
// the `library` family (test-hooks `fin-workers`) whose items are fingerprints and whose
// worker opens each file only through `open_for_read` with the `StageFile` it is given.
import { Effect } from "effect";
import type { KindRegistry, KindSpec, PoolName } from "../../../src/jobs/kinds.ts";
import type { StageFile, StageSpec } from "../../../src/library/api.ts";

export interface TestStageKnobs {
  readonly delayMs?: number;
  readonly failItems?: Record<string, string>;
  readonly crashOn?: string[];
  readonly jobError?: string;
}

export interface TestStageParams extends TestStageKnobs {
  readonly library: string;
  readonly root: string;
  readonly hydrate: boolean;
  readonly pool: string;
  readonly items: string[];
  readonly files: Array<{ fp: string; fileId: number; rel: StageFile["rel"]; expect: StageFile["expect"] }>;
}

/** Registers `test.stage` (resource: the library's read pool; healthy unless `healthy()` says otherwise). */
export function registerTestStageKind(kinds: KindRegistry, healthy: () => boolean = () => true): void {
  const spec: KindSpec<TestStageParams> = {
    kind: "test.stage",
    version: 1,
    family: "library",
    scopes: ["library.scan"],
    resources: (p) => [p.pool as PoolName],
    defaultPriority: "background",
    qos: "utility",
    timeoutS: 600,
    stallS: 120,
    batch: { items: (p) => p.items, restrict: (p, items) => ({ ...p, items: [...items] }) },
    resourceHealthy: () => Effect.sync(healthy),
  };
  kinds.registerKind(spec);
}

/** Every `params` call of a test stage: the files it was given. */
export type Seen = StageFile[][];

/** A stage over `test.stage`; `knobs(files)` is read at each job's params. */
export function testStage(
  name: string,
  opts: Partial<Omit<StageSpec, "name" | "params" | "jobKind">> & { readonly knobs?: (files: ReadonlyArray<StageFile>) => TestStageKnobs; readonly seen?: Seen } = {},
): StageSpec {
  const { knobs, seen, ...rest } = opts;
  return {
    name,
    version: "v1",
    appliesTo: () => true,
    jobKind: "test.stage",
    batch: 16,
    ...rest,
    params: (files, ctx) => {
      seen?.push([...files]);
      const f0 = files[0]!;
      return {
        library: ctx.library,
        root: f0.root,
        hydrate: f0.hydrate,
        pool: ctx.pools.read,
        items: files.map((f) => f.fp),
        files: files.map((f) => ({ fp: f.fp, fileId: f.fileId, rel: f.rel, expect: f.expect })),
        ...knobs?.(files),
      } satisfies TestStageParams;
    },
  };
}
