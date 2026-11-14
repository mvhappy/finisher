// A loopback HTTP server for font files and test catalogs over it (no internet): the
// fin-node and finisherd font tests share it.
import { createHash } from "node:crypto";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";

/** A commit-shaped path segment, so test URLs pass the catalog's immutable-URL rule. */
export const COMMIT = "0123456789abcdef0123456789abcdef01234567";

export const sha256 = (b: Uint8Array): string => createHash("sha256").update(b).digest("hex");

/** What a route answers: the bytes, or a status; `chunkDelayMs` streams 16 KiB at a time. */
export interface Route {
  bytes?: Uint8Array;
  status?: number;
  chunkDelayMs?: number;
}

export class FontServer {
  readonly routes = new Map<string, Route>();
  readonly hits = new Map<string, number>();
  #server: Server;
  port = 0;

  private constructor(server: Server) {
    this.#server = server;
  }

  static async start(): Promise<FontServer> {
    let self!: FontServer;
    const server = createServer((req, res) => {
      const path = req.url ?? "/";
      self.hits.set(path, (self.hits.get(path) ?? 0) + 1);
      const route = self.routes.get(path);
      if (!route || route.status !== undefined || !route.bytes) {
        res.writeHead(route?.status ?? 404).end();
        return;
      }
      const bytes = route.bytes;
      res.writeHead(200, { "content-length": String(bytes.length), "content-type": "font/ttf" });
      if (!route.chunkDelayMs) {
        res.end(Buffer.from(bytes));
        return;
      }
      let at = 0;
      const step = () => {
        if (res.destroyed) return;
        const next = Math.min(at + 16 * 1024, bytes.length);
        res.write(Buffer.from(bytes.subarray(at, next)));
        at = next;
        if (at >= bytes.length) res.end();
        else setTimeout(step, route.chunkDelayMs);
      };
      step();
    });
    self = new FontServer(server);
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
    self.port = (server.address() as AddressInfo).port;
    return self;
  }

  url(name: string): string {
    return `http://127.0.0.1:${this.port}/${COMMIT}/${name}`;
  }

  /** Serves `bytes` at `name`; returns its URL. */
  serve(name: string, bytes: Uint8Array, extra: Omit<Route, "bytes"> = {}): string {
    this.routes.set(`/${COMMIT}/${name}`, { bytes, ...extra });
    return this.url(name);
  }

  hitsOf(name: string): number {
    return this.hits.get(`/${COMMIT}/${name}`) ?? 0;
  }

  close(): Promise<void> {
    this.#server.closeAllConnections();
    return new Promise((r) => this.#server.close(() => r()));
  }
}

/** One catalog file: entry `id`, style `regular`, family `Test <id>`, served at `url`. */
export interface CatalogFile {
  readonly id: string;
  readonly bytes: Uint8Array;
  readonly url: string;
  readonly weight?: number;
}

/** A test catalog (`catalogVersion` 2026.11.1, OFL-1.1) of one entry per file. */
export function catalogJson(files: ReadonlyArray<CatalogFile>, fallback: string[] = []): string {
  const entries = files.map((f) => {
    const weight = f.weight ?? 400;
    const family = `Test ${f.id}`;
    return {
      id: f.id,
      family,
      licence: { spdx: "OFL-1.1", url: `https://example.com/${COMMIT}/OFL.txt` },
      homepage: "https://example.com/",
      scripts: ["Latn"],
      styles: [
        {
          id: "regular",
          weight,
          italic: false,
          file: {
            sha256: sha256(f.bytes),
            size: f.bytes.length,
            format: "ttf",
            faces: [{ index: 0, family, weight, italic: false, scripts: ["Latn"] }],
          },
          sources: [{ url: f.url }],
        },
      ],
    };
  });
  return JSON.stringify({ catalogVersion: "2026.11.1", fallback, entries });
}

/** The model's `Fonts` pinning catalog file `f` (`Test <id>|400|normal`). */
export function catalogPin(f: CatalogFile): { key: string; fonts: string } {
  const weight = f.weight ?? 400;
  const key = `Test ${f.id}|${weight}|normal`;
  const pin = {
    family: `Test ${f.id}`,
    weight,
    italic: false,
    face: 0,
    sha256: sha256(f.bytes),
    size: f.bytes.length,
    format: "ttf",
    scripts: ["Latn"],
    source: { type: "catalog", entry: f.id, style: "regular", catalog: "2026.11.1" },
  };
  return { key, fonts: JSON.stringify({ pins: { [key]: pin }, fallback: [key] }) };
}

/** Polls `f` until it returns a value (not `undefined`) or `ms` pass. */
export async function waitFor<T>(f: () => Promise<T | undefined> | T | undefined, ms = 10_000, what = "condition"): Promise<T> {
  const deadline = Date.now() + ms;
  for (;;) {
    const v = await f();
    if (v !== undefined) return v;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 25));
  }
}
