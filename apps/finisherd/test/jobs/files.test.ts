// Job directories and mapped files (5C-1 plan Decision 38).
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { JobView } from "@finisher/contracts";
import { Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { openControlStore } from "../../src/control/db.ts";
import { checkPrepared, createFiles, removeJobDir, sweep } from "../../src/jobs/files.ts";
import { JobStore } from "../../src/jobs/store.ts";
import { closeTestEngines, OWNER, testEngine, until, testClock, waitThroughBackoff } from "../support/jobs.ts";

afterEach(closeTestEngines);

const win = process.platform === "win32";
const mode = (p: string): number => statSync(p).mode & 0o777;
const scratch: string[] = [];
afterEach(() => {
  for (const d of scratch.splice(0)) rmSync(d, { recursive: true, force: true });
});
function tmp(): string {
  const d = mkdtempSync(join(tmpdir(), "fin-jobfiles-"));
  scratch.push(d);
  return d;
}

describe("job files", () => {
  test("files_created_sized_and_private", async () => {
    const seen: Record<string, number> = {};
    const t = await testEngine({
      testKinds: {
        overrides: {
          "test.files": {
            onSucceeded: (ctx) =>
              Effect.sync(() => {
                const dir = join(t.serverData, "jobs", ctx.job.id);
                seen.size = statSync(join(dir, "out.bin")).size;
                if (!win) {
                  seen.dir = mode(dir);
                  seen.tmp = mode(join(dir, "tmp"));
                  seen.out = mode(join(dir, "out"));
                  seen.bin = mode(join(dir, "out.bin"));
                }
              }),
          },
        },
      },
    });
    const v = await t.runJob("test.files", {});
    expect(v.state).toBe("succeeded");
    const files = (v.result as { files: Array<{ name: string; bytes: number }> }).files;
    expect(files).toEqual([
      expect.objectContaining({ name: "in/data.txt", mode: "read", bytes: "prepared by test.files\n".length }),
      expect.objectContaining({ name: "out.bin", mode: "write", bytes: 4096 }),
    ]);
    expect(seen.size).toBe(4096);
    // Windows: the directory inherits the jobs root's owner-only DACL (`ensurePrivateDir`).
    if (!win) expect(seen).toMatchObject({ dir: 0o700, tmp: 0o700, out: 0o700, bin: 0o600 });
    expect(mode(join(t.serverData, "jobs")) & (win ? 0 : 0o077)).toBe(0);
  });

  test("out_dir_emptied_between_attempts", async () => {
    const clock = await testClock();
    const t = await testEngine({ clock: clock.clock });
    const v = await waitThroughBackoff(t, clock, (await t.submit("test.files", { failOnAttempt: 1 })).id);
    expect(v.state).toBe("succeeded");
    expect(v.attempts).toBe(2);
    // Attempt 1 left `attempt.txt` in out/; attempt 2 found it empty.
    expect((v.result as { outEmpty: boolean }).outEmpty).toBe(true);
  });

  test("missing_prepared_file_fails_the_attempt", async () => {
    const t = await testEngine({ testKinds: { overrides: { "test.files": { prepare: () => Effect.void } } } });
    const v = await t.runJob("test.files", {});
    expect(v.state).toBe("failed");
    expect(v.error).toMatchObject({ code: "IO_ERROR", details: { file: "in/data.txt", problem: "missing" } });
    expect(t.sink.exits).toHaveLength(0);
  });

  test.skipIf(win)("symlinked_prepared_file_is_refused", async () => {
    const elsewhere = join(tmp(), "secret.txt");
    writeFileSync(elsewhere, "not the job's");
    const t = await testEngine({
      testKinds: {
        overrides: {
          "test.files": {
            prepare: (ctx) =>
              Effect.sync(() => {
                // A hook that ignores `ctx.open` and plants a link itself.
                const dir = join(t.serverData, "jobs", ctx.job.id);
                mkdirSync(join(dir, "in"), { recursive: true });
                symlinkSync(elsewhere, join(dir, "in", "data.txt"));
              }),
          },
        },
      },
    });
    const v = await t.runJob("test.files", {});
    expect(v.error).toMatchObject({ code: "IO_ERROR", details: { problem: "a symlink" } });
  });

  test("hard_linked_and_non_regular_prepared_files_are_refused", async () => {
    const dir = tmp();
    mkdirSync(join(dir, "in"));
    mkdirSync(join(dir, "in", "data.txt"));
    await expect(checkPrepared(dir, [{ name: "in/data.txt", mode: "read" }])).rejects.toMatchObject({ details: { problem: "not a regular file" } });
    rmSync(join(dir, "in", "data.txt"), { recursive: true });
    writeFileSync(join(dir, "other"), "x");
    const { linkSync } = await import("node:fs");
    linkSync(join(dir, "other"), join(dir, "in", "data.txt"));
    await expect(checkPrepared(dir, [{ name: "in/data.txt", mode: "read" }])).rejects.toMatchObject({ details: { problem: "a file with other links" } });
  });

  test("read_files_keep_their_content_and_writable_ones_are_recreated", async () => {
    const dir = tmp();
    const files = [
      { name: "a/r.bin", bytes: 8, mode: "read" },
      { name: "w.bin", bytes: 8, mode: "write" },
      { name: "keep/", mode: "read" },
      { name: "out/", mode: "readWrite" },
    ] as const;
    await createFiles(dir, files);
    await writeFile(join(dir, "a", "r.bin"), "12345678");
    await writeFile(join(dir, "w.bin"), "12345678");
    await writeFile(join(dir, "keep", "x"), "x");
    await writeFile(join(dir, "out", "x"), "x");
    await createFiles(dir, files);
    expect(readdirSync(join(dir, "keep"))).toEqual(["x"]);
    expect(readdirSync(join(dir, "out"))).toEqual([]);
    expect(statSync(join(dir, "w.bin")).size).toBe(8);
    const { readFileSync } = await import("node:fs");
    expect(readFileSync(join(dir, "a", "r.bin"), "utf8")).toBe("12345678");
    expect(readFileSync(join(dir, "w.bin"))).toEqual(Buffer.alloc(8));
  });

  test("job_dir_removed_after_failure", async () => {
    const t = await testEngine();
    const v = await t.runJob("test.files", { failOnAttempt: 1 }, { maxAttempts: 1 });
    expect(v.state).toBe("failed");
    // The directory goes right after the job's end is announced (9a review H2).
    await until(() => !existsSync(join(t.serverData, "jobs", v.id)));
    const ok = await t.runJob("test.echo", { value: 1 });
    await until(() => !existsSync(join(t.serverData, "jobs", ok.id)));
    expect(readdirSync(join(t.serverData, "jobs"))).toEqual([]);
  });

  test("job_dirs_swept_at_start", async () => {
    const serverData = tmp();
    // A previous run: one job in its hook phase (its directory is kept), leftovers of others.
    const control = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
    const store = new JobStore(control);
    const hooks: JobView = store.insert({ kind: "test.echo", kindVersion: 1, family: "test", params: { value: 1 }, owner: OWNER, priority: "normal", maxAttempts: 3 });
    store.transition(hooks.id, "running", { attempts: 1 });
    store.enterHooks(hooks.id, { value: 1 });
    await control.close();
    for (const name of [hooks.id, "01OLDJOB", "stray"]) mkdirSync(join(serverData, "jobs", name, "tmp"), { recursive: true });
    writeFileSync(join(serverData, "jobs", "stray-file"), "x");
    let kept: boolean | undefined;
    const t = await testEngine({
      serverData,
      testKinds: { overrides: { "test.echo": { onSucceeded: (ctx) => Effect.sync(() => void (kept = lstatSync(join(serverData, "jobs", ctx.job.id, "tmp")).isDirectory())) } } },
    });
    // The hook-phase job's directory survived the sweep for its hook's re-run (Decision 28).
    expect((await t.wait(hooks.id)).state).toBe("succeeded");
    expect(kept).toBe(true);
    await until(() => readdirSync(join(serverData, "jobs")).length === 0);
  });

  test("sweep_and_remove_tolerate_missing_paths", async () => {
    const dir = tmp();
    expect(await sweep(join(dir, "none"), new Set())).toEqual([]);
    await removeJobDir(join(dir, "none"));
  });
});
