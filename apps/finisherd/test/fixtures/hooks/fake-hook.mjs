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
//   --connect <port>     observes whether TCP 127.0.0.1:<port> connects
import { spawn } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
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
    case "--connect": {
      const port = Number(next());
      seen.connect = await new Promise((resolve) => {
        const s = connect({ host: "127.0.0.1", port });
        const done = (v) => {
          s.destroy();
          resolve(v);
        };
        s.once("connect", () => done("ok"));
        s.once("error", (e) => done(e.code ?? String(e)));
        setTimeout(() => done("timeout"), 3_000).unref();
      });
      break;
    }
    default:
      throw new Error(`unknown argument ${a}`);
  }
}

if (raw !== undefined) process.stdout.write(raw);
else if (reply !== undefined) process.stdout.write(`${JSON.stringify(reply)}\n`);
else if (Object.keys(seen).length > 0) process.stdout.write(`${JSON.stringify({ reply: "context", text: JSON.stringify(seen) })}\n`);
process.exitCode = exit;
