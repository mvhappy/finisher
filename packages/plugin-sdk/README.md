# @finisher/plugin-sdk

The TypeScript SDK for Finisher code plugins that run on the `node` runtime (sub-project 8 spec §3 and §7.2; plan 8B Task 13). A plugin process is a worker of the host's `plugin` family. It speaks the `fin-jobs` envelope (NDJSON on stdin and stdout) and nothing else. The SDK implements the plugin side of that envelope, so a plugin only writes command handlers.

```ts
import { definePlugin, PluginError } from "@finisher/plugin-sdk";

definePlugin({
  commands: {
    "trim-silence": async (args, host) => {
      const project = await host.snapshot();          // the whole project at the job's rev
      const ops = /* fin-core ops built from project */ [];
      return { tx: ops };                              // proposed, the host validates and commits
    },
    "fetch-title": async (args, host) => {
      const res = await host.fetch({ method: "GET", url: args.url });
      if (res.status !== 200) throw new PluginError("FETCH_FAILED", `HTTP ${res.status}`, { status: res.status });
      return { data: { title: new TextDecoder().decode(res.body) } };
    },
  },
});
```

The `examples/plugins/ts-tools` plugin is a complete example: a manifest with two `commands` contributions, a message catalogue, and `src/plugin.ts`.

## The API

- `definePlugin({ commands })` starts the plugin on the process's stdio. Call it once, at the top level of the entry module. `commands` is keyed by the command's `name` in the manifest. `runPlugin(def, io, argv)` does the same over other streams.
- A handler gets `(args, host)`. The host has already checked `args` against the command's schema (`fin-plugin-api::schema_subset`, 8B Decision 13). A handler returns one of:
  - `{ tx: Op[] }`, sent as `CommandOut {kind: "tx", ops}`. Only a command whose manifest says `writes: true` should return it. The host commits it under the invoker with `baseRev` set to the job's `rev`.
  - `{ data: value }`, sent as `{kind: "data", value}`.
  - nothing, sent as `{kind: "data", value: null}`.
- `host.snapshot()` returns the project (`Project`) at the job's pinned `rev`. It fetches every `project.snapshot` page and joins them. A page of another `rev`, another index or another page count (or a count of 0) is `PLUGIN_PROTOCOL {reason: snapshotRev | snapshotPage | snapshotPages, page, expected, got}`.
- `host.view({budget?, range?})` returns `fin view`'s text.
- `host.fetch(request, {grant?})` returns `{status, headers, body, redirectNotFollowed}`. The request goes through the host's HTTPS broker (`net.fetch`), so only approved origins work. The body is read in `net.read` pages of at most `PAGE_BYTES` (756 KiB) until `eof` and must add up to the declared `length` (`bodyLength`); a page longer than asked or empty before `eof` is `bodyPage`, and a declared `length` over 64 MiB is `bodyTooLarge` before anything is read (all `PLUGIN_PROTOCOL`).
- `host.log(level, msg)` writes to the server log. It does not wait for the reply.
- `host.context` is the job's `PluginContext`: host API, plugin, grants, `dirs`, locale and limits. `host.run` is the `CommandRun`: `command`, `args`, `project`, `branch` and `rev`.
- `host.signal` is an `AbortSignal` that fires when the host sends `cancel`. Pending host calls then reject with `JOB_CANCELLED`, and the job ends `JOB_CANCELLED` whatever the handler returns. If the handler has not settled after `CANCEL_GRACE_MS` (1.5 s), the SDK reports the cancel and the process exits. The signal also fires when the job ends normally, so work the handler left running (a timer, a listener) can stop then too.
- `InputFile`, `OutputFile` and `openGrant` are for the job files of cold kinds (8C onward). Inputs are read by position only (`fs.readSync` at an offset). Outputs open write-only (a Node `--allow-fs-write` grant suffices) and are written only inside the size the host created them at. Neither follows a final symlink (where the OS has no `O_NOFOLLOW`, as on Windows, an `lstat` check before the open refuses links and junctions; the host's validation of its copy is the real guard there), and both must be regular files of the declared size. A grant of the wrong mode is `PLUGIN_PERMISSION_DENIED {perm: "fs", access}`; an output of the wrong size, truncated, a link or not a regular file is `PLUGIN_OUTPUT_INVALID {reason: size | truncated | link | notRegular}` (the same for an input is `PLUGIN_PROTOCOL`, the host's fault); an I/O failure is `PLUGIN_COMMAND_FAILED {reason: io}`. Details name the grant (`file`), never its path.

All protocol types come from `@finisher/core-types`, which is generated from the Rust definitions.

## Errors

The table shows what the job's `error` becomes for what a handler throws. The envelope's `JobError` has no `fix`, so a `fix` travels in `details.fix`.

