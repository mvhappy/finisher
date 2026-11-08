import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const gate = (root) => runGate("./check-worker-spawn.mjs", ["--root", root]);
const repo = fileURLToPath(new URL("..", import.meta.url));

const FORBIDDEN = {
  command: "use std::process::Command;\nfn f() { let _ = Command::new(\"sh\"); }\n",
  braced: "use std::process::{Child, Command};\n",
  fork: "fn f() { unsafe { libc::fork() }; }\n",
  vfork: "fn f() { unsafe { libc::vfork() }; }\n",
  posixSpawn: "fn f() { unsafe { libc::posix_spawnp(a, b, c, d, e, g) }; }\n",
  exec: "fn f() { unsafe { libc::execve(p, a, e) }; }\n",
  commandExtExec: "fn f(c: &mut C) { let _ = c.exec(); }\n",
  createProcess: "fn f() { unsafe { CreateProcessW(a, b) }; }\n",
  shellExecute: "fn f() { unsafe { ShellExecuteW(a, b) }; }\n",
  clone: "fn f() { unsafe { libc::clone(cb, stack, flags, arg) }; }\n",
  syscall: "fn f() { unsafe { libc::syscall(libc::SYS_clone3, a, b) }; }\n",
};

for (const [name, text] of Object.entries(FORBIDDEN)) {
  test(`flags ${name} in fin-workers and fin-jobs`, () => {
    for (const crate of ["fin-workers", "fin-jobs"]) {
      const r = gate(fixtureRoot({ [`crates/${crate}/src/worker/x.rs`]: text }));
      assert.equal(r.code, 1, r.out);
      assert.match(r.out, new RegExp(`WORKER_SPAWN crates/${crate}/src/worker/x\\.rs:\\d+: `));
    }
  });
}

test("ignores test_family/, watchdog.rs and other crates", () => {
  const r = gate(
    fixtureRoot({
      "crates/fin-workers/src/test_family/kinds.rs": FORBIDDEN.fork,
      "crates/fin-workers/src/watchdog.rs": FORBIDDEN.command,
      "crates/fin-jobhost/src/unix/linux_spawn.rs": FORBIDDEN.command,
    }),
  );
  assert.equal(r.code, 0, r.out);
});

test("ignores comments, strings and clone() that is not clone(2)", () => {
  const r = gate(
    fixtureRoot({
      "crates/fin-jobs/src/lib.rs": [
        "// std::process::Command is not used here; nor is fork()",
        "/* execve( */ /* nested /* posix_spawn */ still */",
        'const S: &str = "Command::new(\\"x\\")"; const R: &str = r#"fork()"#;',
        "fn f(a: Arc<u8>) { let b = a.clone(); let c = Arc::clone(&a); }",
        "impl Clone for X { fn clone(&self) -> Self { *self } }",
        "fn execute(x: u8) {} fn g() { execute(1); }",
      ].join("\n"),
    }),
  );
  assert.equal(r.code, 0, r.out);
});

test("reports the line", () => {
  const r = gate(fixtureRoot({ "crates/fin-jobs/src/a.rs": "// one\n\nfn f() { unsafe { libc::fork() }; }\n" }));
  assert.match(r.out, /WORKER_SPAWN crates\/fin-jobs\/src\/a\.rs:3: fork/);
});

test("passes on the real tree", () => {
  const r = gate(repo);
  assert.equal(r.code, 0, r.out);
});
