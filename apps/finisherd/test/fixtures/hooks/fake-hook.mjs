// The one hook program of the hook tests (7F plan, Testing): finisherd's own Node runs it as
// `run: ["node", "<this file>", <args…>]`, so the same configuration runs on POSIX and Windows
// with no shebang and no shell. It reads stdin to its end, then does what its arguments say, in
// order, and answers one reply line:
//   --reply allow | block:<reason> | context:<text>   the reply (default: what was observed, as
//                                                       context, or nothing: `allow`)
//   --raw <text>         writes <text> to stdout as is (no reply of its own)
//   --raw-file <path>    writes the file's bytes to stdout as is (no reply of its own)
//   --oversize <bytes>   writes that many bytes to stdout, then waits to be killed
//   --exit <code>        exits with <code> after the reply
//   --kill               kills itself with SIGKILL
//   --sleep <ms>         waits
//   --fork-sleeper <m>   starts a sleeping child (in its process group, <m> in its argv) and
//                        writes its pid to `sleeper.pid` in the working directory
//   --stderr <text>      writes <text> to stderr
//   --report <what>      observes `env`, `args`, `stdin`, `cwd` or `net` (interfaces with an address)
//   --read <path>        observes whether <path> can be read
//   --write <path>       observes whether <path> can be written
//   --append <path>      appends its request (stdin, one line) to <path> (a log of its runs)
//   --gate-if <s> <path> when its request holds <s>, waits until <path> exists
//   --connect <port>     observes whether TCP 127.0.0.1:<port> connects
//   --connects <p,q>     observes, per port, whether TCP 127.0.0.1:<port> connects
//   --fin <json args>    runs `fin` (found on its `PATH`) with the JSON array's arguments and
//                        observes its exit code and its error's code and message
import { spawn, spawnSync } from "node:child_process";
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { connect } from "node:net";
import { networkInterfaces } from "node:os";
import { join } from "node:path";

const args = process.argv.slice(2);
const chunks = [];
for await (const c of process.stdin) chunks.push(c);
const stdin = Buffer.concat(chunks).toString("utf8");

const seen = {};
let reply;
let raw;
let exit = 0;
const errCode = (f) => {
  try {
    f();
    return "ok";
  } catch (e) {
    return e.code ?? String(e);
  }
};

for (let i = 0; i < args.length; i++) {
  const a = args[i];
  const next = () => args[++i];
  switch (a) {
    case "--reply": {
      const v = next();
      if (v === "allow") reply = { reply: "allow" };
      else if (v.startsWith("block:")) reply = { reply: "block", reason: v.slice(6) };
      else if (v.startsWith("context:")) reply = { reply: "context", text: v.slice(8) };
      else throw new Error(`unknown reply ${v}`);
      break;
    }
    case "--raw":
      raw = (raw ?? "") + next();
      break;
    case "--raw-file":
      raw = Buffer.concat([Buffer.from(raw ?? ""), readFileSync(next())]);
      break;
    case "--oversize":
      process.stdout.write("x".repeat(Number(next())));
      await new Promise((r) => setTimeout(r, 60_000));
      break;
    case "--exit":
      exit = Number(next());
      break;
    case "--kill":
      process.kill(process.pid, "SIGKILL");
      break;
    case "--stderr":
      process.stderr.write(next());
      break;
    case "--sleep":
      await new Promise((r) => setTimeout(r, Number(next())));
      break;
    case "--fork-sleeper": {
      const child = spawn(process.execPath, ["-e", "setTimeout(() => {}, 600000)", next()], { stdio: "ignore" });
      writeFileSync(join(process.cwd(), "sleeper.pid"), String(child.pid));
      break;
    }
    case "--report": {
      const what = next();
      if (what === "env") seen.env = { ...process.env };
      else if (what === "args") seen.args = args;
      else if (what === "stdin") seen.stdin = stdin;
      else if (what === "cwd") seen.cwd = process.cwd();
      else if (what === "net") seen.net = Object.values(networkInterfaces()).flat().filter((n) => n !== undefined && !n.internal).map((n) => n.address);
      break;
    }
    case "--read": {
      const p = next();
      seen.read = { ...seen.read, [p]: errCode(() => readFileSync(p)) };
      break;
    }
    case "--write": {
      const p = next();
      seen.write = { ...seen.write, [p]: errCode(() => writeFileSync(p, "hook\n")) };
      break;
    }
    case "--gate-if": {
      const [s, p] = [next(), next()];
      while (stdin.includes(s) && !existsSync(p)) await new Promise((r) => setTimeout(r, 20));
      break;
    }
    case "--append":
      appendFileSync(next(), `${stdin.trim()}\n`);
      break;
    case "--connect": {
      seen.connect = await tcp(Number(next()));
      break;
    }
    case "--connects": {
      seen.connects = {};
      for (const p of next().split(",")) seen.connects[p] = await tcp(Number(p));
      break;
    }
    case "--fin": {
      const r = spawnSync("fin", JSON.parse(next()), { encoding: "utf8", timeout: 20_000 });
      let out;
      try {
        out = JSON.parse(r.stdout);
      } catch {
        // Not JSON.
      }
      // Kept small (a context text is at most 16 KiB): the exit, the error's code and message.
      seen.fin = { code: r.status, ...(out?.error === undefined ? {} : { error: out.error.code, message: out.error.message }), ...(r.error === undefined ? {} : { spawn: r.error.code ?? String(r.error) }), ...(out === undefined ? { stderr: String(r.stderr ?? "").slice(0, 400) } : {}) };
      break;
    }
    default:
      throw new Error(`unknown argument ${a}`);
  }
}

/** Whether TCP 127.0.0.1:<port> connects: `ok`, the error's code or `timeout`. */
function tcp(port) {
  return new Promise((resolve) => {
    const s = connect({ host: "127.0.0.1", port });
    const done = (v) => {
      s.destroy();
      resolve(v);
    };
    s.once("connect", () => done("ok"));
    s.once("error", (e) => done(e.code ?? String(e)));
    setTimeout(() => done("timeout"), 3_000).unref();
  });
}

if (raw !== undefined) process.stdout.write(raw);
else if (reply !== undefined) process.stdout.write(`${JSON.stringify(reply)}\n`);
else if (Object.keys(seen).length > 0) process.stdout.write(`${JSON.stringify({ reply: "context", text: JSON.stringify(seen) })}\n`);
process.exitCode = exit;
