// A generated library tree and the scans a walker would emit over it (plan 5D Task 7b's
// property test). Names come from a pool full of collisions (NFC/NFD twins, case twins,
// invalid UTF-8 and a valid U+FFFD, dot-files); files are inodes so hard links share one.
import type { DirDone, Placeholder, ScanBatch, ScanEntry } from "@finisher/core-types";

export interface Inode {
  id: Buffer | null;
  size: number;
  mtimeNs: string;
  content: number;
}

export interface WFile {
  readonly kind: "file";
  inode: Inode;
  placeholder: Placeholder | null;
  /** Excluded by something other than its name (a glob, an automount). */
  excluded?: boolean;
}

export interface WDir {
  readonly kind: "dir";
  readonly children: Map<string, WNode>;
  unreadable: boolean;
  placeholder: boolean;
  excluded?: boolean;
}

export type WNode = WFile | WDir;

const raw = (s: string): string => Buffer.from(s, "utf8").toString("hex");

/** Raw names (hex): file and directory names share a directory's namespace. */
export const NAMES: ReadonlyArray<string> = [
  raw("a.mov"), raw("A.MOV"), raw("é.wav"), raw("é.wav"), raw("b"), "ff2e6d", "fe2e6d", raw("�.m"), raw(".hidden"), raw("c.txt"),
  raw("x"), raw("X"), raw("ü"), raw("ü"), raw("Straße"), raw("STRASSE"),
];

export type IdMode = "ids" | "none" | "zeros";

export interface WorldOptions {
  readonly caseInsensitive: boolean;
  readonly ids: IdMode;
}

export interface Located {
  readonly node: WNode;
  readonly parent: WDir | null;
  readonly name: string;
  readonly chain: string[]; // hex components of the node itself
}

const fatal = new TextDecoder("utf-8", { fatal: true });
const lossy = new TextDecoder("utf-8");

export class World {
  readonly root: WDir = { kind: "dir", children: new Map(), unreadable: false, placeholder: false };
  #inodes = 0;
  #mtime = 1_700_000_000_000_000_000n;
  readonly opts: WorldOptions;

  constructor(opts: WorldOptions) {
    this.opts = opts;
  }

  newInode(size: number): Inode {
    const n = ++this.#inodes;
    const id = this.opts.ids === "ids" ? Buffer.from(n.toString(16).padStart(16, "0"), "hex") : this.opts.ids === "zeros" ? Buffer.alloc(8) : null;
    return { id, size, mtimeNs: this.nextMtime(), content: n };
  }

  nextMtime(): string {
    this.#mtime += 1_000_000_007n;
    return this.#mtime.toString();
  }

  /** Every node, depth first by raw name (the root first). */
  all(): Located[] {
    const out: Located[] = [{ node: this.root, parent: null, name: "", chain: [] }];
    const walk = (d: WDir, chain: string[]) => {
      for (const name of [...d.children.keys()].sort()) {
        const node = d.children.get(name)!;
        out.push({ node, parent: d, name, chain: [...chain, name] });
        if (node.kind === "dir") walk(node, [...chain, name]);
      }
    };
    walk(this.root, []);
    return out;
  }

  dirs(): Located[] {
    return this.all().filter((l) => l.node.kind === "dir");
  }

  at(chain: ReadonlyArray<string>): WNode | undefined {
    let cur: WNode = this.root;
    for (const c of chain) {
      if (cur.kind !== "dir") return undefined;
      const next: WNode | undefined = cur.children.get(c);
      if (next === undefined) return undefined;
      cur = next;
    }
    return cur;
  }

  /** The display name, lossy flag and key of a component under a parent key. */
  names(nameHex: string, parentKey: string): { nameNfc: string; nameLossy: boolean; relKey: string } {
    const bytes = Buffer.from(nameHex, "hex");
    let nameLossy = false;
    try {
      fatal.decode(bytes);
    } catch {
      nameLossy = true;
    }
    const nameNfc = lossy.decode(bytes).normalize("NFC");
    const fold = this.opts.caseInsensitive ? nameNfc.toLowerCase().normalize("NFC") : nameNfc;
    return { nameNfc, nameLossy, relKey: parentKey === "" ? fold : `${parentKey}/${fold}` };
  }

  keyOf(chain: ReadonlyArray<string>): string {
    let key = "";
    for (const c of chain) key = this.names(c, key).relKey;
    return key;
  }

