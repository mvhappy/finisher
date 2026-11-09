// The plugin side of 5C's `fin-jobs` envelope (plan 8B Decision 2): NDJSON, one message per
// line, each `{v: 1, t, seq}` with `seq` from 1 per direction, at most 1 MiB per line.
// `hello` carries its own `v`; every other line gets `v` here. Incoming lines must be JSON
// objects with `v` 1 and the next `seq` (a host bug otherwise: the process exits 65, as
// fin-jobs' own runtime does); an unknown `t` from a newer host is handed on and dropped by
// the runtime, never fatal (Decision 21).
import { createInterface } from "node:readline";
import type { Readable, Writable } from "node:stream";
import type { HostMsg, WorkerMsg } from "@finisher/core-types";

/** The envelope's major version (`hello.v`, 5C-1). */
export const ENVELOPE_VERSION = 1;
/**
 * The envelope minor this SDK speaks (`fin_jobs::MINOR`). A host whose minor is lower decodes
 * this process leniently; an equal one strictly (5C-1 Decision 3).
 */
export const SDK_MINOR = 0;
/** The largest line either side sends, newline excluded (5C's cap). */
export const MAX_LINE_BYTES = 1 << 20;
/** The exit code of a protocol error (sysexits `EX_DATAERR`, as fin-jobs' runtime). */
export const EXIT_PROTOCOL = 65;

/** A host message, or a message of a type this SDK does not know (a newer host's). */
export type Incoming = HostMsg | { readonly t: "unknown"; readonly type: string; readonly id?: number };

/** What the envelope reads from and writes to: the process's stdio, or a test's streams. */
export interface EnvelopeIo {
  readonly input: Readable;
  readonly output: Writable;
  /** Ends the process (or the test's stand-in) once `output` is flushed. */
  readonly exit: (code: number) => void;
  /** Where a protocol failure is described (stderr; never journaled by the host). */
  readonly warn: (line: string) => void;
}

const KNOWN_HOST_TYPES: ReadonlySet<string> = new Set(["start", "cancel", "shutdown", "attach", "reply"]);

/** A line too long for the envelope, refused before it is written. */
export class LineTooLong extends Error {
  readonly bytes: number;
  constructor(bytes: number) {
    super(`an envelope line of ${bytes} bytes passes the ${MAX_LINE_BYTES}-byte cap`);
    this.bytes = bytes;
  }
}

/** Serialises one message with `v` and `seq`; throws `LineTooLong` past the cap. */
export function encodeLine(seq: number, msg: WorkerMsg): string {
  const line = JSON.stringify({ v: ENVELOPE_VERSION, ...msg, seq });
  const bytes = Buffer.byteLength(line);
  if (bytes > MAX_LINE_BYTES) throw new LineTooLong(bytes);
  return `${line}\n`;
}

/** Parses one host line, checking `v` and `seq`; a string is the protocol error. */
export function decodeLine(line: string, expectSeq: number): Incoming | string {
  let m: unknown;
  try {
    m = JSON.parse(line);
  } catch {
    return "a host line is not JSON";
  }
  if (typeof m !== "object" || m === null || Array.isArray(m)) return "a host line is not an object";
  const o = m as Record<string, unknown>;
  if (o["v"] !== ENVELOPE_VERSION) return `a host line has v ${JSON.stringify(o["v"])}, not ${ENVELOPE_VERSION}`;
  if (o["seq"] !== expectSeq) return `a host line has seq ${JSON.stringify(o["seq"])}, expected ${expectSeq}`;
  const t = o["t"];
  if (typeof t !== "string") return "a host line has no type";
  if (!KNOWN_HOST_TYPES.has(t)) {
    const id = o["id"];
    return typeof id === "number" ? { t: "unknown", type: t, id } : { t: "unknown", type: t };
  }
  return o as unknown as HostMsg;
}

/** One envelope over `io`: `send` writes, `onMessage` receives, `onEnd` is stdin's EOF. */
export class Envelope {
  #seqOut = 0;
  #seqIn = 0;
  #closed = false;
  readonly #io: EnvelopeIo;

  constructor(io: EnvelopeIo, onMessage: (m: Incoming) => void, onEnd: () => void) {
    this.#io = io;
    const rl = createInterface({ input: io.input, crlfDelay: Infinity });
    rl.on("line", (line) => {
      if (this.#closed) return;
      const m = decodeLine(line, ++this.#seqIn);
      if (typeof m === "string") return this.fail(m);
      onMessage(m);
    });
    rl.on("close", () => {
      if (!this.#closed) onEnd();
    });
  }

  /** Writes `msg`; throws `LineTooLong` (nothing is written then). Ignored once closed. */
  send(msg: WorkerMsg): void {
    if (this.#closed) return;
    const line = encodeLine(this.#seqOut + 1, msg);
    this.#seqOut++;
    this.#io.output.write(line);
  }

  /** Flushes what was written and ends the process with `code`. */
  close(code: number): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#io.output.write("", () => this.#io.exit(code));
  }

  /** A protocol error from the host's side: described on stderr, exit 65. */
  fail(why: string): void {
    this.#io.warn(`finisher plugin: protocol error: ${why}`);
    this.close(EXIT_PROTOCOL);
  }

  get closed(): boolean {
    return this.#closed;
  }
}
