// The secret store, file backend (plan 7A Decision 22; sub-project 7 §10):
// `<server-data>/secrets.json`, mode 0600, holding sealed values only. In v1 it holds the
// OpenRouter key the user pastes, once per server and optionally once per project, and
// nothing else. No method returns a value except `reveal`, which only `ModelGateway` calls.
// Writes are temp-then-rename, serialised through one in-process queue and, across processes,
// a kernel lock on `secrets.lock` (review M2); the directory is fsynced after the rename.
//
// Protection is the private directory and the file mode, so both are checked (and repaired)
// at every read and write: the directory is ours, not a link, and 0700; the file is ours, a
// regular file, and 0600. Windows has no mode bits: there the directory gets 5A's protected
// owner-only DACL (fin-node `makePrivateDir`, the helper behind `ensurePrivateDir`) and both the
// directory and the file are read back with `checkPrivateDir` at every read and write; a loose
// directory is repaired like a loose mode, a loose file or another owner is refused. The
// keychain backend (5B) can seal for real later through the `SecretBox` seam.
import { randomBytes } from "node:crypto";
import { chmodSync, closeSync, fsyncSync, lstatSync, mkdirSync, openSync, readFileSync } from "node:fs";
import { open, rename, rm } from "node:fs/promises";
import { join } from "node:path";
import { checkPrivateDir, lockExclusive, makePrivateDir } from "@finisher/fin-node";
import { type Clock, type Ids, systemClock, systemIds } from "../agent/clock.ts";
import { finError } from "../errors.ts";
import { type Sealed, type SecretBox, identityBox } from "./box.ts";

/** The providers whose keys Finisher stores (v1: OpenRouter only, Owner decision 7). */
export type SecretProvider = "openrouter";

export interface SecretMeta {
  readonly id: string;
  readonly provider: SecretProvider;
  readonly label: string;
  /** Bound to a project; absent for the server's default key. */
  readonly projectId?: string;
  readonly createdAt: number;
}

export interface NewSecret {
  readonly provider: string;
  readonly label: string;
  readonly projectId?: string;
  readonly value: Buffer;
}

export interface SecretStore {
  /** Stores `value` sealed; replaces the key already bound to the same provider and project. */
  put(e: NewSecret): Promise<string>;
  /** Every entry's metadata, never a value. */
  list(): SecretMeta[];
  remove(id: string): Promise<void>;
  /** The plaintext of `id`, a fresh buffer the caller zero-fills (`ModelGateway` only). */
  reveal(id: string): Buffer;
}

interface Entry extends SecretMeta {
  readonly sealed: Sealed;
}

interface FileDoc {
  readonly v: 1;
  readonly entries: Entry[];
}

const FILE = "secrets.json";
const LOCK = "secrets.lock";
/** How long a write waits for another process's lock. */
const LOCK_WAIT_MS = 5000;
const LABEL = /^[\x20-\x7e]{1,100}$/;
const PROJECT = /^[0-9a-z]{26}$/;
/** An API key is 8–1024 bytes of printable ASCII without spaces (checked on the bytes). */
function isKey(b: Buffer): boolean {
  if (!Buffer.isBuffer(b) || b.length < 8 || b.length > 1024) return false;
  for (const c of b) if (c < 0x21 || c > 0x7e) return false;
  return true;
}

const corrupt = (message: string, details: unknown = {}) =>
  finError("SECRET_CORRUPT", message, details, "remove the entry with `finisherd provider key rm` and set it again");

const unsafe = (message: string, details: unknown = {}) =>
  finError("SECRET_STORE_UNSAFE", message, details, "make the server-data directory private to the user finisherd runs as (chmod 700, chown)");

export interface StoreOptions {
  readonly box?: SecretBox;
  readonly clock?: Clock;
  readonly ids?: Ids;
  /** The platform whose protection applies (tests take the Windows path elsewhere). */
  readonly platform?: NodeJS.Platform;
}

export class FileSecretStore implements SecretStore {
  readonly path: string;
  private readonly box: SecretBox;
  private readonly clock: Clock;
  private readonly ids: Ids;
  private readonly dir: string;
  private readonly windows: boolean;
  private queue: Promise<unknown> = Promise.resolve();

