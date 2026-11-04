// Maps fin-node rejections (JS `Error` with `code`/`details`/`fix`) to the wire `FinError`.
import { FinError } from "@finisher/contracts";
import { Effect } from "effect";

/**
 * Only errors tagged by `@finisher/fin-node` (`finNode === true`) keep their code: those are
 * the stable fin-core/fin-store codes. Anything else — Node (`ENOENT`), SQLite
 * (`ERR_SQLITE_*`), bugs — becomes `INTERNAL`, with the original code in `details.cause`.
 */
export function toFinError(e: unknown): FinError {
  if (e instanceof FinError) return e;
  if (e instanceof Error) {
    const { code, details, fix, finNode } = e as Error & { code?: unknown; details?: unknown; fix?: unknown; finNode?: unknown };
    if (finNode === true && typeof code === "string") {
      return new FinError({
        code,
        message: e.message,
        details: details ?? {},
        ...(typeof fix === "string" ? { fix } : {}),
      });
    }
    return new FinError({
      code: "INTERNAL",
      message: e.message,
      details: { name: e.name, ...(code === undefined ? {} : { cause: String(code) }) },
    });
  }
  return new FinError({ code: "INTERNAL", message: String(e), details: {} });
}

/** Runs a promise-returning engine call, mapping every rejection through `toFinError`. */
export function attempt<A>(f: () => Promise<A>): Effect.Effect<A, FinError> {
  return Effect.tryPromise({ try: f, catch: toFinError });
}

/** Parses a JSON string returned by fin-node; malformed output is an `INTERNAL` error. */
export function attemptJson<A>(f: () => Promise<string>): Effect.Effect<A, FinError> {
  return attempt(async () => JSON.parse(await f()) as A);
}

export function finError(code: string, message: string, details: unknown = {}, fix?: string): FinError {
  return new FinError({ code, message, details, ...(fix === undefined ? {} : { fix }) });
}

/** True when an engine can no longer be used: a panic (`INTERNAL`) or `ENGINE_CLOSED` after one. */
export function isEngineDead(e: FinError): boolean {
  if (e.code === "INTERNAL") return true;
  const reason = (e.details as { reason?: unknown } | null | undefined)?.reason;
  return e.code === "ENGINE_CLOSED" && reason === "panicked";
}
