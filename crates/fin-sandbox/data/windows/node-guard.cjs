// Finisher's Node guard for the Windows sandbox (plan 7C Decision 25). Loaded with `--require`
// by Node processes `fin-sandbox-exec` starts in an AppContainer (and, through NODE_OPTIONS,
// by Node processes they start). Inside a container libuv cannot create the named pipes it
// uses for piped stdio (`\\?\pipe\uv\...`), and a `child_process` call with piped stdio hangs
// forever. On the first such call this guard probes, off the main thread, whether a pipe of
// that kind can be made; if it cannot (or the probe takes longer than PROBE_MS), the call
// throws WINDOWS_NODE_PIPES instead of hanging. A probe that passes is cached and the guard
// changes nothing, so it goes inert once Node ships the fix. Without the marker variable (a
// process outside a container) it does nothing at all. No dependencies.
"use strict";

if (process.platform === "win32" && process.env.FIN_SANDBOX_NODE_GUARD === "1") {
  const cp = require("node:child_process");
  const { Worker } = require("node:worker_threads");
  const { syncBuiltinESMExports } = require("node:module");

  const PROBE_MS = 2000;
  const MESSAGE =
    "Node child processes with piped stdio are not supported inside the Windows sandbox " +
    "until Node ships the AppContainer pipe fix; see the Finisher README";
  let verdict; // undefined: not probed; true: pipes work; false: they do not

  const PROBE = `
    const { workerData } = require("node:worker_threads");
    const net = require("node:net");
    const flag = new Int32Array(workerData.sab);
    const done = (v) => { Atomics.store(flag, 0, v); Atomics.notify(flag, 0); };
    const name = "\\\\\\\\?\\\\pipe\\\\uv\\\\finisher-guard-" + process.pid + "-" + Date.now();
    const server = net.createServer();
    server.on("error", () => done(2));
    server.listen(name, () => { server.close(); done(1); });
  `;

  const pipesWork = () => {
    if (verdict !== undefined) return verdict;
    const sab = new SharedArrayBuffer(4);
    const flag = new Int32Array(sab);
    let worker;
    try {
      worker = new Worker(PROBE, { eval: true, workerData: { sab } });
      worker.unref();
      Atomics.wait(flag, 0, 0, PROBE_MS);
    } catch {
      // A probe that cannot run says nothing: treat the pipes as broken.
    }
    verdict = Atomics.load(flag, 0) === 1;
    if (worker) worker.terminate().catch(() => {});
    return verdict;
  };

  const PIPES = new Set(["pipe", "overlapped", "ipc"]);
  // Whether a `stdio` option asks for a pipe (Node's defaults: three pipes; `fork` always has
  // its IPC channel, a pipe too).
  const piped = (stdio, fork) => {
    if (fork) return true;
    if (stdio === undefined || stdio === null) return true;
    if (typeof stdio === "string") return PIPES.has(stdio);
    if (!Array.isArray(stdio)) return false;
    for (let i = 0; i < Math.max(3, stdio.length); i++) {
      const s = stdio[i];
      if ((s === undefined || s === null) && i < 3) return true;
      if (typeof s === "string" && PIPES.has(s)) return true;
    }
    return false;
  };

  const optionsOf = (args) => {
    for (const a of args) if (a && typeof a === "object" && !Array.isArray(a)) return a;
    return undefined;
  };

  const refuse = () => {
    const e = new Error(MESSAGE);
    e.code = "WINDOWS_NODE_PIPES";
    return e;
  };

  const wrap = (name, fork) => {
    const real = cp[name];
    if (typeof real !== "function") return;
    cp[name] = function guarded(...args) {
      const opts = optionsOf(args.slice(1));
      if (piped(opts && opts.stdio, fork) && !pipesWork()) {
        throw refuse();
      }
      return real.apply(this, args);
    };
  };

  for (const name of ["spawn", "spawnSync", "exec", "execSync", "execFile", "execFileSync"]) {
    wrap(name, false);
  }
  wrap("fork", true);
  syncBuiltinESMExports();
}
