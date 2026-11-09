// The job engine's public shape (spec §6.0–6.5, 5C-1 plan "Frozen interfaces"): `JobEngine`,
// its Effect tag, what a submit carries, what `workers()` answers, and what the engine is
// built from. `engine.ts` re-exports them.
import type { FinError, JobOwner, JobView, JobVia, Priority } from "@finisher/contracts";
import type { JobHostCaps } from "@finisher/fin-node";
import { Context, type Effect, type Stream } from "effect";
import type { RunFork } from "./calls.ts";
import type { JobEventSink } from "./events.ts";
import type { EngineHost, ProcView } from "./host.ts";
import type { KindRegistry, PoolName } from "./kinds.ts";
import type { PoolView, Pools } from "./pools.ts";
import type { RunnerEnv } from "./runner.ts";
import type { JobQuery, JobStore } from "./store.ts";
import type { Timers } from "./timers.ts";
import type { JobEvent } from "./view.ts";
import type { WarmWorkerInfo } from "./warm.ts";

export interface SubmitSpec {
  readonly kind: string;
  readonly params: unknown;
  readonly priority?: Priority;
  readonly idemKey?: string;
  readonly project?: string;
  readonly library?: string;
  readonly parent?: string;
  readonly via?: JobVia;
  readonly rights?: unknown;
  readonly maxAttempts?: number;
}

export interface WorkersView {
  readonly pools: ReadonlyArray<PoolView>;
  /** Warm workers (Decision 36). */
  readonly warm: ReadonlyArray<WarmWorkerInfo>;
  readonly processes: ReadonlyArray<ProcView>;
  readonly stuck: ReadonlyArray<string>;
  readonly caps: JobHostCaps;
  /** `diag` messages dropped (diagnostics off, or a kind the family does not forward). */
  readonly diagDropped: number;
}

export interface JobEngine {
  readonly kinds: KindRegistry;
  submit(owner: JobOwner, spec: SubmitSpec): Effect.Effect<JobView, FinError>;
  get(id: string): Effect.Effect<JobView, FinError>;
  list(q: JobQuery): { readonly jobs: JobView[]; readonly next?: string };
  wait(ids: ReadonlyArray<string>, opts: { readonly any?: boolean; readonly timeoutMs: number }): Effect.Effect<ReadonlyArray<JobView>, FinError>;
  cancel(id: string): Effect.Effect<JobView, FinError>;
  pause(target: { readonly job: string } | { readonly all: true }): Effect.Effect<void, FinError>;
  resume(target: { readonly job: string } | { readonly all: true }): Effect.Effect<void, FinError>;
  warm(family: string): Effect.Effect<void, FinError>;
  /**
   * Retires `family`'s warm workers whose key `match`es (sp8b Task 5: a plugin disabled,
   * revoked or updated): a ready one gets `shutdown` and is killed after the grace, a busy one
   * goes after its job, a spawning one is killed. Only the engine's own processes; how many.
   */
  retireWarm(family: string, match: (key: string) => boolean): number;
  definePool(name: PoolName, size: number): void;
  workers(): WorkersView;
  events(): Stream.Stream<JobEvent>;
  /**
   * Every event as it is published, synchronously, nothing dropped (5C-2's `jobs.watch` keeps its
   * own bounded buffer); a throwing listener is logged. Returns the unsubscribe.
   */
  onEvent(listener: (e: JobEvent) => void): () => void;
  onDone(listener: (job: JobView) => void): () => void;
  close(): Effect.Effect<void>;
}

export class JobEngineTag extends Context.Service<JobEngineTag, JobEngine>()("finisherd/JobEngine") {}

/** What `makeJobEngine` is built from (`layer.ts` assembles it). */
export interface EngineDeps {
  readonly host: EngineHost;
  readonly store: JobStore;
  readonly registry: KindRegistry;
  readonly pools: Pools;
  readonly sink: JobEventSink;
  readonly root: string;
  readonly totalMem: number;
  readonly now: () => number;
  readonly run: RunFork;
  readonly timers: Timers;
  readonly log: RunnerEnv["log"];
  readonly diagnostics: () => boolean;
  readonly testHooks: boolean;
  /** The warm workers' held-files check runs (test-hooks builds with real processes). */
  readonly heldFilesCheck: boolean;
  readonly queueLimit: number;
  /** Unfinished jobs per limited owner (`access.ts` `ownerLimited`; default `JOB_OWNER_LIMIT`). */
  readonly ownerLimit?: number;
}
