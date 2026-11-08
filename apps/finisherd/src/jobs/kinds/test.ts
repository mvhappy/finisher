// The test families and kinds (5C-1 plan Task 7; the workers are `fin-workers`' test family,
// Task 2). Registered only when the engine is built with `testKinds: true`. Params types are
// generated from Rust (`fin_jobs::test_kinds`).
import { join } from "node:path";
import type {
  TestCallParams,
  TestCheckpointParams,
  TestCrashParams,
  TestEchoParams,
  TestFilesParams,
  TestHelperParams,
  TestPluginMinorParams,
  TestSleepParams,
  TestSpawnParams,
  TestWarmParams,
} from "@finisher/core-types";
import { FinError } from "@finisher/contracts";
import { Duration, Effect } from "effect";
import type { FamilySpec, HelperProgram, KindRegistry, KindSpec, OnCall } from "../kinds.ts";

/** What `test.files`' `prepare` writes into its prepared input. */
export const TEST_FILES_INPUT = "prepared by test.files\n";

/**
 * The `test` family's host calls: `test.echo` (answers its params and the call's context),
 * `test.fail` (fails `TEST_FAIL` with the params as details), `test.slow {ms}` (answers after
 * `ms`). Any other method is `INVALID_REQUEST {method}`.
 */
export const testOnCall: OnCall = (ctx, method, params) => {
  switch (method) {
    case "test.echo":
      return Effect.succeed({
        echo: params,
        job: ctx.job.id,
        role: ctx.role,
        helper: ctx.helper,
        owner: ctx.owner,
        via: ctx.via,
        rights: ctx.rights,
      });
    case "test.fail":
      return Effect.fail(new FinError({ code: "TEST_FAIL", message: "test.fail was called", details: params }));
    case "test.slow": {
      const ms = Number((params as { ms?: unknown } | null)?.ms ?? 0);
      return Effect.sleep(Duration.millis(ms)).pipe(Effect.as(params));
    }
    default:
      return Effect.fail(new FinError({ code: "INVALID_REQUEST", message: `no host call ${method}`, details: { method } }));
  }
};

const base = { family: "test", version: 1, scopes: [], resources: ["cpu"], defaultPriority: "normal", qos: "normal", timeoutS: 600 } as const;

function kind<P>(k: Partial<KindSpec<P>> & { readonly kind: string }): KindSpec<P> {
  return { ...base, ...k } as KindSpec<P>;
}

/**
 * Registers `test`, `testbad` and `testplugin`, their kinds and the `codec:test` helper. The
 * programs here are the cold argv; a warm spawn takes `registry.program(family, job, "warm")`,
 * which appends `--warm` (`fin-workers test --warm`, `<path> testplugin --warm`). `overrides`
 * replaces fields of kinds by name and `families` of families (the engine's tests: a short
 * `stallS`, a hook, no `diagKinds`).
 */
export function registerTestKinds(
  registry: KindRegistry,
  opts: {
    readonly binaryDir: string;
    readonly overrides?: Readonly<Record<string, Partial<KindSpec>>>;
    readonly families?: Readonly<Record<string, Partial<FamilySpec>>>;
    /** Fields of the `codec:test` helper replaced (its `onCall`, for the engine's tests). */
    readonly helper?: Partial<Pick<HelperProgram, "onCall">>;
  },
): void {
  const exe = process.platform === "win32" ? "fin-workers.exe" : "fin-workers";
  registry.registerFamily({
    family: "test",
    profile: "worker",
    program: { binary: "fin-workers", args: ["test"] },
    onCall: testOnCall,
    diagKinds: ["test.diag"],
    ...(opts.families?.test ?? {}),
  });
  registry.registerFamily({ family: "testbad", profile: "worker", program: { binary: "fin-workers", args: ["testbad"] } });
  registry.registerFamily({
    family: "testplugin",
    profile: "plugin",
    program: { path: join(opts.binaryDir, exe), args: ["testplugin"] },
    warmKey: (job) => (job.params as TestPluginMinorParams).key ?? "",
    pluginId: "test.plugin",
  });
  registry.registerHelper({
    kind: "codec",
    id: "test",
    profile: "worker",
    resolve: (job) => ({ binary: "fin-workers", args: ["--helper", `test:${(job.params as TestHelperParams).helper ?? "ok"}`] }),
    onCall: testOnCall,
    ...(opts.helper ?? {}),
  });

  const kinds: KindSpec<never>[] = [
    kind<TestSleepParams>({ kind: "test.sleep" }),
    kind<TestEchoParams>({ kind: "test.echo" }),
    kind({ kind: "test.progressFlood" }),
    kind<TestWarmParams>({ kind: "test.warm", warm: true, defaultPriority: "interactive" }),
    kind<TestCrashParams>({
      kind: "test.crash",
      batch: {
        items: (p) => p.items,
        restrict: (p, items) => ({ ...p, items: [...items] }),
      },
    }),
    kind({ kind: "test.hang" }),
    kind({ kind: "test.stall", stallS: 600 }),
    kind<TestSpawnParams>({ kind: "test.spawn", files: (p) => [{ name: p.out, bytes: 4096, mode: "write" }] }),
    kind<TestFilesParams>({
      kind: "test.files",
      files: [
        { name: "in/data.txt", mode: "read" },
        { name: "out.bin", bytes: 4096, mode: "write" },
        { name: "out/", mode: "write" },
      ],
      prepare: (ctx) =>
        Effect.promise(async () => {
          const { mkdir, writeFile } = await import("node:fs/promises");
          await mkdir(ctx.path("in"), { recursive: true });
          await writeFile(ctx.path("in/data.txt"), TEST_FILES_INPUT, { mode: 0o600 });
          return { extra: { prepared: true } };
        }),
    }),
    kind<TestCheckpointParams>({ kind: "test.checkpoint", resumable: true }),
    kind({ kind: "test.oom" }),
    kind<TestCallParams>({ kind: "test.call" }),
    kind({ kind: "test.diag" }),
    kind({ kind: "test.garbage" }),
    kind({ kind: "test.exitAfterResult" }),
    kind<TestHelperParams>({
      kind: "test.helper",
      files: [{ name: "helper.bin", bytes: 4096, mode: "readWrite" }],
      helpers: [{ kind: "codec", id: "test", profile: "worker", files: ["helper.bin"] }],
    }),
    kind({ kind: "test.badHello", family: "testbad" }),
    kind<TestPluginMinorParams>({ kind: "test.pluginMinor", family: "testplugin", warm: true }),
  ] as KindSpec<never>[];
  for (const k of kinds) registry.registerKind({ ...k, ...(opts.overrides?.[k.kind] ?? {}) } as KindSpec<never>);
}
