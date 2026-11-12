// Helpers of the "plugin-code" e2e scenario: the example plugins staged as packages for this
// host, the test probe packaged as a registry plugin, the loopback HTTPS server the egress
// broker's test hooks route the examples' origin to, and process checks.
import { chmodSync, copyFileSync, cpSync, existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { createServer } from "node:https";
import { dirname, join } from "node:path";
import { pathToFileURL } from "node:url";
import { ROOT, TARGET_DIR, WIN } from "./lib.mjs";
import { TEST_NATIVE_DIR } from "./fixture.mjs";

export const EXAMPLES = join(ROOT, "examples", "plugins");
/** The examples' approved `net` origin's host. */
export const API = "api.example.org";
export const TITLE = "Finisher e2e";
export const PLATFORM = `${process.platform}-${process.arch}`;
const PROBE = join(ROOT, "apps", "finisherd", "test", "fixtures", "plugins", "probe");

const readJson = (p) => JSON.parse(readFileSync(p, "utf8"));

/** Whether both examples list this host's platform (darwin-x64 and win32-arm64 are not). */
export const examplesRunHere = () => ["rust-tools", "ts-tools"].every((n) => readJson(join(EXAMPLES, n, "finisher-plugin.json")).platforms.includes(PLATFORM));

/**
 * The examples as package folders under `work`: the TypeScript one bundled (`build.mjs`), the
 * Rust one with this host's release binary only (the validator wants every platform a manifest
 * names inside its package). Returns `{ts, rust}`: `{src, id}` each.
 */
export async function stageExamples(work) {
  const { build } = await import(pathToFileURL(join(EXAMPLES, "ts-tools", "build.mjs")).href);
  build();
  const ts = join(work, "ts-tools");
  for (const f of ["finisher-plugin.json", "messages", "dist"]) cpSync(join(EXAMPLES, "ts-tools", f), join(ts, f), { recursive: true });
  const rust = join(work, "rust-tools");
  const manifest = readJson(join(EXAMPLES, "rust-tools", "finisher-plugin.json"));
  const rel = manifest.runtime.exec[PLATFORM];
  mkdirSync(rust, { recursive: true });
  writeFileSync(join(rust, "finisher-plugin.json"), JSON.stringify({ ...manifest, platforms: [PLATFORM], runtime: { type: "native", exec: { [PLATFORM]: rel } } }, null, 2));
  cpSync(join(EXAMPLES, "rust-tools", "i18n"), join(rust, "i18n"), { recursive: true });
  const exec = join(rust, ...rel.split("/"));
  mkdirSync(dirname(exec), { recursive: true });
  copyFileSync(join(TARGET_DIR, "release", WIN ? "rust-tools.exe" : "rust-tools"), exec);
  if (!WIN) chmodSync(exec, 0o755);
  return { ts: { src: ts, id: readJson(join(ts, "finisher-plugin.json")).id }, rust: { src: rust, id: manifest.id } };
}

/** `@e2e/probe`: the test-only probe (`marker`, `command` ops) as a registry code plugin with `pluginData`. */
export function stageProbe(work) {
  const dir = join(work, "probe");
  mkdirSync(join(dir, "messages"), { recursive: true });
  copyFileSync(join(PROBE, "main.mjs"), join(dir, "main.mjs"));
  const args = {
    type: "object",
    properties: {
      op: { type: "string", enum: ["marker", "command"] },
      set: { type: "string", pattern: "^[a-z0-9-]+$", maxLength: 64 },
      say: { type: "array", items: { type: "string", maxLength: 200 }, maxItems: 4 },
      sleepMs: { type: "integer", minimum: 0, maximum: 60000 },
    },
    required: ["op"],
    additionalProperties: false,
  };
  const manifest = {
    ...readJson(join(PROBE, "finisher-plugin.json")),
    id: "@e2e/probe",
    publisher: { name: "Finisher e2e" },
    permissions: [{ perm: "project:read" }, { perm: "fs", scope: "pluginData", access: "readWrite" }],
    messages: "messages",
    locales: ["en"],
    contributes: { commands: [{ name: "run", title: "cmd.run", args, timeoutS: 60 }] },
  };
  writeFileSync(join(dir, "finisher-plugin.json"), JSON.stringify(manifest, null, 2));
  writeFileSync(join(dir, "messages", "en.json"), JSON.stringify({ "cmd.run": "Run a probe op" }));
  return { src: dir, id: manifest.id };
}

/**
 * The loopback HTTPS server standing in for `https://api.example.org` (a certificate made for
 * this run by fin-node's test build, nothing committed): `/` is a page titled `TITLE`, `/slow`
 * never answers. `{port, ca, close}`.
 */
export async function startOrigin() {
  process.env.FIN_NODE_BINARY_DIR = TEST_NATIVE_DIR;
  const { generateIdentityKey, selfSignedCert } = await import(pathToFileURL(join(ROOT, "packages", "fin-node", "index.js")).href);
  const id = generateIdentityKey();
  const ca = selfSignedCert(id.pem, [API], 1);
  const page = Buffer.from(`<html><head><title> ${TITLE} </title></head><body>e2e</body></html>`);
  const held = new Set();
  const server = createServer({ key: id.pem, cert: ca }, (req, res) => {
    if (req.url === "/slow") return void held.add(res);
    if (req.url === "/") return void res.writeHead(200, { "content-type": "text/html", "content-length": String(page.length) }).end(page);
    res.writeHead(404).end("missing");
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return {
    port: server.address().port,
    ca,
    held: () => held.size,
    close: () =>
      new Promise((resolve) => {
        for (const r of held) r.destroy();
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}

/** Whether process `pid` is alive (signal 0; Windows: the same through libuv). */
export function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return e?.code === "EPERM";
  }
}

/** Every file under `dir` whose text is exactly `text` (marker files); none when `dir` is missing. */
export function filesHolding(dir, text) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) return filesHolding(p, text);
    return readFileSync(p, "utf8") === text ? [p] : [];
  });
}
