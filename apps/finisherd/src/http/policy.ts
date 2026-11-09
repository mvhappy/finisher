// What a listener accepts before anything authenticates (5B Decisions 11, 12): its Host names
// (with its port), its Origins, and its pre-upgrade pool. The loopback listener keeps 5A's
// lists exactly (`loopbackPolicy`); the remote listener's are its routes' (Task 10).

/** `origin` as `new URL` normalises it (lower-cased, default port dropped), or `undefined`. */
export function normalOrigin(origin: string): string | undefined {
  try {
    const o = new URL(origin).origin;
    return o === "null" ? undefined : o;
  } catch {
    return undefined;
  }
}

/** The loopback names a local client may use for this server (Host and Origin). */
export function loopbackHosts(port: number): ReadonlyArray<string> {
  return [`127.0.0.1:${port}`, `localhost:${port}`, `[::1]:${port}`];
}

export interface ListenerPolicy {
  readonly name: "loopback" | "remote";
  /** `host:port`, lower case. */
  readonly hosts: () => ReadonlyArray<string>;
  /** Normalised origins (`normalOrigin`) a browser may call from. */
  readonly origins: () => ReadonlyArray<string>;
  /** Sockets that have not upgraded (or whose HTTP request has not ended), at most. */
  readonly maxUnauthenticated: number;
  /** Of those, at most this many from one remote address (the remote listener: 4). */
  readonly perAddress?: number;
}

export const RPC_PATH = "/rpc";

/** Pre-upgrade sockets per listener (5A's cap, Decision 12). */
export const MAX_UNAUTHENTICATED = 16;
/** Pre-upgrade sockets per remote address on the remote listener. */
export const PER_ADDRESS = 4;

/**
 * The loopback listener (5A): Host is one of the three loopback names with its port; Origin is
 * absent, one of those as `http://`, or a configured one (`settings.json` `server.origins`).
 */
export function loopbackPolicy(port: () => number, configured: () => ReadonlyArray<string>): ListenerPolicy {
  return {
    name: "loopback",
    hosts: () => loopbackHosts(port()),
    origins: () => [...loopbackHosts(port()).map((h) => `http://${h}`), ...configured().flatMap((o) => normalOrigin(o) ?? [])],
    maxUnauthenticated: MAX_UNAUTHENTICATED,
  };
}

/** Whether a request's `Host` header is one `policy` serves. */
export function hostAllowed(policy: ListenerPolicy, host: string | undefined): boolean {
  return host !== undefined && policy.hosts().includes(host.toLowerCase());
}

/** Whether a request's `Origin` (absent: not a browser) is one `policy` serves. */
export function originAllowed(policy: ListenerPolicy, origin: string | undefined): boolean {
  if (origin === undefined) return true;
  const got = normalOrigin(origin);
  return got !== undefined && policy.origins().includes(got);
}
