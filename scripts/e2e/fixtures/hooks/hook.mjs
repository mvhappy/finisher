// The one hook program of the "hooks" e2e scenario: run as `node hook.mjs <args>` from a hooks
// file. It reads its request from stdin, then does what its arguments say, in order:
//   --write <path>        tries to write <path> and answers a `context` reply holding the outcome
//   --write-project       tries to write <project.dir>/planted.txt (from the request) and appends the outcome to a
//                         `block` reply's reason, as ` [write=<code>]`
//   --reply allow | block:<reason> | context:<text>   its reply (default: `allow`)
import { writeFileSync } from "node:fs";
import { join } from "node:path";

const args = process.argv.slice(2);
let stdin = "";
for await (const chunk of process.stdin) stdin += chunk;

let reply = { reply: "allow" };
let projectWrite;
for (let i = 0; i < args.length; i++) {
  const next = () => args[++i];
  switch (args[i]) {
    case "--write": {
      const path = next();
      let write = "ok";
      try {
        writeFileSync(path, "hook\n");
      } catch (e) {
        write = e.code ?? String(e);
      }
      reply = { reply: "context", text: JSON.stringify({ write }) };
      break;
    }
    case "--write-project": {
      projectWrite = "ok";
      try {
        writeFileSync(join(JSON.parse(stdin).project.dir, "planted.txt"), "hook\n");
      } catch (e) {
        projectWrite = e.code ?? String(e);
      }
      break;
    }
    case "--reply": {
      const v = next();
      if (v.startsWith("block:")) reply = { reply: "block", reason: v.slice(6) };
      else if (v.startsWith("context:")) reply = { reply: "context", text: v.slice(8) };
      break;
    }
    default:
      throw new Error(`unknown argument ${args[i]}`);
  }
}
if (projectWrite !== undefined && reply.reply === "block") reply.reason += ` [write=${projectWrite}]`;
process.stdout.write(`${JSON.stringify(reply)}\n`);
