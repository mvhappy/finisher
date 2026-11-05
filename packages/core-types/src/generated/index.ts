/* eslint-disable */
/**
 * Generated from schema/fin-core.schema.json by scripts/generate.mjs.
 * Do not edit by hand: change the Rust types and run `pnpm --filter @finisher/core-types generate`.
 */

/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ChangeKind".
 */
export type ChangeKind = "added" | "removed" | "updated";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EntityKind".
 */
export type EntityKind = "asset" | "track" | "clip" | "timeline";
/**
 * Exact rational seconds in canonical form: "n/d" in lowest terms with d > 1, or "n" when d is 1; no leading zeros, no "+", no "-0".
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Rational".
 */
export type Rational = string;
/**
 * A primitive edit operation, tagged by `op`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Op".
 */
export type Op =
  | {
      audio?: AudioInfo | null;
      duration: Rational;
      fingerprint: string;
      id: string;
      op: "asset.add";
      path: string;
      video?: VideoInfo | null;
    }
  | {
      asset: string;
      at: Rational;
      duration: Rational;
      id: string;
      op: "clip.insert";
      sourceIn: Rational;
      track: string;
    }
  | {
      clip: string;
      delta: Rational;
      edge: Edge;
      op: "clip.trim";
    }
  | {
      clip: string;
      op: "clip.delete";
    }
  | {
      id: string;
      op: "asset.remove";
    }
  | {
      id: string;
      op: "clip.remove";
      ripple: boolean;
    }
  | {
      clip: Clip;
      op: "clip.restore";
      ripple?: boolean;
      track: string;
    };
/**
 * Which edge of a clip a trim moves.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Edge".
 */
export type Edge = "start" | "end";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TrackKind".
 */
export type TrackKind = "video" | "audio";
/**
 * A record's kind and what it points at (spec §4.2). Serialised (in `CommittedTx.link`) as
 * `{"kind": "<kind>", …fields}`, camelCase, absent optional fields omitted.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Link".
 */
export type Link =
  | {
      kind: "edit";
    }
  | {
      kind: "undo";
      undoes: string[];
    }
  | {
      kind: "redo";
      redoes: string;
    }
  | {
      from: string;
      kind: "merge";
      mergeId: string;
      origin: string;
    }
  | {
      from: string;
      kind: "cherryPick";
      origin: string;
      originHash?: string | null;
    }
  | {
      kind: "revert";
      reverts: string;
    }
  | {
      kind: "checkpoint";
      name: string;
    }
  | {
      branchId: string;
      forkHash?: string | null;
      forkSeq: number;
      kind: "branch";
      name: string;
    }
  | {
      archived?: boolean | null;
      branchId: string;
      kind: "branchSet";
      name?: string | null;
    };
/**
 * One engine event (`Engine.onEvent`), tagged by `type`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EngineEvent".
 */
export type EngineEvent =
  | {
      tx: CommittedTx;
      type: "applied";
    }
  | {
      status: unknown;
      type: "status";
    }
  | {
      branch: string;
      fromRev: number;
      generation: number;
      incarnation: string;
      own: boolean;
      quarantined: number;
      type: "rewound";
    }
  | {
      incarnation: string;
      type: "rebuilt";
    }
  | {
      error: CoreError;
      fatal: boolean;
      type: "error";
    }
  | {
      dropped: number;
      type: "lagged";
    };

export interface FinCore {
  [k: string]: unknown;
}
/**
 * The result of applying a transaction to a project.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Applied".
 */
export interface Applied {
  /**
   * Placeholder (`$name`) to allocated id.
   */
  bindings: {
    [k: string]: string;
  };
  diff: Diff;
  inverse: Op[];
  project: Project;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Diff".
 */
export interface Diff {
  changes: Change[];
  shifts: Shift[];
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Change".
 */
export interface Change {
  change: ChangeKind;
  id: string;
  kind: EntityKind;
}
/**
 * Items on `track` at or after `from` moved by `by` (ripple).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Shift".
 */
export interface Shift {
  by: Rational;
  from: Rational;
  track: string;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AudioInfo".
 */
export interface AudioInfo {
  channels: number;
  sampleRate: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "VideoInfo".
 */
export interface VideoInfo {
  height: number;
  rate: Rational;
  width: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Clip".
 */
export interface Clip {
  asset: string;
  duration: Rational;
  id: string;
  sourceIn: Rational;
  start: Rational;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Project".
 */
export interface Project {
  assets: {
    [k: string]: Asset;
  };
  id: string;
  name: string;
  rate: Rational;
  schemaVersion: number;
  timeline: Timeline;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Asset".
 */
export interface Asset {
  audio?: AudioInfo | null;
  duration: Rational;
  fingerprint: string;
  id: string;
  path: string;
  video?: VideoInfo | null;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Timeline".
 */
export interface Timeline {
  id: string;
  tracks: Track[];
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Track".
 */
export interface Track {
  id: string;
  /**
   * Sorted by `start`, non-overlapping.
   */
  items: Clip[];
  kind: TrackKind;
}
/**
 * A transaction as persisted: with its revision, resolved bindings and diff.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CommittedTx".
 */
export interface CommittedTx {
  bindings: {
    [k: string]: string;
  };
  diff: Diff;
  /**
   * The record's kind and links; omitted for an `edit`. Boxed: most txs are edits, and a
   * link would double the size of every `CommittedTx`.
   */
  link?: Link | null;
  rev: number;
  tx: Tx;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Tx".
 */
export interface Tx {
  /**
   * What produced the tx, for a client that sends an idempotency key as `txId`:
   * `blake3:<64 hex>` of the command input (op kinds and canonical args, before they were
   * resolved against a snapshot). A retry rebuilds `ops` against a newer head (positions
   * move; the stored tx is rebased), so `ops` cannot tell a retry from another command;
   * this can. The store refuses a `txId` it holds with another `argsHash`
   * (`IDEM_KEY_REUSED`). Stored with the record, so every server that imported it checks.
   */
  argsHash?: string | null;
  author: string;
  baseRev: number;
  branch: string;
  ops: Op[];
  schemaVersion: number;
  txId: string;
}
/**
 * A structured, machine-readable error. `code` is a stable `SCREAMING_SNAKE` string.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CoreError".
 */
export interface CoreError {
  code: string;
  details: unknown;
  fix?: string | null;
  message: string;
}
/**
 * A branch generation as a caller holds it: the database's incarnation and the branch's
 * rewind count in it (module docs).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Generation".
 */
export interface Generation {
  incarnation: string;
  n: number;
}
/**
 * What the server needs to register a media asset. Its serde form is what `asset.probe`
 * answers (`@finisher/core-types` exports it as `Probe`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Probe".
 */
export interface Probe {
  audio?: AudioInfo | null;
  duration: Rational;
  fingerprint: string;
  size: number;
  video?: VideoInfo | null;
}
/**
 * A record kept in quarantine ([`TxStatus::SetAside`](crate::TxStatus)). `reason`:
 * `SUPERSEDED`, `DIVERGED_TAIL`, `SALVAGED`, `MISSING_FROM_FOLDER`, `STALE_EPOCH`,
 * `MALFORMED`, `CONFLICT_COPY`; `origin`: `local`, `folder`, `foreign`, `salvage`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SetAside".
 */
export interface SetAside {
  branch: string;
  origin: string;
  reason: string;
  txId: string;
}
/**
 * `tx.get` for a transaction that is not (or no longer) on its branch.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TxSetAside".
 */
export interface TxSetAside {
  setAside: SetAside;
}
