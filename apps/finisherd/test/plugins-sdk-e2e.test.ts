// The SDK example plugins end to end (sp8b Tasks 12 and 13 against Tasks 7-9 and 15): the Rust
// example (`examples/plugins/rust-tools`, a native binary on `fin-plugin-sdk`) and the TypeScript
// one (`examples/plugins/ts-tools`, bundled with `@finisher/plugin-sdk`) installed as code
// plugins and run through the real `plugin.command.run` path: the snapshot held at submit and
// read through `project.snapshot` pages, a transaction committed with `Tx.via`, `net.fetch` and
// a body of several `net.read` pages from a loopback HTTPS server standing in for the examples'
// fixed origin (`https://api.example.org`), and a plugin's own error code mapped to
// `PLUGIN_COMMAND_FAILED`.
import { execFileSync } from "node:child_process";
import { once } from "node:events";
import { chmodSync, copyFileSync, cpSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createServer, type Server } from "node:https";
import type { AddressInfo } from "node:net";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { CommittedTx } from "@finisher/core-types";
import { generateIdentityKey, selfSignedCert } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, describe, expect, test } from "vitest";
import { egressTestHooks } from "../src/plugins/host/egress-test-hooks.ts";
import { READ_BYTES } from "../src/plugins/host/egress-bodies.ts";
import { mapPluginError } from "../src/plugins/host/family.ts";
import { hostPlatform } from "../src/plugins/host/program.ts";
import { tx } from "./support/ndjson-client.ts";
import { type CommandRig, commandRig, errorOf, value } from "./support/plugin-command-rig.ts";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const EXAMPLES = join(ROOT, "examples", "plugins");
const API = "api.example.org";
/** The page `/` answers: a title and a body of several `net.read` pages. */
const TITLE = "Finisher SDK";
const PAGE = Buffer.concat([Buffer.from(`<html><head><title>  ${TITLE}  </title></head><body>`), Buffer.alloc(READ_BYTES * 2 + 1234, 0x61), Buffer.from("</body></html>")]);

const cleanups: Array<() => Promise<void> | void> = [];
afterEach(async () => {
  for (const c of cleanups.splice(0).reverse()) await c();
});

let server: Server;
let port = 0;
let ca = "";
let rustBinary = "";

