// The server's routes (5B Decision 10): how a device can reach it, discovered at start and
// every 30 s (Node has no address-change event).
//
// - `loopback`: the local listener (always);
// - while the remote listener runs: `lan` (non-internal interfaces; link-local excluded),
//   `tailscale` (addresses in `100.64.0.0/10` and `fd7a:115c:a1e0::/48`), `public`
//   (`server.remote.publicUrl`); `connect` is the `ConnectRelay` extension point (none in v1).
// Each route is `{kind, url, pin}`: `identity` for the self-signed certificate made from the
// identity key, `webpki` for the owner's trusted certificate files. A change fires `onChange`
// once (the certificate is remade and the descriptor updated); nothing outside a route ever
// names an address (owner decision 4).
import { hostname as osHostname, networkInterfaces, type NetworkInterfaceInfo } from "node:os";
import type { Route, RouteKind } from "@finisher/contracts";

export interface RouteSource {
  interfaces(): NodeJS.Dict<NetworkInterfaceInfo[]>;
  hostname(): string;
}

export const OS_ROUTES: RouteSource = { interfaces: networkInterfaces, hostname: osHostname };

/** What the running remote listener contributes: its port and how its certificate is pinned. */
export interface RemoteInfo {
  readonly port: number;
  readonly pin: Route["pin"];
}

export interface RoutesOptions {
  /** The local listener's port (the `loopback` route). */
  readonly loopbackPort: () => number;
  readonly remote?: RemoteInfo | undefined;
  readonly publicUrl?: string | undefined;
}

/** Discovery period. */
export const ROUTES_POLL_MS = 30_000;

function ipv4Int(a: string): number | undefined {
  const parts = a.split(".").map(Number);
  if (parts.length !== 4 || parts.some((p) => !Number.isInteger(p) || p < 0 || p > 255)) return undefined;
  return ((parts[0]! << 24) | (parts[1]! << 16) | (parts[2]! << 8) | parts[3]!) >>> 0;
}

/** Whether `address` is a Tailscale address: `100.64.0.0/10` or `fd7a:115c:a1e0::/48`. */
export function isTailscale(address: string, family: "IPv4" | "IPv6"): boolean {
  if (family === "IPv4") {
    const n = ipv4Int(address);
    return n !== undefined && n >>> 22 === (100 << 2) + 1;
  }
  return /^fd7a:115c:a1e0:/i.test(address);
}

/** Whether `address` is an IPv6 link-local address (`fe80::/10`). */
export function isLinkLocal6(address: string): boolean {
  return /^fe[89ab][0-9a-f]:/i.test(address);
}

/** The URL host of an address: IPv6 in brackets. */
export const urlHost = (address: string, family: "IPv4" | "IPv6") => (family === "IPv6" ? `[${address}]` : address);

/** The routes `src` and `opts` give now, in a stable order. */
export function discover(src: RouteSource, opts: RoutesOptions): ReadonlyArray<Route> {
  const routes: Route[] = [{ kind: "loopback", url: `http://127.0.0.1:${opts.loopbackPort()}`, pin: "identity" }];
  const remote = opts.remote;
  if (remote === undefined) return routes;
  const found: Array<{ kind: RouteKind; url: string }> = [];
  for (const infos of Object.values(src.interfaces())) {
    for (const i of infos ?? []) {
      const family = i.family === "IPv6" || (i.family as unknown) === 6 ? "IPv6" : "IPv4";
      if (i.internal) continue;
      if (family === "IPv6" && isLinkLocal6(i.address)) continue;
      // IPv4 link-local (169.254.0.0/16, a host without DHCP) is not reachable from elsewhere either.
      if (family === "IPv4" && i.address.startsWith("169.254.")) continue;
      const kind: RouteKind = isTailscale(i.address, family) ? "tailscale" : "lan";
      found.push({ kind, url: `https://${urlHost(i.address, family)}:${remote.port}` });
    }
  }
  const order: Record<string, number> = { tailscale: 0, lan: 1 };
  found.sort((a, b) => order[a.kind]! - order[b.kind]! || (a.url < b.url ? -1 : a.url > b.url ? 1 : 0));
  const seen = new Set<string>();
  for (const f of found) {
    if (seen.has(f.url)) continue;
    seen.add(f.url);
    routes.push({ kind: f.kind, url: f.url, pin: remote.pin });
  }
  if (opts.publicUrl !== undefined) routes.push({ kind: "public", url: opts.publicUrl.replace(/\/+$/, ""), pin: remote.pin });
  return routes;
}

const same = (a: ReadonlyArray<Route>, b: ReadonlyArray<Route>) =>
  a.length === b.length && a.every((r, i) => r.kind === b[i]!.kind && r.url === b[i]!.url && r.pin === b[i]!.pin);

export class Routes {
  readonly #src: RouteSource;
  #opts: RoutesOptions;
  #current: ReadonlyArray<Route>;
  readonly #listeners = new Set<(r: ReadonlyArray<Route>) => void>();

  constructor(src: RouteSource, opts: RoutesOptions) {
    this.#src = src;
    this.#opts = opts;
    this.#current = discover(src, opts);
  }

  current(): ReadonlyArray<Route> {
    return this.#current;
  }

  /** The certificate's names: every route's host and this machine's host name (Decision 8). */
  sans(): ReadonlyArray<string> {
    const hosts = this.#current.filter((r) => r.kind !== "loopback" && r.kind !== "public").map((r) => new URL(r.url).hostname.replace(/^\[|\]$/g, ""));
    return [...new Set([...hosts, this.#src.hostname()])];
  }

  /** `f` runs after every change, once per change; the returned function removes it. */
  onChange(f: (r: ReadonlyArray<Route>) => void): () => void {
    this.#listeners.add(f);
    return () => void this.#listeners.delete(f);
  }

  /** The remote listener started, changed or stopped (`undefined`), or `publicUrl` changed. */
  reconfigure(opts: Partial<RoutesOptions>): void {
    this.#opts = { ...this.#opts, ...opts };
    this.poll();
  }

  /** Looks again; fires `onChange` when the routes differ. */
  poll(): void {
    const next = discover(this.#src, this.#opts);
    if (same(next, this.#current)) return;
    this.#current = next;
    for (const f of [...this.#listeners]) {
      try {
        f(next);
      } catch (e) {
        process.stderr.write(`finisherd: warning: a route listener failed: ${String(e)}\n`);
      }
    }
  }
}
