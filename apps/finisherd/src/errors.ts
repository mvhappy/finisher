// Maps fin-node rejections (JS `Error` with `code`/`details`/`fix`) to the wire `FinError`.
import { FinError } from "@finisher/contracts";
import { Effect } from "effect";

const CODE = /^[A-Z][A-Z0-9_]*$/;

/** An engine `Error` keeps its stable code; anything else becomes `INTERNAL`. */
export function toFinError(e: unknown): FinError {
  if (e instanceof FinError) return e;
  if (e instanceof Error) {
    const { code, details, fix } = e as Error & { code?: unknown; details?: unknown; fix?: unknown };
    if (typeof code === "string" && CODE.test(code)) {
      return new FinError({
        code,
        message: e.message,
        details: details ?? {},
        ...(typeof fix === "string" ? { fix } : {}),
      });
    }
    return new FinError({ code: "INTERNAL", message: e.message, details: { name: e.name } });
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
