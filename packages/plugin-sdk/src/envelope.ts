// The plugin side of 5C's `fin-jobs` envelope (plan 8B Decision 2): NDJSON, one message per
// line, each `{v: 1, t, seq}` with `seq` from 1 per direction, at most 1 MiB per line.
// `hello` carries its own `v`; every other line gets `v` here. Incoming lines must be JSON
// objects with `v` 1, the next `seq` and, for a type this SDK knows, the fields it reads (a
// host bug otherwise: the process exits 65, as fin-jobs' own runtime does); a line passing the
// cap is refused as it arrives, never buffered whole. An unknown `t` from a newer host is
// described on stderr once per type, handed on and dropped by the runtime, never fatal
// (Decision 21). The end of stdin, or a stdout that fails (EPIPE), means the host is gone:
// exit 70.
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
/** The exit code once the host is gone: stdin ended or stdout broke (`fin_jobs` `EXIT_HOST_GONE`). */
export const EXIT_HOST_GONE = 70;

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

type Obj = Record<string, unknown>;
const isObj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);
const isStr = (v: unknown): v is string => typeof v === "string";
const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

/**
 * What is wrong with a known message's shape, or `undefined`: only the fields this SDK reads
 * are checked, and unknown fields are allowed (a newer host may add some).
 */
function shapeProblem(t: string, o: Obj): string | undefined {
  switch (t) {
    case "start": {
      const job = o["job"];
      if (!isObj(job)) return "no job";
      if (!isStr(job["id"]) || !isStr(job["kind"]) || !isNum(job["kindVersion"])) return "a job without a string id and kind and a numeric kindVersion";
      if (!isObj(o["limits"]) || !isNum(o["limits"]["aliveMs"])) return "no limits.aliveMs";
      if (!isObj(o["dirs"])) return "no dirs";
      if (o["files"] !== undefined && !Array.isArray(o["files"])) return "files that are not a list";
      return undefined;
    }
    case "cancel":
      return isStr(o["job"]) ? undefined : "no job id";
    case "reply": {
      if (!Number.isSafeInteger(o["id"])) return "no numeric id";
      const e = o["error"];
      if (e !== undefined && (!isObj(e) || !isStr(e["code"]) || !isStr(e["message"]))) return "an error without a string code and message";
      return undefined;
    }
    case "attach":
      return isStr(o["job"]) && isObj(o["dirs"]) ? undefined : "no job id and dirs";
    default:
      return undefined;
  }
}

const KNOWN_HOST_TYPES: ReadonlySet<string> = new Set(["start", "cancel", "shutdown", "attach", "reply"]);

/** Parses one host line, checking `v`, `seq` and a known type's shape; a string is the protocol error. */
export function decodeLine(line: string, expectSeq: number): Incoming | string {
  let m: unknown;
  try {
    m = JSON.parse(line);
  } catch {
    return "a host line is not JSON";
  }
  if (!isObj(m)) return "a host line is not an object";
  if (m["v"] !== ENVELOPE_VERSION) return `a host line has v ${JSON.stringify(m["v"])}, not ${ENVELOPE_VERSION}`;
  if (m["seq"] !== expectSeq) return `a host line has seq ${JSON.stringify(m["seq"])}, expected ${expectSeq}`;
  const t = m["t"];
  if (!isStr(t)) return "a host line has no type";
  if (!KNOWN_HOST_TYPES.has(t)) {
    const id = m["id"];
    return typeof id === "number" ? { t: "unknown", type: t, id } : { t: "unknown", type: t };
  }
  const problem = shapeProblem(t, m);
  if (problem !== undefined) return `a host ${t} message has ${problem}`;
  return m as unknown as HostMsg;
}

/** One envelope over `io`: `send` writes, `onMessage` receives, `onEnd` is stdin's EOF. */
export class Envelope {
  #seqOut = 0;
  #seqIn = 0;
  #closed = false;
  #exited = false;
  #ended = false;
  /** The bytes of the line being read, before its newline. */
  #parts: Buffer[] = [];
  #partBytes = 0;
  readonly #warnedTypes = new Set<string>();
  readonly #io: EnvelopeIo;
  readonly #onMessage: (m: Incoming) => void;
  readonly #onEnd: () => void;

  constructor(io: EnvelopeIo, onMessage: (m: Incoming) => void, onEnd: () => void) {
    this.#io = io;
    this.#onMessage = onMessage;
    this.#onEnd = onEnd;
    io.input.on("data", (chunk: Buffer | string) => this.#read(typeof chunk === "string" ? Buffer.from(chunk) : chunk));
    const end = (): void => {
      if (this.#ended) return;
      this.#ended = true;
      // A last line without its newline is still a line (as `readline` reads it).
      if (!this.#closed && this.#partBytes > 0) this.#line(Buffer.concat(this.#parts));
      if (!this.#closed) this.#onEnd();
    };
    io.input.on("end", end);
    io.input.on("close", end);
    io.input.on("error", () => this.#hostGone());
    // A write to a host that has gone (EPIPE) is not the plugin's crash.
    io.output.on("error", () => this.#hostGone());
  }

  /** Splits `chunk` into lines; a line that passes the cap fails before it is held whole. */
  #read(chunk: Buffer): void {
    let from = 0;
    while (!this.#closed) {
      const nl = chunk.indexOf(0x0a, from);
      const piece = chunk.subarray(from, nl < 0 ? chunk.length : nl);
      // One more byte than the cap for a CRLF's `\r`.
      if (this.#partBytes + piece.length > MAX_LINE_BYTES + 1) {
        return this.fail(`a host line passes the ${MAX_LINE_BYTES}-byte cap`);
      }
      if (nl < 0) {
        if (piece.length > 0) {
          this.#parts.push(Buffer.from(piece));
          this.#partBytes += piece.length;
        }
        return;
      }
      const line = this.#parts.length === 0 ? piece : Buffer.concat([...this.#parts, piece]);
      this.#parts = [];
      this.#partBytes = 0;
      this.#line(line);
      from = nl + 1;
    }
  }

  #line(bytes: Buffer): void {
    const end = bytes.length > 0 && bytes[bytes.length - 1] === 0x0d ? bytes.length - 1 : bytes.length;
    if (end > MAX_LINE_BYTES) return this.fail(`a host line passes the ${MAX_LINE_BYTES}-byte cap`);
    const m = decodeLine(bytes.toString("utf8", 0, end), ++this.#seqIn);
    if (typeof m === "string") return this.fail(m);
    if (m.t === "unknown" && !this.#warnedTypes.has(m.type)) {
      this.#warnedTypes.add(m.type);
      this.#io.warn(`finisher plugin: dropping host messages of the unknown type ${JSON.stringify(m.type)} (a newer host?)`);
    }
    this.#onMessage(m);
  }

  /** Writes `msg`; throws `LineTooLong` or JSON's own error (nothing is written then). Ignored once closed. */
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
    this.#io.output.write("", () => this.#exit(code));
  }

  /** A protocol error from the host's side: described on stderr, exit 65. */
  fail(why: string): void {
    if (this.#closed) return;
    this.#io.warn(`finisher plugin: protocol error: ${why}`);
    this.close(EXIT_PROTOCOL);
  }

  #hostGone(): void {
    this.#closed = true;
    this.#exit(EXIT_HOST_GONE);
  }

  #exit(code: number): void {
    if (this.#exited) return;
    this.#exited = true;
    this.#io.exit(code);
  }

  get closed(): boolean {
    return this.#closed;
  }
}
