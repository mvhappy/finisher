// `tx.get`'s answer for a tx that is not on its branch (kept apart from `rpc.ts` for its size limit).
import { Schema } from "effect";

/**
 * A tx that is not (or no longer) on its branch: kept in quarantine. `reason`: `SUPERSEDED`,
 * `DIVERGED_TAIL`, `SALVAGED`, `MISSING_FROM_FOLDER`, `STALE_EPOCH`, `MALFORMED`,
 * `CONFLICT_COPY`; `origin`: `local`, `folder`, `foreign`, `salvage`.
 */
export const TxSetAsideSchema = Schema.Struct({
  setAside: Schema.Struct({
    txId: Schema.String,
    branch: Schema.String,
    reason: Schema.String,
    origin: Schema.String,
  }),
});
