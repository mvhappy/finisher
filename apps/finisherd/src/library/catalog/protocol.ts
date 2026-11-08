// What crosses the catalog threads' `MessagePort`s (plan 5D Decisions 2, 24): requests
// `{id, method, args}` and replies `{id, ok, value | error}`, all structured-clone data. The
// main thread checks every `args` with `assertCloneable` **before** posting, so a function, a
// symbol or a class instance (which a structured clone would drop or strip silently) is
// refused instead. Imported by both threads and the client: Node built-ins only.

/** A request to a catalog thread. */
export interface Request {
  readonly id: number;
  readonly method: string;
  readonly args: unknown;
}

/** An error as it crosses a port: `FinError`'s fields. */
export interface WireError {
  readonly code: string;
  readonly message: string;
  readonly details: unknown;
}

export type Reply =
  | { readonly id: number; readonly ok: true; readonly value: unknown }
  | { readonly id: number; readonly ok: false; readonly error: WireError };

/** The first message of a thread: its extensions loaded, or why not. */
export type Ready = { readonly ready: true } | { readonly ready: false; readonly error: WireError };

/** An error with a code, thrown inside a catalog thread and sent back as a `WireError`. */
export class CatalogFailure extends Error {
  readonly code: string;
  readonly details: unknown;
  constructor(code: string, message: string, details: unknown = {}) {
    super(message);
    this.code = code;
    this.details = details;
  }
}

/** SQLite's primary result code of a `node:sqlite` error, if it has one. */
export function sqliteCode(e: unknown): number | undefined {
  const code = (e as { errcode?: unknown } | null)?.errcode;
  return typeof code === "number" ? code & 0xff : undefined;
}

/** Maps anything thrown in a thread to a `WireError` (`CATALOG_OP_FAILED` unless it has a code). */
export function toWire(e: unknown): WireError {
  if (e instanceof CatalogFailure) return { code: e.code, message: e.message, details: e.details };
  const message = String((e as Error | null)?.message ?? e);
  const code = sqliteCode(e);
  return {
    code: "CATALOG_OP_FAILED",
    message: "a catalog operation failed",
    details: { cause: message, ...(code === undefined ? {} : { sqlite: code }) },
  };
}

/** Prototypes whose instances a structured clone carries intact. */
const CLONEABLE = new Set<unknown>([
  Object.prototype,
  null,
  Array.prototype,
  Date.prototype,
  Map.prototype,
  Set.prototype,
  RegExp.prototype,
  ArrayBuffer.prototype,
  Uint8Array.prototype,
  Int8Array.prototype,
  Uint16Array.prototype,
  Int16Array.prototype,
  Uint32Array.prototype,
  Int32Array.prototype,
  Float32Array.prototype,
  Float64Array.prototype,
  BigInt64Array.prototype,
  BigUint64Array.prototype,
  DataView.prototype,
]);

/**
 * Throws `CATALOG_ARG_NOT_CLONEABLE` (with `details.at`, the offending key path) for a value a
 * structured clone would refuse or alter: functions, symbols, class instances, `Buffer`s (a
 * clone arrives as a plain `Uint8Array`) and `Error`s. Cycles are fine.
 */
export function assertCloneable(value: unknown, at = "args", seen = new Set<object>()): void {
  const t = typeof value;
  if (t === "function" || t === "symbol") {
    throw new CatalogFailure("CATALOG_ARG_NOT_CLONEABLE", "catalog arguments must be plain data", { at, type: t });
  }
  if (value === null || t !== "object") return;
  const obj = value as object;
  if (seen.has(obj)) return;
  seen.add(obj);
  const proto = Object.getPrototypeOf(obj) as unknown;
  if (!CLONEABLE.has(proto)) {
    const type = (obj as { constructor?: { name?: unknown } }).constructor?.name;
    throw new CatalogFailure("CATALOG_ARG_NOT_CLONEABLE", "catalog arguments must be plain data", {
      at,
      type: typeof type === "string" ? type : "object",
    });
  }
  if (obj instanceof Map) {
    for (const [k, v] of obj) {
      assertCloneable(k, `${at}.<key>`, seen);
      assertCloneable(v, `${at}.<value>`, seen);
    }
    return;
  }
  if (obj instanceof Set) {
    for (const v of obj) assertCloneable(v, `${at}.<item>`, seen);
    return;
  }
  if (ArrayBuffer.isView(obj) || obj instanceof ArrayBuffer || obj instanceof Date || obj instanceof RegExp) return;
  for (const key of Reflect.ownKeys(obj)) {
    if (typeof key === "symbol") {
      throw new CatalogFailure("CATALOG_ARG_NOT_CLONEABLE", "catalog arguments must be plain data", { at, type: "symbol key" });
    }
    const desc = Object.getOwnPropertyDescriptor(obj, key);
    if (desc?.get !== undefined || desc?.set !== undefined) {
      throw new CatalogFailure("CATALOG_ARG_NOT_CLONEABLE", "catalog arguments must be plain data", { at: `${at}.${key}`, type: "accessor" });
    }
    assertCloneable(desc?.value, `${at}.${key}`, seen);
  }
}

/** One event-loop turn, so other libraries' messages run between a long apply's transactions. */
export const nextTurn = (): Promise<void> => new Promise((resolve) => setImmediate(resolve));
