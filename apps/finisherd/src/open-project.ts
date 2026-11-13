// One project the registry has open (`projects.ts`), and the libraries it holds in use.
import { realpathSync } from "node:fs";
import type { Engine } from "@finisher/fin-node";
import type { Semaphore } from "effect";
import { projectIdIn } from "./chain/resolve.ts";

/** One open project: its engine and a commit lock. */
export interface OpenProject {
  readonly engine: Engine;
  /** The registry's number for this engine, rising with each one registered (`project.events`' `source`). */
  readonly epoch: number;
  readonly dir: string;
  /** Serializes commit + publish so hubs see revs in commit order. */
  readonly commitLock: Semaphore.Semaphore;
  /** Releases the libraries its assets live in (5D Decision 28). */
  release?: () => void;
  /** Calls in flight on it, a commit or history write waiting for the lock among them. */
  active: number;
  /** When it was last used (`Date.now()`): a call ended or a subscriber left. */
  lastUsed: number;
  /** Set once the registry closed it (idle, `project.close`, moved; not an eviction): a call that raced it reopens. */
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


/** `dir` with every link resolved; `undefined` when it does not exist (any more). */
function realDir(dir: string): string | undefined {
  try {
    return realpathSync(dir);
  } catch {
    return undefined;
  }
}

/**
 * Whether the project open here at `oldDir` was moved to `dir` (review #32): another folder, and
 * the old one no longer holds the project (gone, or something else is there now). A link to the
 * same folder, or a copy whose original is still there, keeps the open engine.
 */
export async function movedFrom(oldDir: string, dir: string, projectId: string): Promise<boolean> {
  return realDir(oldDir) !== realDir(dir) && (await projectIdIn(oldDir)) !== projectId;
}
