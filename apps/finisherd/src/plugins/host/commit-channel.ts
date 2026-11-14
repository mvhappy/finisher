// How the plugin host commits a command's transaction as the live invoker (sp8b Task 9): it
// calls the chain's own `tx.commit` with the invoker's connection, so every stage applies as
// to the invoker's own commit (its credential, scopes, branch limits, write guard, author, the
// agent hooks), and passes `via` (and a dry run) beside the payload under a symbol only this
// process can set. A client's request is decoded from the wire and can carry no symbol, and the
// chain still strips any `via` from the payload (`client-via.ts`).

/** The channel's key on a handler call's options (the chain's `optionsOf` copies it). */
export const HOST_COMMIT: unique symbol = Symbol("finisherd.pluginHostCommit");

export interface HostCommit {
  readonly via: { readonly plugin: string; readonly command: string };
  /** Check everything, commit nothing (`{dryRun: true}`). */
  readonly dryRun: boolean;
}

/** The plugin host's commit options of a `tx.commit` call, if it is one. */
export function hostCommitOf(options: object): HostCommit | undefined {
  return (options as { [HOST_COMMIT]?: HostCommit })[HOST_COMMIT];
}

/** `options` carrying `commit`. */
export function withHostCommit<O extends object>(options: O, commit: HostCommit): O & { readonly [HOST_COMMIT]: HostCommit } {
  return { ...options, [HOST_COMMIT]: commit };
}

/**
 * What `tx.commit` sends to the store for `tx`: a client's without any `via`; the plugin
 * host's with its `via`; or, for the host's dry run, nothing (`{dryRun: true}`).
 */
export function txToCommit(tx: unknown, options: object, stripVia: (tx: unknown) => unknown): { readonly dryRun: true } | { readonly tx: unknown } {
  const host = hostCommitOf(options);
  if (host?.dryRun === true) return { dryRun: true };
  const clean = stripVia(tx);
  if (host === undefined || clean === null || typeof clean !== "object" || Array.isArray(clean)) return { tx: clean };
  return { tx: { ...(clean as Record<string, unknown>), via: { plugin: host.via.plugin, command: host.via.command } } };
}