  constructor(dir: string, opts: StoreOptions = {}) {
    this.dir = dir;
    this.windows = (opts.platform ?? process.platform) === "win32";
    this.box = opts.box ?? identityBox;
    this.clock = opts.clock ?? systemClock;
    this.ids = opts.ids ?? systemIds(this.clock);
    const created = mkdirSync(dir, { recursive: true, mode: 0o700 }) !== undefined;
    if (created && this.windows) makePrivateDir(dir);
    this.path = join(dir, FILE);
    this.ensurePrivate();
  }

  /** The directory is ours, a real directory and 0700; the file, if any, ours, regular and 0600. */
  private ensurePrivate(): void {
    if (this.windows) return this.ensurePrivateWindows();
    const uid = process.getuid?.();
    const d = lstatSync(this.dir);
    if (!d.isDirectory() || d.isSymbolicLink()) throw unsafe("the secrets directory is not a directory", { what: "dir" });
    if (uid !== undefined && d.uid !== uid) throw unsafe("the secrets directory belongs to another user", { what: "dir" });
    if ((d.mode & 0o077) !== 0) chmodSync(this.dir, 0o700);
    let f;
    try {
      f = lstatSync(this.path);
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code === "ENOENT") return;
      throw e;
    }
    if (!f.isFile()) throw unsafe("secrets.json is not a regular file", { what: "file" });
    if (uid !== undefined && f.uid !== uid) throw unsafe("secrets.json belongs to another user", { what: "file" });
    if ((f.mode & 0o077) !== 0) chmodSync(this.path, 0o600);
  }

  /**
   * Windows: the same checks through the DACL. The directory is a real directory whose DACL
   * admits only this user, SYSTEM and Administrators (`checkPrivateDir`); one that also admits
   * others is made private again (`makePrivateDir`, as a loose mode is chmodded), one owned by
   * another account is refused. The file must be a regular file, not a link, and admit nobody
   * else: it inherits the directory's DACL, so an extra entry was put there on purpose.
   */
  private ensurePrivateWindows(): void {
    const d = lstatSync(this.dir);
    if (!d.isDirectory() || d.isSymbolicLink()) throw unsafe("the secrets directory is not a directory", { what: "dir" });
    let report = checkPrivateDir(this.dir);
    if (!report.ok && !report.problems.some((p) => p.startsWith("owner "))) {
      makePrivateDir(this.dir);
      report = checkPrivateDir(this.dir);
    }
    if (!report.ok) throw unsafe("other accounts can open the secrets directory", { what: "dir", problems: report.problems });
    let f;
    try {
      f = lstatSync(this.path);
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code === "ENOENT") return;
      throw e;
    }
    if (!f.isFile() || f.isSymbolicLink()) throw unsafe("secrets.json is not a regular file", { what: "file" });
    const file = checkPrivateDir(this.path);
    if (!file.ok) throw unsafe("other accounts can open secrets.json", { what: "file", problems: file.problems });
  }

  /** Runs `f` holding the cross-process lock (retried while another process holds it). */
  private async locked<A>(f: () => Promise<A>): Promise<A> {
    // Real time, not the injected clock: this waits on another process.
    const deadline = Date.now() + LOCK_WAIT_MS;
    for (;;) {
      let lock;
      try {
        lock = lockExclusive(join(this.dir, LOCK));
      } catch (e) {
        if ((e as { code?: string }).code !== "SERVER_RUNNING") throw e;
        if (Date.now() > deadline) {
          throw finError("SECRET_STORE_BUSY", "another process holds the secret store's lock", {}, "try again; only finisherd should write secrets.json");
        }
        await new Promise((r) => setTimeout(r, 20));
        continue;
      }
      try {
        return await f();
      } finally {
        lock.release();
      }
    }
  }

  put(e: NewSecret): Promise<string> {
    if (e.provider !== "openrouter") {
      return Promise.reject(
        finError("PROVIDER_UNAVAILABLE", `Finisher stores no ${String(e.provider)} key in v1`, { provider: e.provider }, "only an OpenRouter key can be set"),
      );
    }
    if (typeof e.label !== "string" || !LABEL.test(e.label)) {
      return Promise.reject(finError("SECRET_INVALID", "a label is 1–100 printable characters", { field: "label" }));
    }
    if (e.projectId !== undefined && !PROJECT.test(e.projectId)) {
      return Promise.reject(finError("SECRET_INVALID", "not a project id", { field: "projectId" }));
    }
    if (!isKey(e.value)) return Promise.reject(finError("SECRET_INVALID", "an API key is 8–1024 printable characters without spaces", { field: "value" }));
    const sealed = this.box.seal(e.value);
    const id = this.ids.ulid();
    return this.serial(async () => {
      const doc = this.read();
      const kept = doc.entries.filter((x) => !(x.provider === e.provider && x.projectId === e.projectId));
      const entry: Entry = {
        id,
        provider: "openrouter",
        label: e.label,
        ...(e.projectId === undefined ? {} : { projectId: e.projectId }),
        sealed,
        createdAt: this.clock.now(),
      };
      await this.write({ v: 1, entries: [...kept, entry] });
      return id;
    });
  }

  list(): SecretMeta[] {
    return this.read().entries.map(({ id, provider, label, projectId, createdAt }) => ({
      id,
      provider,
      label,
      ...(projectId === undefined ? {} : { projectId }),
      createdAt,
    }));
  }

  remove(id: string): Promise<void> {
    return this.serial(async () => {
      const doc = this.read();
      const kept = doc.entries.filter((x) => x.id !== id);
      if (kept.length === doc.entries.length) {
        throw finError("SECRET_NOT_FOUND", "no key with that id", { id }, "list the keys with `finisherd provider key ls`");
      }
      await this.write({ v: 1, entries: kept });
    });
  }

  reveal(id: string): Buffer {
    const entry = this.read().entries.find((x) => x.id === id);
    if (entry === undefined) throw finError("SECRET_NOT_FOUND", "no key with that id", { id });
    try {
      return this.box.open(entry.sealed);
    } catch {
      throw corrupt("a stored key cannot be opened", { id });
    }
  }

  private serial<A>(f: () => Promise<A>): Promise<A> {
    const g = () => this.locked(f);
    const run = this.queue.then(g, g);
    this.queue = run.catch(() => undefined);
    return run;
  }

  private read(): FileDoc {
    this.ensurePrivate();
    let text: string;
    try {
      text = readFileSync(this.path, "utf8");
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code === "ENOENT") return { v: 1, entries: [] };
      throw e;
    }
    let doc: FileDoc;
    try {
      doc = JSON.parse(text) as FileDoc;
    } catch {
      throw corrupt("secrets.json is not JSON");
    }
    if (doc?.v !== 1 || !Array.isArray(doc.entries)) throw corrupt("secrets.json has an unknown shape");
    for (const x of doc.entries) {
      const shaped =
        typeof x?.id === "string" &&
        x.provider === "openrouter" &&
        typeof x.label === "string" &&
        (x.projectId === undefined || typeof x.projectId === "string") &&
        typeof x.createdAt === "number" &&
        x.sealed?.v === 1 &&
        typeof x.sealed.data === "string";
      if (!shaped) throw corrupt("secrets.json holds a malformed entry", { id: typeof x?.id === "string" ? x.id : null });
    }
    return doc;
  }

  /**
   * Temp-then-rename; the temp file is created 0600 (on Windows it inherits the private
   * directory's DACL) before any byte is written; the directory is fsynced. On Windows the
   * rename is retried on sharing violations (a reader has the file open) and the directory is
   * not fsynced: Node cannot open a directory for flushing there, and NTFS journals the rename.
   */
  private async write(doc: FileDoc): Promise<void> {
    this.ensurePrivate();
    const tmp = `${this.path}.${randomBytes(6).toString("hex")}.tmp`;
    const fh = await open(tmp, "wx", 0o600);
    try {
      await fh.writeFile(`${JSON.stringify(doc, null, 2)}\n`);
      await fh.sync();
    } catch (e) {
      await fh.close();
      await rm(tmp, { force: true });
      throw e;
    }
    await fh.close();
    await this.replace(tmp);
    if (this.windows) return;
    const d = openSync(this.dir, "r");
    try {
      fsyncSync(d);
    } finally {
      closeSync(d);
    }
  }

  private async replace(tmp: string): Promise<void> {
    for (let attempt = 0; ; attempt++) {
      try {
        await rename(tmp, this.path);
        return;
      } catch (e) {
        const code = (e as NodeJS.ErrnoException).code;
        if (!this.windows || (code !== "EPERM" && code !== "EBUSY" && code !== "EACCES") || attempt >= 20) {
          await rm(tmp, { force: true });
          throw e;
        }
        await new Promise((r) => setTimeout(r, 25));
      }
    }
  }
}
