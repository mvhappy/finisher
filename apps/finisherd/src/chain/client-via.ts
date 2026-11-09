// A client never sends `Tx.via` (spec §3.8): only the plugin host, which commits through the
// project registry and not through an RPC, attributes a transaction to a plugin.

/** `tx` without `via` (any value, even `null`); anything that is not an object is kept. */
export const withoutVia = (tx: unknown): unknown => {
  if (tx === null || typeof tx !== "object" || Array.isArray(tx) || !Object.hasOwn(tx, "via")) return tx;
  const { via: _via, ...rest } = tx as Record<string, unknown>;
  return rest;
};

/** A `tx.commit` payload whose `tx` has no `via`; every other call's payload as it is. */
export const withoutClientVia = (tag: string, payload: unknown): unknown => {
  if (tag !== "tx.commit" || payload === null || typeof payload !== "object" || Array.isArray(payload)) return payload;
  const p = payload as Record<string, unknown>;
  const tx = withoutVia(p.tx);
  return tx === p.tx ? p : { ...p, tx };
};
