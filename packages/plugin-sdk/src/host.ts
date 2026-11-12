// Typed host calls of host API 1.1 (plan 8B Decision 2: `call {id, method, params}` →
// `reply`): `project.snapshot` (paged, joined here), `project.view`, `net.fetch` (+ `net.read`
// pages until `eof`, Decision 11) and `log`. A `Host` serves one job: its calls are refused
// once the job is cancelled or has ended, and `signal` fires on the host's `cancel`.
import type {
  CommandRun,
  HostMethod,
  HttpRequest,
  LogLevel,
  NetFetchReply,
  NetReadPage,
  PluginContext,
  Project,
  ProjectSnapshotPage,
  ProjectView,
  ProjectViewText,
} from "@finisher/core-types";
import { protocolError } from "./errors.ts";

/** Sends one call and resolves to its reply's `value` (rejects with a `PluginError`). */
export type CallFn = (method: HostMethod, params: unknown) => Promise<unknown>;

/**
 * The largest page this SDK asks for: 756 KiB (774,144 bytes), mirroring
 * `fin_plugin_api::protocol::PAGE_BYTES` (not in the generated types). Its base64 is 1,032,192
 * characters, which leaves room for the reply's own JSON inside one 1 MiB envelope line.
 */
export const PAGE_BYTES = 756 * 1024;
/** The longest response body this SDK reads (`net.fetch`'s declared `length`): 64 MiB. */
export const MAX_BODY_BYTES = 64 << 20;

/** A response read to its end: the body joined from `net.read` pages. */
export interface HttpResponse {
  readonly status: number;
  readonly headers: NetFetchReply["headers"];
  readonly body: Uint8Array;
  /** A cross-origin redirect the host returned unfollowed (`location` is in `headers`). */
  readonly redirectNotFollowed: boolean;
}

/** `fetch`'s options: `grant` names a secret grant (8G; in 8B it answers `SECRET_MISSING`). */
export interface FetchOptions {
  readonly grant?: string;
}

const b64 = (s: string): Buffer => Buffer.from(s, "base64");

export class Host {
  /** The job's context (`start.job.extra`): host API, plugin, grants, dirs, locale, limits. */
  readonly context: PluginContext;
  /** The command run: `command`, `args`, `project`, `branch` and the pinned `rev`. */
  readonly run: CommandRun;
  /**
   * Fires when the host cancels the job, when the process is shutting down, and also when the
   * job ends normally (its handler settled): work still holding it should stop then too.
   */
  readonly signal: AbortSignal;
  readonly #call: CallFn;

  constructor(context: PluginContext, run: CommandRun, signal: AbortSignal, call: CallFn) {
    this.context = context;
    this.run = run;
    this.signal = signal;
    this.#call = call;
  }

  /** The page size to ask for: the context's `pageBytes`, at most `PAGE_BYTES`. */
  get #pageBytes(): number {
    const p = this.context.limits.pageBytes;
    return Number.isInteger(p) && p > 0 ? Math.min(p, PAGE_BYTES) : PAGE_BYTES;
  }

  /**
   * The project at the job's pinned `rev` (`project:read`): every page fetched and joined.
   * `PLUGIN_PROTOCOL {reason, page, expected, got}` when a page names another index
   * (`snapshotPage`), the page count is 0 or changes (`snapshotPages`) or a page is of another
   * `rev` (`snapshotRev`).
   */
  async snapshot(): Promise<Project> {
    const first = (await this.#call("project.snapshot", {})) as ProjectSnapshotPage;
    const pages = first.pages;
    if (!Number.isInteger(pages) || pages < 1) throw protocolError("a snapshot has no pages", { reason: "snapshotPages", page: 0, expected: 1, got: pages });
    const parts: Buffer[] = [];
    for (let page = 0; page < pages; page++) {
      const p = page === 0 ? first : ((await this.#call("project.snapshot", { page })) as ProjectSnapshotPage);
      if (p.rev !== this.run.rev) throw protocolError("a snapshot page is of another revision", { reason: "snapshotRev", page, expected: this.run.rev, got: p.rev });
      if (p.page !== page) throw protocolError("a snapshot page is out of order", { reason: "snapshotPage", page, expected: page, got: p.page });
      if (p.pages !== pages) throw protocolError("a snapshot's page count changed", { reason: "snapshotPages", page, expected: pages, got: p.pages });
      parts.push(b64(p.bytesB64));
    }
    try {
      return JSON.parse(Buffer.concat(parts).toString("utf8")) as Project;
    } catch {
      throw protocolError("a snapshot is not JSON", { reason: "snapshotJson" });
    }
  }

  /** `fin view`'s text of the job's branch (`project:read`). */
  async view(opts: ProjectView = {}): Promise<string> {
    return ((await this.#call("project.view", opts)) as ProjectViewText).text;
  }

  /**
   * An HTTPS request through the host's broker (`net`, approved origins only): the head from
   * `net.fetch`, then the body in `net.read` pages until `eof`. `PLUGIN_PROTOCOL` when the
   * declared `length` passes `MAX_BODY_BYTES` (`bodyTooLarge`, nothing read), a page is longer
   * than asked or empty before `eof` (`bodyPage`), or the pages do not add up to `length`
   * (`bodyLength`).
   */
  async fetch(request: HttpRequest, opts: FetchOptions = {}): Promise<HttpResponse> {
    const head = (await this.#call("net.fetch", opts.grant === undefined ? { request } : { request, grant: opts.grant })) as NetFetchReply;
    if (!Number.isSafeInteger(head.length) || head.length < 0) throw protocolError("a response has no valid length", { reason: "bodyLength", length: head.length });
    if (head.length > MAX_BODY_BYTES) throw protocolError("a response body is larger than this SDK reads", { reason: "bodyTooLarge", length: head.length, max: MAX_BODY_BYTES });
    const parts: Buffer[] = [];
    let offset = 0;
    for (;;) {
      const length = Math.min(this.#pageBytes, Math.max(head.length - offset, 0)) || 1;
      const page = (await this.#call("net.read", { handle: head.handle, offset, length })) as NetReadPage;
      const bytes = b64(page.bytesB64);
      if (bytes.length > length) throw protocolError("a response page is longer than asked", { reason: "bodyPage", offset, length, got: bytes.length });
      parts.push(bytes);
      offset += bytes.length;
      if (offset > head.length) throw protocolError("a response body is longer than its length", { reason: "bodyLength", length: head.length, got: offset });
      if (page.eof) break;
      if (bytes.length === 0) throw protocolError("an empty response page before the end", { reason: "bodyPage", offset });
    }
    if (offset !== head.length) throw protocolError("a response body is shorter than its length", { reason: "bodyLength", length: head.length, got: offset });
    return { status: head.status, headers: head.headers, body: new Uint8Array(Buffer.concat(parts)), redirectNotFollowed: head.redirectNotFollowed === true };
  }

  /** A line in the server log (rate-limited and scrubbed by the host, never journaled). Fire and forget. */
  log(level: LogLevel, msg: string): void {
    this.#call("log", { level, msg }).catch(() => {});
  }
}
