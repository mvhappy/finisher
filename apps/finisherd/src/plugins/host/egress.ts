// The egress broker, `net.fetch` and `net.read` (sp8b plan Task 8, Decisions 10 and 11; spec
// §9.3). A plugin has no network of its own; this is the one way out, and its only network code
// is `node:https` (with `node:dns` for resolution). A request, in order:
// 1. names no secret grant: injection is 8G's, so `grant` answers `SECRET_MISSING {name}` and
//    nothing is sent (no header is ever added for a plugin);
// 2. is `https:` (`http:`, `ws:`, `wss:` and every other scheme: `EGRESS_NOT_HTTPS`) to exactly
//    one of the job's approved `net` origins (scheme, host, port; `EGRESS_DENIED {origin}`);
// 3. is within the plugin's rate (10 a second) and its in-flight cap (4 at once; 64 over all
//    plugins), else `EGRESS_RATE_LIMITED`; its body within 768,000 bytes (`EGRESS_TOO_LARGE`);
//    hop-by-hop and broker-owned headers, names that are not tokens and values with CR, LF or
//    NUL are refused;
// 4. is resolved by the host once per call (`dns.lookup(host, {all: true})`, at most 16
//    lookups at once so slow DNS cannot take libuv's pool), and every answer must be a globally
//    reachable unicast address (`egress-address.ts`; `EGRESS_PRIVATE_ADDRESS`, the address
//    named only in the server log);
// 5. connects to the checked address itself (the request's `lookup` returns it: no second
//    resolution, so rebinding cannot move a checked name; the next checked address when one
//    refuses the connection), with SNI and `Host` from the origin, through its own
//    `https.Agent` (no keep-alive, no proxy, no cookie jar, no credentials);
// 6. follows same-origin redirects (at most 3, reusing the checked addresses); a cross-origin
//    one (or a 4th) comes back as the 3xx with `redirectNotFollowed: true` and its `location`;
// 7. holds the response body (≤ 64 MiB) for `net.read` until the job ends (`egress-bodies.ts`:
//    bytes count as they arrive against the job's and the server's budgets; a job's end aborts
//    its requests); the whole call is bounded by 120 s (`EGRESS_TIMEOUT`).
import { lookup as dnsLookup } from "node:dns/promises";
import { Agent, request as httpsRequest } from "node:https";
import type { IncomingMessage } from "node:http";
import { isIPv6, type LookupFunction } from "node:net";
import { FinError } from "@finisher/contracts";
import type { Header, NetFetch, NetFetchReply, NetRead, NetReadPage } from "@finisher/core-types";
import { testHooks } from "@finisher/fin-node";
import { finError } from "../../errors.ts";
import { hostLiteral, privateReason } from "./egress-address.ts";
import { BODY_BYTES, JOB_HELD_BYTES, makeHeldBodies, READ_BYTES, SERVER_BYTES, tooLarge } from "./egress-bodies.ts";

export interface EgressLimits {
  /** Request body bytes (decoded), inline in the call. */
  readonly requestBytes: number;
  /** Response body bytes held for `net.read`. */
  readonly bodyBytes: number;
  /** The whole call, redirects included. */
  readonly timeoutMs: number;
  /** Requests a second per plugin. */
  readonly perSecond: number;
  /** Same-origin redirects followed. */
  readonly redirects: number;
  /** Requests of one plugin at once. */
  readonly inFlightPerPlugin: number;
  /** Requests of all plugins at once. */
  readonly inFlight: number;
  /** Name lookups at once, over all plugins (each holds a libuv pool thread). */
  readonly lookups: number;
  /** Body bytes held and in flight per job, and over every job. */
  readonly jobBytes: number;
  readonly serverBytes: number;
}

export const EGRESS_LIMITS: EgressLimits = {
  requestBytes: READ_BYTES, bodyBytes: BODY_BYTES, timeoutMs: 120_000, perSecond: 10, redirects: 3,
  inFlightPerPlugin: 4, inFlight: 64, lookups: 16, jobBytes: JOB_HELD_BYTES, serverBytes: SERVER_BYTES,
};