  entry(chain: ReadonlyArray<string>, name: string, node: WNode): ScanEntry {
    const n = this.names(name, this.keyOf(chain));
    const dir = { components: chain.map((c) => Buffer.from(c, "hex").toString("base64")) };
    const excluded = n.nameNfc.startsWith(".") ? { excluded: "DOTFILE" } : node.excluded === true ? { excluded: "GLOB" } : {};
    const dev = this.opts.ids === "none" ? {} : { dev: "64769" };
    const base = { dir, name: Buffer.from(name, "hex").toString("base64"), ...n, mountRoot: false, automount: false, ...excluded, ...dev };
    if (node.kind === "dir") {
      return { ...base, entryType: "dir", size: 0, mtimeNs: "1", ...(node.placeholder ? { placeholder: "dataless" as const } : {}) };
    }
    const { inode } = node;
    return {
      ...base, entryType: "file", size: inode.size, mtimeNs: inode.mtimeNs,
      ...(inode.id === null ? {} : { fileId: inode.id.toString("base64") }),
      ...(node.placeholder === null ? {} : { placeholder: node.placeholder }),
    };
  }

  /** One listing of the directory at `chain`: its entries (by raw name), then its `DirDone`. */
  listing(chain: ReadonlyArray<string>): { items: Item[]; subdirs: string[][] } {
    const d = this.at(chain) as WDir;
    const rel = { components: chain.map((c) => Buffer.from(c, "hex").toString("base64")) };
    if (d.unreadable) return { items: [{ done: { dir: rel, complete: false, entries: 0, code: "EACCES" } }], subdirs: [] };
    const items: Item[] = [];
    const subdirs: string[][] = [];
    for (const name of [...d.children.keys()].sort()) {
      const node = d.children.get(name)!;
      const e = this.entry(chain, name, node);
      items.push({ entry: e });
      if (node.kind === "dir" && e.excluded == null) subdirs.push([...chain, name]);
    }
    items.push({ done: { dir: rel, complete: true, entries: d.children.size, mtimeNs: this.nextMtime() } });
    return { items, subdirs };
  }

  /**
   * A full scan, breadth first by depth then raw name. A placeholder directory is closed
   * incomplete and not descended; `skip(chain)` skips a listing (a resumed scan) but still
   * descends into `known` subdirectories.
   */
  fullScan(skip?: (chain: string[]) => boolean, known?: (chain: string[]) => boolean): Item[] {
    const out: Item[] = [];
    const queue: string[][] = [[]];
    while (queue.length > 0) {
      const chain = queue.shift()!;
      const d = this.at(chain) as WDir;
      if (d.placeholder) {
        out.push({ done: { dir: { components: chain.map((c) => Buffer.from(c, "hex").toString("base64")) }, complete: false, entries: 0, code: "PLACEHOLDER" } });
        continue;
      }
      if (skip?.(chain) === true) {
        for (const name of [...d.children.keys()].sort()) {
          const sub = [...chain, name];
          if (d.children.get(name)!.kind === "dir" && known?.(sub) === true) queue.push(sub);
        }
        continue;
      }
      const l = this.listing(chain);
      out.push(...l.items);
      queue.push(...l.subdirs);
    }
    return out;
  }
}

export type Item = { readonly entry: ScanEntry } | { readonly done: DirDone };

/** Cuts a stream into batch files of `sizes[i % n]` items (each file one `ScanBatch` line). */
export function cut(items: ReadonlyArray<Item>, sizes: ReadonlyArray<number>): ScanBatch[][] {
  const files: ScanBatch[][] = [];
  let i = 0;
  let k = 0;
  while (i < items.length) {
    const n = Math.max(1, sizes[k++ % sizes.length] ?? 3);
    const chunk = items.slice(i, i + n);
    i += n;
    files.push([
      {
        seq: files.length + 1,
        entries: chunk.flatMap((x) => ("entry" in x ? [x.entry] : [])),
        dirsDone: chunk.flatMap((x) => ("done" in x ? [x.done] : [])),
      },
    ]);
  }
  return files;
}

/** Every path in the world now: `d:<chain>` for directories, `f:<chain>|<name>` for files (hex). */
export function paths(w: World): Set<string> {
  const out = new Set<string>();
  for (const l of w.all().slice(1)) {
    if (l.node.kind === "dir") out.add(`d:${l.chain.join("/")}`);
    else out.add(`f:${l.chain.slice(0, -1).join("/")}|${l.name}`);
  }
  return out;
}

/** The fingerprint a probe computes for a file's content (fp1 covers size and mtime). */
export const fpOf = (inode: Inode): string => `fp1:${inode.content}:${inode.size}:${inode.mtimeNs}`;
