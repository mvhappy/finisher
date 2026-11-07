// 5D's own catalog schema as an extension module (plan 5D Decision 24): loaded by URL in the
// indexer and the reader thread like 8C's and 8E's, so the threads treat every extension
// alike. Frozen once merged: add a step to `migrations`, never edit one.
import type { ChainVersions, Migration } from "../../../control/migrations.ts";
import { catalogCore0001 } from "./migrations/0001.ts";
import { coreOps } from "./ops.ts";
import { coreQueries } from "./queries.ts";

export const migrations: ReadonlyArray<Migration> = [catalogCore0001];
export const versions: ChainVersions = { formatVersion: 1, minReaderVersion: 1 };
export const ops = coreOps;
export const queries = coreQueries;
