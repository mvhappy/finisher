// A loopback HTTP server for generated fonts and a test catalog over it (no internet), for the
// "reel" e2e scenario: the JS twin of packages/fin-node/test/fontserver.ts.
import { createHash } from "node:crypto";
import { createServer } from "node:http";

/** A commit-shaped path segment, so test URLs pass the catalog's immutable-URL rule. */
export const COMMIT = "0123456789abcdef0123456789abcdef01234567";

export const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

/** Serves registered `/<COMMIT>/<name>` paths on 127.0.0.1 and counts the hits per path. */
export async function startFontServer() {
  const routes = new Map();
  const hits = new Map();
  const server = createServer((req, res) => {
    const path = req.url ?? "/";
    hits.set(path, (hits.get(path) ?? 0) + 1);
    const bytes = routes.get(path);
    if (!bytes) {
      res.writeHead(404).end();
      return;
    }
    res.writeHead(200, { "content-length": String(bytes.length), "content-type": "font/ttf" });
    res.end(Buffer.from(bytes));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  return {
    port,
    /** Serves `bytes` at `name`; returns its URL. */
    serve(name, bytes) {
      routes.set(`/${COMMIT}/${name}`, bytes);
      return `http://127.0.0.1:${port}/${COMMIT}/${name}`;
    },
    hitsOf: (name) => hits.get(`/${COMMIT}/${name}`) ?? 0,
    close() {
      server.closeAllConnections();
      return new Promise((resolve) => server.close(() => resolve()));
    },
  };
}

/**
 * A test catalog (OFL-1.1) of one entry per file `{id, family, weight, scripts, bytes, url}`,
 * each with one style (`regular` at 400, else `bold`); `fallback` lists entry ids.
 */
export function catalogJson(version, files, fallback) {
  const entries = files.map((f) => ({
    id: f.id,
    family: f.family,
    licence: { spdx: "OFL-1.1", url: `https://example.com/${COMMIT}/OFL.txt` },
    homepage: "https://example.com/",
    scripts: f.scripts,
    styles: [
      {
        id: f.weight === 400 ? "regular" : "bold",
        weight: f.weight,
        italic: false,
        file: {
          sha256: sha256(f.bytes),
          size: f.bytes.length,
          format: "ttf",
          faces: [{ index: 0, family: f.family, weight: f.weight, italic: false, scripts: f.scripts }],
        },
        sources: [{ url: f.url }],
      },
    ],
  }));
  return JSON.stringify({ catalogVersion: version, fallback, entries });
}
