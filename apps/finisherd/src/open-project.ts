// One project the registry has open (`projects.ts`), and the libraries it holds in use.
import type { Engine } from "@finisher/fin-node";
import type { Semaphore } from "effect";

/** One open project: its engine and a commit lock. */
export interface OpenProject {
  readonly engine: Engine;
  readonly dir: string;
  /** Serializes commit + publish so hubs see revs in commit order. */
  readonly commitLock: Semaphore.Semaphore;
  /** Releases the libraries its assets live in (5D Decision 28). */
  release?: () => void;
  /** Calls in flight on it, a commit or history write waiting for the lock among them. */
  active: number;
  /** When it was last used (`Date.now()`): a call ended or a subscriber left. */
  lastUsed: number;
  /** Set once the registry closed it (idle, `project.close`; not an eviction): a call that raced it reopens. */
  retired: boolean;
}

const MAIN = "main";

/** Holds the libraries the project's assets (on `main`, at open) live in, until it closes. */
export function holdLibraries(
  p: OpenProject,
  use: (assetPaths: ReadonlyArray<string>, projectId: string) => { release(): void },
): void {
  void p.engine.snapshot(MAIN).then(
    (json) => {
      const assets = (JSON.parse(json) as { project?: { assets?: Record<string, { path?: unknown }> } }).project?.assets ?? {};
      const held = use(Object.values(assets).flatMap((a) => (typeof a.path === "string" ? [a.path] : [])), p.engine.projectId);
      if (p.release === undefined) p.release = () => held.release();
      else held.release();
    },
    () => undefined,
  );
}