beforeAll(async () => {
  // The Rust example's binary (cargo's own target directory) and the TypeScript example's bundle.
  execFileSync("cargo", ["build", "-q", "-p", "example-rust-tools", "--bin", "rust-tools"], { cwd: ROOT, stdio: "inherit" });
  const target = process.env.CARGO_TARGET_DIR ?? join(ROOT, "target");
  rustBinary = join(target, "debug", process.platform === "win32" ? "rust-tools.exe" : "rust-tools");
  const { build } = (await import(join(EXAMPLES, "ts-tools", "build.mjs"))) as { build(): string };
  build();
  const id = generateIdentityKey();
  ca = selfSignedCert(id.pem, [API], 1);
  server = createServer({ key: id.pem, cert: ca }, (req, res) => {
    if (req.url === "/") return void res.writeHead(200, { "content-type": "text/html", "content-length": String(PAGE.length) }).end(PAGE);
    res.writeHead(404).end("missing");
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  port = (server.address() as AddressInfo).port;
}, 600_000);

afterAll(() => {
  server?.closeAllConnections();
  server?.close();
});

/** The examples' origin reaches the loopback server: that name and address alone pass. */
const hooks = () =>
  egressTestHooks({
    lookup: async (h) => (h === API ? ["127.0.0.1"] : []),
    allow: [{ host: API, address: "127.0.0.1" }],
    ca,
    port: (origin) => (origin === `https://${API}` ? port : undefined),
  });

/**
 * A project of `main`: a video and an audio asset's clips on `t1` at 0, 1 and 2 s and on `t2` at
 * 1 s (1 s each), `t2` muted, and a `silence` span over 1-3 s.
 */
const SETUP = [
  (rev: number) =>
    tx("t-assets", rev, [
      { op: "asset.add", id: "$a", path: "/media/a.mov", duration: "600", video: { width: 1920, height: 1080, rate: "30" }, audio: { sampleRate: 48000, channels: 2 }, fingerprint: "fp" },
    ]),
  (rev: number) =>
    tx("t-clips", rev, [
      { op: "clip.insert", id: "$v0", track: "t1", at: "0", asset: "a1", sourceIn: "0", duration: "1" },
      { op: "clip.insert", id: "$v1", track: "t1", at: "1", asset: "a1", sourceIn: "0", duration: "1" },
      { op: "clip.insert", id: "$v2", track: "t1", at: "2", asset: "a1", sourceIn: "0", duration: "1" },
      { op: "clip.insert", id: "$s1", track: "t2", at: "1", asset: "a1", sourceIn: "0", duration: "1" },
      { op: "track.set", track: "t2", path: "mute", value: true },
      { op: "marker.add", id: "$m", timeline: "tl1", at: "1", duration: "2", label: "silence" },
    ]),
];

/** The example `name` installed with every permission its manifest asks for. */
async function example(name: "rust-tools" | "ts-tools"): Promise<CommandRig> {
  const src = join(EXAMPLES, name);
  const manifest = JSON.parse(readFileSync(join(src, "finisher-plugin.json"), "utf8")) as { id: string; version: string; permissions: object[]; runtime: { exec?: Record<string, string> } };
  return commandRig({
    id: manifest.id,
    version: manifest.version,
    grants: manifest.permissions,
    cleanups,
    egressHooks: hooks(),
    setup: SETUP,
    place: (dir) => {
      if (name === "rust-tools") {
        // One platform's package: this host's executable only (the validator wants every
        // platform the manifest names in the package).
        const platform = hostPlatform();
        const rel = manifest.runtime.exec![platform]!;
        writeFileSync(join(dir, "finisher-plugin.json"), JSON.stringify({ ...manifest, platforms: [platform], runtime: { type: "native", exec: { [platform]: rel } } }));
        cpSync(join(src, "i18n"), join(dir, "i18n"), { recursive: true });
        const exec = join(dir, ...rel.split("/"));
        mkdirSync(dirname(exec), { recursive: true });
        copyFileSync(rustBinary, exec);
        chmodSync(exec, 0o755);
      } else {
        cpSync(join(src, "finisher-plugin.json"), join(dir, "finisher-plugin.json"));
        cpSync(join(src, "messages"), join(dir, "messages"), { recursive: true });
        cpSync(join(src, "dist"), join(dir, "dist"), { recursive: true });
      }
    },
  });
}

/** `t1` and `t2`'s item starts on `main`. */
async function starts(r: CommandRig): Promise<{ t1: string[]; t2: string[] }> {
  const p = (await Effect.runPromise(r.projects.snapshot(r.projectId, "main"))).project as { timelines: Array<{ tracks: Array<{ id: string; items: Array<{ start: string }> }> }> };
  const of = (id: string) => p.timelines[0]!.tracks.find((t) => t.id === id)!.items.map((i) => i.start);
  return { t1: of("t1"), t2: of("t2") };
}

const runnable = (name: string) => {
  const m = JSON.parse(readFileSync(join(EXAMPLES, name, "finisher-plugin.json"), "utf8")) as { platforms: string[] };
  return m.platforms.includes(hostPlatform());
};

describe("SDK examples through the real plugin host", () => {
  test.skipIf(!runnable("rust-tools"))("rust_example_trims_silence_and_fetches_through_the_host", async () => {
    const r = await example("rust-tools");
    expect(await starts(r)).toEqual({ t1: ["0", "1", "2"], t2: ["1"] });
    const base = await r.head();
    // `trim-silence` reads the held snapshot and lifts the items inside the `silence` span.
    const done = value(await r.run("trim-silence", {}));
    expect(done).toMatchObject({ outcome: "accepted", rev: base + 1 });
    const got = (await Effect.runPromise(r.projects.txGet(r.projectId, done.txId!))) as CommittedTx;
    expect(got.tx.via).toEqual({ plugin: "@example/rust-tools@0.1.0", command: "trim-silence" });
    expect(got.tx.baseRev).toBe(base);
    expect(await starts(r)).toEqual({ t1: ["0"], t2: [] });
    expect(r.host.calls.heldSnapshots()).toBe(0);
    // A dry run commits nothing.
    expect(value(await r.run("trim-silence", { mode: "ripple" }, { dryRun: true }))).toMatchObject({ outcome: "data", dryRun: true, ops: [] });

    // `fetch-title`: `net.fetch`, then the body in three `net.read` pages.
    expect(value(await r.run("fetch-title", { path: "/" }))).toEqual({ outcome: "data", data: { status: 200, title: TITLE } });
    expect(value(await r.run("fetch-title", { path: "/nothing" }))).toEqual({ outcome: "data", data: { status: 404, title: null } });
    expect(r.host.egress.stats().get("@example/rust-tools")).toMatchObject({ completed: 2 });
  });

  test.skipIf(!runnable("ts-tools"))("ts_example_trims_silence_and_fetches_through_the_host", async () => {
    const r = await example("ts-tools");
    const base = await r.head();
    // `trim-silence` deletes the muted track's items (ripple).
    const done = value(await r.run("trim-silence", {}));
    expect(done).toMatchObject({ outcome: "accepted", rev: base + 1 });
    const got = (await Effect.runPromise(r.projects.txGet(r.projectId, done.txId!))) as CommittedTx;
    expect(got.tx.via).toEqual({ plugin: "@example/ts-tools@0.1.0", command: "trim-silence" });
    expect(await starts(r)).toEqual({ t1: ["0", "1", "2"], t2: [] });

    expect(value(await r.run("fetch-title", { url: `https://${API}/` }))).toEqual({ outcome: "data", data: { status: 200, title: TITLE } });
    // The example's own code (`FETCH_TITLE_HTTP`) is not the host's: `PLUGIN_COMMAND_FAILED`,
    // nothing of the plugin's kept.
    const e = errorOf(await r.run("fetch-title", { url: `https://${API}/nothing` }));
    expect(e).toMatchObject({ code: "PLUGIN_COMMAND_FAILED", details: { cause: "plugin" } });
    expect(JSON.stringify(e)).not.toContain("FETCH_TITLE_HTTP");
    expect(r.runs.at(-1)).toMatchObject({ command: "fetch-title", code: "PLUGIN_COMMAND_FAILED" });
  });

  test("sdk_failure_codes_survive_the_host_mapping", () => {
    // The SDKs' uncaught failure and job-file refusal are host codes: kept with their details.
    expect(mapPluginError({ code: "PLUGIN_COMMAND_FAILED", message: "disk full", details: { reason: "io" } })).toEqual({ code: "PLUGIN_COMMAND_FAILED", message: "disk full", details: { reason: "io" } });
    expect(mapPluginError({ code: "PLUGIN_OUTPUT_INVALID", message: "short", details: { reason: "size" } })).toMatchObject({ code: "PLUGIN_OUTPUT_INVALID", details: { reason: "size" } });
    expect(existsSync(join(EXAMPLES, "ts-tools", "dist", "plugin.mjs"))).toBe(true);
  });
});
