// The library service over a real engine (plan 5D Task 10): `libraryWiring` as `main.ts` uses
// it, the test-hooks `fin-workers`, real catalog threads and real temp roots (overlays slow or
// break them). As in `main.ts`, the wiring (and its registry) exists before the engine and the
// service after it; the rig opens its own control store on the engine's database for that.
import { mkdirSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Audit } from "../../../src/control/audit.ts";
import { type ControlStore, openControlStore } from "../../../src/control/db.ts";
import { type Catalogs, startCatalogThreads } from "../../../src/library/catalog/client.ts";
import type { KindRegistry } from "../../../src/jobs/kinds.ts";
import { type LibraryWiring, type LibraryWiringOptions, libraryWiring } from "../../../src/library/layer.ts";
import type { LibraryService } from "../../../src/library/service.ts";
import { type TestEngine, testEngine } from "../../support/jobs.ts";
import { FAST } from "./kinds.ts";

export interface LiveRig {
  readonly t: TestEngine;
  readonly catalogs: Catalogs;
  readonly wiring: LibraryWiring;
  readonly svc: LibraryService;
  readonly control: ControlStore;
  readonly serverData: string;
  readonly journal: Array<{ kind: string; payload: Record<string, unknown> }>;
  /** Stops everything as if `finisherd` died, keeping the data (a restart reuses `serverData`). */
  crash(): Promise<void>;
  close(): Promise<void>;
}

const rigs: LiveRig[] = [];
const dirs: string[] = [];

export function liveRoots(): string {
  const d = realpathSync(mkdtempSync(join(tmpdir(), "fin-libsvc-roots-")));
  dirs.push(d);
  return d;
}

export interface LiveRigOptions {
  readonly watchQuietMs?: number;
  /** More kinds (the stage tests' `test.stage`). */
  readonly register?: (kinds: KindRegistry) => void;
  /** The service's clock and timers (the engine keeps real time). */
  readonly service?: NonNullable<LibraryWiringOptions["service"]>;
  /** The audit the registry writes `library.*` rows to. */
  readonly audit?: Audit;
  /** Reuse a server data folder (a restart). */
  readonly serverData?: string;
}

export async function liveRig(opts: LiveRigOptions = {}): Promise<LiveRig> {
  const serverData = opts.serverData ?? realpathSync(mkdtempSync(join(tmpdir(), "fin-libsvc-data-")));
  if (opts.serverData === undefined) dirs.push(serverData);
  const jobsRoot = join(serverData, "jobs");
  mkdirSync(jobsRoot, { recursive: true, mode: 0o700 });
  const control = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
  const catalogs = await startCatalogThreads({ serverData, jobsRoot, testHooks: true, ...(opts.watchQuietMs === undefined ? {} : { watchQuietMs: opts.watchQuietMs }) });
  const journal: LiveRig["journal"] = [];
  const wiring = libraryWiring({
    serverData,
    jobsRoot,
    control,
    catalogs,
    ...(opts.audit === undefined ? {} : { audit: opts.audit }),
    journal: { enabled: true, record: (kind, _kv, _ctx, payload) => void journal.push({ kind, payload: JSON.parse(payload) as Record<string, unknown> }) },
    service: { timing: FAST, ...opts.service },
  });
  let t: TestEngine;
  try {
    t = await testEngine({
      serverData,
      keepData: true,
      cores: 4,
      register: (k) => {
        wiring.register(k);
        opts.register?.(k);
      },
    });
  } catch (e) {
    await catalogs.shutdown();
    await control.close();
    throw e;
  }
  const svc = await wiring.start(t.engine);
  let closed = false;
  const rig: LiveRig = {
    t,
    catalogs,
    wiring,
    svc,
    control,
    serverData,
    journal,
    async crash() {
      if (closed) return;
      closed = true;
      svc.close();
      await t.crash();
      await catalogs.shutdown();
      await control.close();
    },
    async close() {
      if (closed) return;
      closed = true;
      svc.close();
      await t.close();
      await catalogs.shutdown();
      await control.close();
    },
  };
  rigs.push(rig);
  return rig;
}

export async function closeLiveRigs(): Promise<void> {
  for (const r of rigs.splice(0)) await r.close();
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
}

export { FAST };