| Thrown | Job error |
|---|---|
| `new PluginError(code, message, details?, fix?)`, or any object with string `code` and `message` that is not a Node error | that code, message and details |
| Node's `ERR_ACCESS_DENIED` (the permission model) | `PLUGIN_PERMISSION_DENIED {perm: "fs" \| "unknown", nodePermission}`: `perm` is a manifest permission name (`fs` for Node's `FileSystem*` scopes) or `"unknown"`, `nodePermission` Node's scope; never the resource (often a path) |
| anything else (a `TypeError`, `ENOENT`, …) | `PLUGIN_COMMAND_FAILED {name, nodeCode?}` with the error's message |
| a command the plugin does not define | `PLUGIN_PROTOCOL {reason: unknownCommand, command}` |
| a job of another kind or version, params that are not a `CommandRun`, a run without a `PluginContext` | `PLUGIN_PROTOCOL {reason: kind \| params \| context}` (`kind` carries `kind` and `version`) |
| a result that is neither `{tx}` nor `{data}`, or does not serialise as JSON (a `BigInt`, a cycle; `NaN` becomes `null` as in `JSON.stringify`) | `PLUGIN_PROTOCOL {reason: badResult}` |
| a result whose line passes 1 MiB | `PLUGIN_PROTOCOL {reason: tooLarge, bytes}` |
| an error whose line passes 1 MiB (or does not serialise) | sent again with its code, its message cut to 1 KiB and no details; if even that fails, `PLUGIN_PROTOCOL {reason: tooLarge, bytes}` |

Every job is answered: the SDK never leaves one without a `result` or an `error`. A host call too large for one line rejects with `PLUGIN_PROTOCOL {reason: tooLarge, method, bytes}` without being sent. `PLUGIN_COMMAND_FAILED` and `PLUGIN_OUTPUT_INVALID` exit `fin` with 4, as the other job outcomes.

A failed host call rejects with a `PluginError` that carries the host's code, for example `PLUGIN_PERMISSION_DENIED` or `EGRESS_DENIED`. If the handler does not catch it, it becomes the job's error.

## The envelope rules (plan 8B Decision 21)

- `hello` is `{v: 1, minor: SDK_MINOR, role: "worker", family: "plugin", kinds: [{kind: "plugin.command", version: 1}], warm, pid}`, with no `envelope` hash.
- A trailing `--warm` argument means the SDK sends `ready` after each job. Without it, the process exits after its one job.
- `alive` goes out every 5 s (`ALIVE_MS`) from `hello`, and every `limits.aliveMs` of the latest `start` from then on.
- `shutdown` ends the process with 0. The end of stdin, or a stdout that fails (EPIPE), means the host is gone: the process exits 70.
- A host message type the SDK does not know comes from a newer host. The SDK drops it (describing it on stderr once per type) and never fails on it, even when it carries an `id`. Plugins cannot answer host-initiated calls, and the host applies its own minor rule to them. Unknown fields of known messages and payloads are ignored.
- A host line that is not JSON, has a wrong `v` or `seq`, passes 1 MiB (refused as it arrives, never buffered whole) or is a known type without the fields the SDK reads (`start.job.id`, `cancel.job`, `reply.id`, …) is a host bug. The process describes it on stderr and exits 65.

## Bundling (esbuild)

The `node` runtime runs the manifest's `runtime.main` with Node's permission model. The plugin can read its package and its temp directory, and write only to its temp. It cannot start child processes, load addons or start workers. The package is installed as-is: there is no `npm install`, no `node_modules` and no install script. The plugin must therefore be **one ES module that imports only `node:` builtins**.

```sh
npx esbuild src/plugin.ts --bundle --platform=node --format=esm --target=node24 \
  --outfile=dist/plugin.mjs --legal-comments=inline
```

- `--bundle` inlines the SDK and every other dependency. Check the output (`grep -E '^import|require\(' dist/plugin.mjs`): only `node:` specifiers may remain. Do not mark packages `--external`.
- Do not depend on packages with native addons or install scripts. They cannot run under the permission model, and nothing installs them.
- `runtime.main` in `finisher-plugin.json` names the bundle (`"main": "dist/plugin.mjs"`). The package's `engines.node` (`>=24.22` for this SDK) becomes the manifest's `runtime.node`. The host refuses a range its Node does not satisfy.
- Ship `dist/` in the package, not `src/` or `node_modules`.

This repository does not depend on esbuild. Its own example and the SDK's test plugins are built by `tools/bundle.mjs`, a small bundler that joins the SDK's module subset (Node's `module.stripTypeScriptTypes`, no dependency). It refuses anything outside that subset, so it is not a general bundler: use esbuild for your plugin. `dist/` is git-ignored, and the SDK's tests build `examples/plugins/ts-tools/dist/plugin.mjs` (`node examples/plugins/ts-tools/build.mjs`) before they spawn it.

## Tests

```sh
pnpm --filter @finisher/plugin-sdk test     # or: pnpm vitest run packages/plugin-sdk
```

`test/sdk.test.ts` runs the bundled test plugin and the example as real `node` processes. It uses a fake host (`test/fake-host.ts`) that speaks the host side of the envelope, and runs some of the plugins under the `node` runtime's permission flags.