/** Connection failures after which the next checked address is tried. */
const CONNECT_CODES = new Set(["ECONNREFUSED", "EHOSTUNREACH", "ENETUNREACH", "EADDRNOTAVAIL", "ENETDOWN"]);
/** An HTTP header name (RFC 9110 token). */
const TOKEN = /^[!#$%&'*+.^_`|~0-9a-z-]+$/;

/**
 * Test-only changes to the broker (`egress-test-hooks.ts` makes them; accepted only by a
 * `test-hooks` build of fin-node, and `check-test-hooks` keeps that file out of `src/`).
 */
export interface EgressTestHooks {
  readonly testOnly: true;
  /** Answers in place of `dns.lookup`. */
  readonly lookup?: (host: string) => Promise<ReadonlyArray<string>>;
  /** `(name, address)` pairs let through the private-address check: only an address that name resolved to. */
  readonly allow?: ReadonlyArray<{ readonly host: string; readonly address: string }>;
  /** The CA of the loopback test server. */
  readonly ca?: string;
  readonly limits?: Partial<EgressLimits>;
}

/** What a call brings: its job (bodies belong to it), its plugin and its approved origins. */
export interface EgressCall {
  readonly jobId: string;
  /** `<id>@<version>` (the rate and the counters are per plugin id). */
  readonly plugin: string;
  readonly origins: ReadonlyArray<string>;
}

/** Per plugin counters, for `plugin.stats` (Task 15). */
export interface EgressCounters {
  requests: number;
  completed: number;
  denied: number;
  notHttps: number;
  privateAddress: number;
  secretMissing: number;
  rateLimited: number;
  tooLarge: number;
  timeouts: number;
  failed: number;
  redirectsFollowed: number;
  redirectsReturned: number;
  bytesOut: number;
  bytesIn: number;
}

const emptyCounters = (): EgressCounters => ({
  requests: 0, completed: 0, denied: 0, notHttps: 0, privateAddress: 0, secretMissing: 0, rateLimited: 0,
  tooLarge: 0, timeouts: 0, failed: 0, redirectsFollowed: 0, redirectsReturned: 0, bytesOut: 0, bytesIn: 0,
});

const COUNTER_OF: Readonly<Record<string, keyof EgressCounters>> = {
  EGRESS_DENIED: "denied", EGRESS_NOT_HTTPS: "notHttps", EGRESS_PRIVATE_ADDRESS: "privateAddress", SECRET_MISSING: "secretMissing",
  EGRESS_RATE_LIMITED: "rateLimited", EGRESS_TOO_LARGE: "tooLarge", EGRESS_TIMEOUT: "timeouts",
};

/** Headers the broker owns or that would change the connection; a plugin may not send them. */
const REFUSED_HEADERS = new Set(["host", "connection", "content-length", "transfer-encoding", "te", "trailer", "upgrade", "keep-alive", "expect", "proxy-authorization", "proxy-connection", "http2-settings"]);
const REDIRECTS = new Set([301, 302, 303, 307, 308]);
const BAD_VALUE = /[\r\n\0]/;
const B64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;

const pluginIdOf = (plugin: string): string => (plugin.lastIndexOf("@") > 0 ? plugin.slice(0, plugin.lastIndexOf("@")) : plugin);

function originOf(o: string): string | undefined {
  try {
    const u = new URL(o);
    return u.protocol === "https:" && u.pathname === "/" && u.search === "" && u.hash === "" && u.username === "" && u.password === "" ? u.origin : undefined;
  } catch {
    return undefined;
  }
}

const invalid = (reason: string, extra: Record<string, unknown> = {}, message = `net.fetch refused the request (${reason})`): FinError =>
  finError("INVALID_REQUEST", message, { method: "net.fetch", reason, ...extra });

interface Hop {
  readonly status: number;
  readonly headers: Header[];
  readonly body: Buffer;
  readonly location: string | undefined;
}

/** Where the broker reports refusals to the server log (scrubbed, by the plugin's keyed id). */
export type EgressLog = (plugin: string, message: string) => void;

export function makeEgress(opts: { readonly hooks?: EgressTestHooks; readonly now?: () => number; readonly log?: EgressLog } = {}) {
  const hooks = opts.hooks;
  if (hooks !== undefined && (testHooks === undefined || hooks.testOnly !== true)) {
    throw new Error("egress test hooks need a test-hooks build of fin-node");
  }
  const limits: EgressLimits = { ...EGRESS_LIMITS, ...hooks?.limits };
  const now = opts.now ?? Date.now;
  const bodies = makeHeldBodies({ jobBytes: limits.jobBytes, serverBytes: limits.serverBytes }, now);
  const counters = new Map<string, EgressCounters>();
  const recent = new Map<string, number[]>();
  const running = new Map<string, number>();
  let runningAll = 0;
  let lookingUp = 0;
  const log = opts.log ?? (() => undefined);
  const busy = (reason: string, limit: number): FinError =>
    finError("EGRESS_RATE_LIMITED", `too many requests at once (${reason})`, { reason, limit, retryable: true });
  const resolveName = hooks?.lookup ?? (async (host: string) => (await dnsLookup(host, { all: true, verbatim: true })).map((a) => a.address));

  const timedOut = (): FinError => finError("EGRESS_TIMEOUT", `the request took over ${limits.timeoutMs / 1000} s`, { limitS: limits.timeoutMs / 1000 });

  const countersOf = (plugin: string): EgressCounters => {
    let c = counters.get(plugin);
    if (c === undefined) {
      c = emptyCounters();
      counters.set(plugin, c);
    }
    return c;
  };

  /** Whether `plugin` may send one more request now (and counts it if so). */
  const admit = (plugin: string): number | undefined => {
    const t = now();
    const times = (recent.get(plugin) ?? []).filter((x) => t - x < 1000);
    if (times.length >= limits.perSecond) {
      recent.set(plugin, times);
      return 1000 - (t - times[0]!);
    }
    times.push(t);
    recent.set(plugin, times);
    return undefined;
  };

  /** The checked addresses of `hostname`: every answer public (or a hook's named exception). */
  const checked = async (hostname: string, origin: string, plugin: string): Promise<string[]> => {
    const literal = hostLiteral(hostname);
    let answers: ReadonlyArray<string>;
    if (literal !== undefined) {
      answers = [literal];
    } else {
      if (lookingUp >= limits.lookups) throw busy("lookups", limits.lookups);
      lookingUp++;
      try {
        answers = await resolveName(hostname);
      } catch (e) {
        throw finError("EGRESS_UNREACHABLE", `${origin} does not resolve`, { origin, reason: (e as { code?: string }).code ?? "lookup" });
      } finally {
        lookingUp--;
      }
    }
    if (answers.length === 0) throw finError("EGRESS_UNREACHABLE", `${origin} does not resolve`, { origin, reason: "noAddress" });
    for (const a of answers) {
      const why = privateReason(a);
      // The test hook lets one named loopback address through, never a literal.
      const allowed = literal === undefined && hooks?.allow?.some((x) => x.host === hostname && x.address === a) === true;
      if (why !== undefined && !allowed) {
        // The address itself stays in the server log: the plugin learns no internal address.
        log(plugin, `egress refused: ${origin} resolves to ${a} (${why})`);
        throw finError("EGRESS_PRIVATE_ADDRESS", `${origin} resolves to an address plugins may not reach`, { origin, reason: why });
      }
    }
    return [...answers];
  };

  const aborted = (signal: AbortSignal): FinError => (signal.reason instanceof FinError ? signal.reason : timedOut());

  /** One HTTP exchange with `address`; `charge` accounts each arriving chunk (an error stops it). */
  const once = (url: URL, method: string, headers: Record<string, string[]>, body: Buffer | undefined, address: string, signal: AbortSignal, charge: (n: number) => FinError | undefined): Promise<Hop> =>
    new Promise<Hop>((resolve, reject) => {
      const family = isIPv6(address) ? 6 : 4;
      const pinned: LookupFunction = (_host, options, cb) => {
        if ((options as { all?: boolean }).all === true) (cb as unknown as (e: null, a: Array<{ address: string; family: number }>) => void)(null, [{ address, family }]);
        else cb(null, address, family);
      };
      const literal = hostLiteral(url.hostname);
      const agent = new Agent({ keepAlive: false, maxSockets: 1 });
      const done = (f: () => void) => {
        agent.destroy();
        f();
      };
      let req;
      try {
        req = httpsRequest({
          agent,
          host: literal ?? url.hostname,
          port: url.port === "" ? 443 : Number(url.port),
          path: `${url.pathname}${url.search}`,
          method,
          headers: Object.assign(Object.create(null) as Record<string, string | string[]>, headers, { connection: "close" }, body === undefined ? {} : { "content-length": String(body.length) }),
          lookup: pinned,
          ...(literal === undefined ? { servername: url.hostname } : {}),
          ...(hooks?.ca === undefined ? {} : { ca: hooks.ca }),
          rejectUnauthorized: true,
          signal,
        });
      } catch (e) {
        agent.destroy();
        reject(invalid("header", { error: (e as { code?: string }).code ?? "invalid" }));
        return;
      }
      req.on("error", (e: Error & { code?: string }) =>
        done(() => {
          if (e instanceof FinError) reject(e);
          else if (signal.aborted) reject(aborted(signal));
          else reject(finError("EGRESS_UNREACHABLE", `${url.origin} could not be reached`, { origin: url.origin, reason: e.code ?? "connect" }));
        }),
      );
      req.on("response", (res: IncomingMessage) => {
        const declared = Number(res.headers["content-length"] ?? "0");
        if (declared > limits.bodyBytes) {
          req.destroy(tooLarge("response", limits.bodyBytes));
          return;
        }
        const chunks: Buffer[] = [];
        let size = 0;
        let over: FinError | undefined;
        res.on("data", (c: Buffer) => {
          if (over !== undefined) return;
          size += c.length;
          over = size > limits.bodyBytes ? tooLarge("response", limits.bodyBytes) : charge(c.length);
          if (over !== undefined) req.destroy(over);
          else chunks.push(c);
        });
        res.on("error", (e) => req.destroy(e));
        res.on("end", () =>
          done(() => {
            if (over !== undefined) return reject(over);
            const pairs: Header[] = [];
            for (let i = 0; i + 1 < res.rawHeaders.length; i += 2) pairs.push({ name: res.rawHeaders[i]!.toLowerCase(), value: res.rawHeaders[i + 1]! });
            const loc = res.headers.location;
            resolve({ status: res.statusCode ?? 0, headers: pairs, body: Buffer.concat(chunks), location: typeof loc === "string" ? loc : undefined });
          }),
        );
      });
      req.end(body);
    });

  const fetchAll = async (call: EgressCall, params: NetFetch, c: EgressCounters): Promise<NetFetchReply> => {
    const r = params.request;
    if (params.grant != null) {
      throw finError("SECRET_MISSING", `no secret named ${params.grant} is bound to this plugin (secret injection is not served before 8G)`, { name: params.grant });
    }
    let url: URL;
    try {
      url = new URL(r.url);
    } catch {
      throw invalid("url");
    }
    if (url.protocol !== "https:") throw finError("EGRESS_NOT_HTTPS", `plugins reach the network over HTTPS only, not ${url.protocol}`, { scheme: url.protocol.replace(/:$/, "") });
    if (url.username !== "" || url.password !== "") throw invalid("userinfo");
    const approved = new Set(call.origins.map(originOf).filter((o): o is string => o !== undefined));
    if (!approved.has(url.origin)) {
      log(call.plugin, `egress refused: ${url.origin} is not approved`);
      throw finError("EGRESS_DENIED", `${url.origin} is not an origin this plugin was approved to reach`, { origin: url.origin });
    }
    // No prototype: a header named `__proto__` (refused anyway) cannot reach one.
    const headers = Object.create(null) as Record<string, string[]>;
    for (const h of r.headers ?? []) {
      const name = h.name.toLowerCase();
      if (!TOKEN.test(name) || name === "__proto__" || REFUSED_HEADERS.has(name) || name.startsWith("proxy-")) throw invalid("header", { name: name.slice(0, 64) });
      if (BAD_VALUE.test(h.value)) throw invalid("header", { name, problem: "value" });
      (headers[name] ??= []).push(h.value);
    }
    let body: Buffer | undefined;
    if (r.bodyB64 != null) {
      if (!B64.test(r.bodyB64)) throw invalid("bodyB64");
      body = Buffer.from(r.bodyB64, "base64");
      if (body.length > limits.requestBytes) throw tooLarge("request", limits.requestBytes);
    }
    const plugin = pluginIdOf(call.plugin);
    const mine = running.get(plugin) ?? 0;
    if (mine >= limits.inFlightPerPlugin) throw busy("inFlight", limits.inFlightPerPlugin);
    if (runningAll >= limits.inFlight) throw busy("serverInFlight", limits.inFlight);
    const wait = admit(plugin);
    if (wait !== undefined) {
      throw finError("EGRESS_RATE_LIMITED", `this plugin sent ${limits.perSecond} requests in the last second`, { reason: "perSecond", limit: limits.perSecond, retryAfterMs: wait, retryable: true });
    }
    const ctl = new AbortController();
    const tracked = bodies.track(call.jobId, ctl);
    if (tracked !== undefined) throw tracked;
    c.requests++;
    running.set(plugin, mine + 1);
    runningAll++;
    const timer = setTimeout(() => ctl.abort(timedOut()), limits.timeoutMs);
    // Bytes of this call charged to its job and not yet held.
    let charged = 0;
    const charge = (n: number): FinError | undefined => {
      const e = bodies.charge(call.jobId, n);
      if (e === undefined) charged += n;
      return e;
    };
    try {
      // One resolution a call: a same-origin redirect has the same host and reuses it.
      const addresses = await Promise.race([
        checked(url.hostname, url.origin, call.plugin),
        new Promise<never>((_, reject) => {
          if (ctl.signal.aborted) reject(aborted(ctl.signal));
          ctl.signal.addEventListener("abort", () => reject(aborted(ctl.signal)), { once: true });
        }),
      ]);
      let method: string = r.method;
      for (let hops = 0; ; hops++) {
        c.bytesOut += body?.length ?? 0;
        let hop: Hop | undefined;
        for (let i = 0; hop === undefined; i++) {
          try {
            hop = await once(url, method, headers, body, addresses[i]!, ctl.signal, charge);
          } catch (e) {
            const reason = e instanceof FinError && e.code === "EGRESS_UNREACHABLE" ? (e.details as { reason?: string }).reason : undefined;
            if (i + 1 >= addresses.length || reason === undefined || !CONNECT_CODES.has(reason)) throw e;
          }
        }
        c.bytesIn += hop.body.length;
        let next: URL | undefined;
        if (REDIRECTS.has(hop.status) && hop.location !== undefined) {
          try {
            next = new URL(hop.location, url);
          } catch {
            next = undefined;
          }
        }
        if (next !== undefined && next.protocol === "https:" && next.origin === url.origin && hops < limits.redirects) {
          c.redirectsFollowed++;
          bodies.uncharge(call.jobId, hop.body.length);
          charged -= hop.body.length;
          if (hop.status === 303 || ((hop.status === 301 || hop.status === 302) && method === "POST")) {
            method = method === "HEAD" ? "HEAD" : "GET";
            body = undefined;
            delete headers["content-type"];
          }
          url = next;
          continue;
        }
        const unfollowed = REDIRECTS.has(hop.status) && hop.location !== undefined;
        if (unfollowed) c.redirectsReturned++;
        const handle = bodies.hold(call.jobId, hop.body);
        if (handle instanceof FinError) throw handle;
        charged -= hop.body.length;
        c.completed++;
        return { status: hop.status, headers: hop.headers, handle, length: hop.body.length, ...(unfollowed ? { redirectNotFollowed: true } : {}) };
      }
    } finally {
      clearTimeout(timer);
      if (charged > 0) bodies.uncharge(call.jobId, charged);
      bodies.untrack(call.jobId, ctl);
      running.set(plugin, (running.get(plugin) ?? 1) - 1);
      if (running.get(plugin) === 0) running.delete(plugin);
      runningAll--;
    }
  };

  return {
    limits,
    /** `net.fetch` for `call`: the response head, its body held under `handle`. */
    async fetch(call: EgressCall, params: NetFetch): Promise<NetFetchReply> {
      const c = countersOf(pluginIdOf(call.plugin));
      try {
        return await fetchAll(call, params, c);
      } catch (e) {
        const err = e instanceof FinError ? e : finError("EGRESS_UNREACHABLE", "the request failed", { reason: "internal" });
        const k = COUNTER_OF[err.code];
        if (k !== undefined) c[k]++;
        else c.failed++;
        throw err;
      }
    },
    /** `net.read`: a page of a body the call's job holds. */
    read(jobId: string, params: NetRead): NetReadPage {
      const page = bodies.read(jobId, params.handle, params.offset, params.length);
      if (page instanceof FinError) throw page;
      return page;
    },
    /** `jobId` ended: its bodies go. */
    release(jobId: string): void {
      bodies.release(jobId);
    },
    heldBytes: (): number => bodies.heldBytes(),
    /** Requests in flight now, over all plugins (tests). */
    inFlight: (): number => runningAll,
    /** A copy of each plugin's counters (by plugin id), for `plugin.stats`. */
    stats(): ReadonlyMap<string, Readonly<EgressCounters>> {
      return new Map([...counters].map(([k, v]) => [k, { ...v }]));
    },
  };
}

export type Egress = ReturnType<typeof makeEgress>;
