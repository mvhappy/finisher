// Helpers for the catalog tests (plan 5D Task 7a): scan entries from readable names, batch
// files in a job's `out/`, and catalog threads that are always shut down.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { DirDone, ScanBatch, ScanEntry } from "@finisher/core-types";
import { Effect } from "effect";
import { type Catalogs, type CatalogThreadsOptions, startCatalogThreads } from "../../../src/library/catalog/client.ts";
import { CatalogExtensionRegistry } from "../../../src/library/catalog/extensions.ts";
import { ulid } from "../../../src/control/ulid.ts";

export const NOTES_URL = new URL("../fixtures/notes-extension.ts", import.meta.url);

export const b64 = (s: string | Uint8Array): string => Buffer.from(typeof s === "string" ? Buffer.from(s, "utf8") : s).toString("base64");

/** A directory chain from readable (UTF-8) components. */
export const rel = (...parts: string[]) => ({ components: parts.map((p) => b64(p)) });

export interface EntryOpts {
  readonly type?: ScanEntry["entryType"];
  readonly size?: number;
  readonly mtimeNs?: string;
  readonly fileId?: string;
  readonly placeholder?: ScanEntry["placeholder"];
  readonly excluded?: string;
  readonly raw?: Uint8Array;
}

/** A scan entry under `dir` (readable components) named `name`, on a case-sensitive root. */
export function entry(dir: string[], name: string, o: EntryOpts = {}): ScanEntry {
  const nameNfc = name.normalize("NFC");
  return {
    dir: rel(...dir),
    name: o.raw === undefined ? b64(name) : b64(o.raw),
    nameNfc,
    nameLossy: false,
    relKey: [...dir.map((d) => d.normalize("NFC")), nameNfc].join("/"),
    entryType: o.type ?? "file",
    size: o.size ?? 100,
    mtimeNs: o.mtimeNs ?? "1700000000000000000",
    mountRoot: false,
    automount: false,
    ...(o.fileId === undefined ? {} : { fileId: o.fileId }),
    ...(o.placeholder === undefined ? {} : { placeholder: o.placeholder }),
    ...(o.excluded === undefined ? {} : { excluded: o.excluded }),
  };
}

export const dirEntry = (dir: string[], name: string, o: EntryOpts = {}): ScanEntry => entry(dir, name, { ...o, type: "dir", size: 0 });

export const done = (dir: string[], entries: number, o: Partial<DirDone> = {}): DirDone => ({
  dir: rel(...dir),
  complete: true,
  entries,
  mtimeNs: "1700000000000000001",
  ...o,
});

/** A scratch server data directory and a jobs root with one job's `out/` and `in/`. */
export interface Scratch {
  readonly root: string;
  readonly serverData: string;
  readonly jobsRoot: string;
  readonly outDir: string;
  readonly inDir: string;
  cleanup(): void;
}

export function scratch(): Scratch {
  const root = mkdtempSync(join(tmpdir(), "catalog-"));
  const serverData = join(root, "server");
  const jobsRoot = join(root, "jobs");
  const outDir = join(jobsRoot, "job1", "out");
  const inDir = join(jobsRoot, "job1", "in");
  for (const d of [serverData, outDir, inDir]) mkdirSync(d, { recursive: true });
  return { root, serverData, jobsRoot, outDir, inDir, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

let fileSeq = 0;

/** Writes `batches` as NDJSON into `outDir` and answers the path. */
export function writeBatch(outDir: string, batches: ReadonlyArray<ScanBatch>, name = `batch-${++fileSeq}.ndjson`): string {
  const path = join(outDir, name);
  writeFileSync(path, `${batches.map((b) => JSON.stringify(b)).join("\n")}\n`);
  return path;
}

/** A registry with `core` and the notes fixture. */
export function withNotes(): CatalogExtensionRegistry {
  const r = new CatalogExtensionRegistry();
  r.register({ name: "notes", module: NOTES_URL });
  return r;
}

const live: Catalogs[] = [];

export async function threads(s: Scratch, opts: Partial<CatalogThreadsOptions> = {}): Promise<Catalogs> {
  const c = await startCatalogThreads({ serverData: s.serverData, jobsRoot: s.jobsRoot, extensions: withNotes(), ...opts });
  live.push(c);
  return c;
}

export async function stopAll(): Promise<void> {
  await Promise.all(live.splice(0).map((c) => c.shutdown()));
}

export const run = <A, E>(e: Effect.Effect<A, E>): Promise<A> => Effect.runPromise(e);

/** The `FinError` code an effect fails with. */
export async function failCode<A, E>(e: Effect.Effect<A, E>): Promise<string> {
  const exit = await Effect.runPromise(Effect.flip(e));
  return (exit as { code: string }).code;
}

export const newLib = (): string => ulid();
