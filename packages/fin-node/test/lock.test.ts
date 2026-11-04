import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { type FinNodeError, lockExclusive } from "../index.js";

const INDEX = fileURLToPath(new URL("../index.js", import.meta.url));
let tmp: string;
let file: string;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "fin-node-lock-"));
  file = join(tmp, "finisherd.lock");
});

afterEach(() => {
  rmSync(tmp, { recursive: true, force: true });
});

function thrown(f: () => unknown): FinNodeError {
  try {
    f();
  } catch (e) {
    return e as FinNodeError;
  }
  throw new Error("expected a throw");
}

/** Tries the lock from a separate process; returns its error code or "OK". */
function lockFromChild(path: string): string {
  const script = `
    const { lockExclusive } = await import(${JSON.stringify(INDEX)});
    try { lockExclusive(${JSON.stringify(path)}).release(); console.log("OK"); }
    catch (e) { console.log(e.code); }
  `;
  const r = spawnSync(process.execPath, ["--input-type=module", "-e", script], { encoding: "utf8" });
  if (r.status !== 0) throw new Error(r.stderr);
  return r.stdout.trim();
}

describe("lockExclusive", () => {
  test("is exclusive across descriptors and processes until released", () => {
    const held = lockExclusive(file);
    expect(held.path).toBe(file);
    expect(statSync(file).mode & 0o777).toBe(0o600);
    expect(readFileSync(file, "utf8").trim()).toBe(String(process.pid));

    const again = thrown(() => lockExclusive(file));
    expect(again.code).toBe("SERVER_RUNNING");
    expect(again.finNode).toBe(true);
    expect(lockFromChild(file)).toBe("SERVER_RUNNING");

    held.release();
    held.release(); // idempotent
    expect(lockFromChild(file)).toBe("OK");
    const next = lockExclusive(file);
    next.release();
  });

  test("a leftover lock file nobody holds does not block", () => {
    lockExclusive(file).release();
    expect(lockFromChild(file)).toBe("OK");
    lockExclusive(file).release();
  });

  test("I/O failures are SERVER_START_FAILED with a cause", () => {
    const err = thrown(() => lockExclusive(join(tmp, "missing-dir", "finisherd.lock")));
    expect(err.code).toBe("SERVER_START_FAILED");
    expect(err.details.cause).toBeTruthy();
  });
});
