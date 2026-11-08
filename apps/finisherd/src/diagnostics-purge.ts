// A diagnostics purge across the two D34 stores (sp7 §12.1; plan 7A Task 10 review H2). The
// agent-trace store goes first: it holds the content, so a purge that stops part-way never
// leaves content behind with its metadata gone. Both stores are always attempted, each once more
// when its first attempt fails; the answer reports each store, and fails (naming the store) when
// either still failed, so a caller never reads a partial purge as a whole one.
import type { PurgeReport } from "@finisher/core-types";
import type { FinError } from "@finisher/contracts";
import type { Journal } from "@finisher/fin-node";
import { Effect } from "effect";
import { attemptJson, finError } from "./errors.ts";

export type PurgeStore = "agentTraces" | "diagnostics";

/** One store's part: its report, or the error of its last attempt. */
export type StorePurge = PurgeReport | { readonly error: { readonly code: string; readonly message: string } };

/** The stores' reports summed, and each store's own. */
export interface PurgeOutcome extends PurgeReport {
  readonly stores: Readonly<Record<PurgeStore, StorePurge>>;
}

/** How often a store's purge is attempted before it counts as failed. */
export const PURGE_ATTEMPTS = 2;

const failed = (p: StorePurge): p is Extract<StorePurge, { error: unknown }> => "error" in p;

export function purgeStores(stores: Readonly<Record<PurgeStore, Pick<Journal, "purge">>>, selector: unknown): Effect.Effect<PurgeOutcome, FinError> {
  const sel = JSON.stringify(selector);
  const one = (j: Pick<Journal, "purge">) =>
    Effect.gen(function* () {
      let last: FinError | undefined;
      for (let i = 0; i < PURGE_ATTEMPTS; i++) {
        const r = yield* Effect.result(attemptJson<PurgeReport>(() => j.purge(sel)));
        if (r._tag === "Success") return r.success as StorePurge;
        last = r.failure;
      }
      return { error: { code: last!.code, message: last!.message } } as StorePurge;
    });
  return Effect.gen(function* () {
    const agentTraces = yield* one(stores.agentTraces);
    const diagnostics = yield* one(stores.diagnostics);
    const parts = { agentTraces, diagnostics };
    const bad = (Object.keys(parts) as PurgeStore[]).filter((k) => failed(parts[k]));
    if (bad.length > 0) {
      const first = parts[bad[0]!] as Extract<StorePurge, { error: unknown }>;
      return yield* Effect.fail(
        finError(first.error.code, `the purge of ${bad.join(" and ")} failed: ${first.error.message}`, { stores: parts, failed: bad }, "run the purge again"),
      );
    }
    const sum = (k: keyof PurgeReport) => (agentTraces as PurgeReport)[k] + (diagnostics as PurgeReport)[k];
    return { removed: sum("removed"), files: sum("files"), blobs: sum("blobs"), stores: parts };
  });
}
