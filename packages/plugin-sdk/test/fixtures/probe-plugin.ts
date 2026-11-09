// A test plugin built with the SDK (bundled by the tests with tools/bundle.mjs): each command
// exercises one path of the runtime.
import { readFileSync } from "node:fs";
import { definePlugin, PluginError } from "../../src/index.ts";

const never = new Promise<never>(() => {});

definePlugin({
  commands: {
    echo: (args, host) => ({ data: { args, run: host.run, context: host.context, pid: process.pid } }),
    snapshot: async (_args, host) => ({ data: await host.snapshot() }),
    view: async (args, host) => ({ data: await host.view(args) }),
    fetch: async (args, host) => {
      const res = await host.fetch({ method: "GET", url: args.url }, args.grant === undefined ? {} : { grant: args.grant });
      return { data: { status: res.status, headers: res.headers, redirect: res.redirectNotFollowed, text: Buffer.from(res.body).toString("utf8") } };
    },
    log: (args, host) => {
      host.log("warn", args.msg);
      return undefined;
    },
    ops: () => ({ tx: [{ op: "marker.remove", marker: "mk1" }] }),
    "fail-plugin": () => {
      throw new PluginError("PROBE_FAILED", "the probe failed", { at: 1 }, "try again");
    },
    "fail-like": () => {
      throw { code: "PROBE_LIKE", message: "like a plugin error", details: { b: 2 } };
    },
    "fail-generic": () => {
      throw new TypeError("boom");
    },
    "fail-enoent": () => ({ data: readFileSync("/definitely/not/here/x", "utf8") }),
    "read-outside": (args) => ({ data: readFileSync(args.path, "utf8") }),
    "wait-cancel": async (_args, host) => {
      await new Promise((resolve) => host.signal.addEventListener("abort", resolve));
      return { data: { aborted: host.signal.aborted, reason: (host.signal.reason as { code?: string }).code } };
    },
    // Awaits a call the host never answers: the cancel fails it.
    "wait-call": async (_args, host) => ({ data: await host.view() }),
    "ignore-cancel": () => never,
    big: () => ({ data: "x".repeat(1 << 20) }),
    "bad-result": () => 42 as never,
  },
});
