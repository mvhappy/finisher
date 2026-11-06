/* eslint-disable */
/**
 * Generated from schema/fin-core.schema.json by scripts/generate.mjs.
 * Do not edit by hand: change the Rust types and run `pnpm --filter @finisher/core-types generate`.
 */

/**
 * Exact rational seconds in canonical form: "n/d" in lowest terms with d > 1, or "n" when d is 1; no leading zeros, no "+", no "-0".
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Rational".
 */
export type Rational = string;
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ChangeKind".
 */
export type ChangeKind = "added" | "removed" | "updated";
/**
 * Which edge of a clip a trim moves.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Edge".
 */
export type Edge = "start" | "end";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EntityKind".
 */
export type EntityKind = "asset" | "track" | "clip" | "timeline";
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
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TrackKind".
 */
export type TrackKind = "video" | "audio";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BranchState".
 */
export type BranchState = "active" | "archived" | "merged";
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
      /**
       * A recovered record's quarantined original (plan 4B Decision 27).
       */
      originHash?: string | null;
    }
  | {
      branchId: string;
      epoch: number;
      forkHash?: string | null;
      forkSeq: number;
      kind: "branch";
      name: string;
      /**
       * A recovered record's quarantined original (plan 4B Decision 27).
       */
      originHash?: string | null;
    }
  | {
      archived?: boolean | null;
      branchId: string;
      kind: "branchSet";
      name?: string | null;
      /**
       * A recovered record's quarantined original (plan 4B Decision 27).
       */
      originHash?: string | null;
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
/**
 * Where an undo takes a unit's inverse from ([`UnitEntry::source`]).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "UnitSource".
 */
export type UnitSource =
  | {
      kind: "records";
    }
  | {
      kind: "redo";
      txId: string;
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
 * What a transaction changed, op by op: each entry names the op that recorded it (`op`),
 * and both lists are in op order. An op may record any number of changes and shifts.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Diff".
 */
export interface Diff {
  changes: Change[];
  shifts: Shift[];
}
/**
 * One entity an op changed (plan 4C Decision 4).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Change".
 */
export interface Change {
  /**
   * Content offset of a start-edge move: every item-relative time of the entity moved by
   * it (plan 4C Decision 13).
   */
  by?: Rational | null;
  change: ChangeKind;
  /**
   * The edge an `updated` change moved, if that is all it did to the entity.
   */
  edge?: Edge | null;
  id: string;
  kind: EntityKind;
  /**
   * The index of the op in its transaction that made the change.
   */
  op: number;
  /**
   * The parameter an `updated` change set (a registry path), if that is all it did.
   */
  path?: string | null;
}
/**
 * Items on `track` at or after `from` moved by `by` (ripple), recorded by op `op` of the
 * transaction.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Shift".
 */
export interface Shift {
  by: Rational;
  from: Rational;
  op: number;
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
 * The answer of [`Store::create_branch`](crate::Store::create_branch): the new branch and
 * its `branch` record on the parent. `dry_run`: planned and rolled back, nothing kept (a
 * retry of a committed create answers with that create, `dry_run` false).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BranchCreated".
 */
export interface BranchCreated {
  branch: BranchView;
  dryRun: boolean;
  record: CommittedTx;
}
/**
 * A branch as listings show it ([`Store::branches`](crate::Store::branches), `branch.list`):
 * `display` is the reference that names it (its name, or `<name>#<id>` for an archived
 * branch or the second holder of a duplicate name); `head` its head seq (its fork seq until
 * its first record).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BranchView".
 */
export interface BranchView {
  createdBy: string;
  display: string;
  forkSeq: number;
  head: number;
  id: string;
  name: string;
  parent?: string | null;
  state: BranchState;
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
 * One op that cannot be carried to the head: which op of which tx, why (a reason code), the
 * record to blame when there is one (else null), and what to do. A re-run failure keeps its
 * error's `details` (an `INVALID_RANGE`'s `min` and `max`, say); other conflicts have none.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Conflict".
 */
export interface Conflict {
  againstTx?: string | null;
  /**
   * Boxed: rarely present, and it keeps `Result<_, Conflict>` small.
   */
  details?: {
    [k: string]: unknown;
  };
  fix: string;
  opIndex: number;
  reason: string;
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
 * The answer of a history operation ([`Store::undo`](crate::Store::undo),
 * [`Store::redo`](crate::Store::redo)): the records it committed, in order (or planned and
 * rolled back, `dry_run`; a retry of a committed operation answers with its records,
 * `dry_run` false), and what they act on: the units an undo undoes, the undo a redo
 * reverses, or the merge group a revert reverts. `no_op`: why a cherry-pick or revert
 * committed nothing (`ALREADY_PRESENT`: the target holds the record's origin root;
 * `ALREADY_REVERTED`: the merge group's effect is gone there); absent otherwise.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "HistoryResult".
 */
export interface HistoryResult {
  committed: CommittedTx[];
  dryRun: boolean;
  noOp?: string | null;
  undone: string[];
}
/**
 * The answer of [`Store::merge`](crate::Store::merge): merge group `merge_id`'s records on
 * the target, in order (or planned and rolled back, `dry_run`); `merged`: the source txIds
 * they copy; `skipped`: the candidates that conflict and were left on the source (with
 * `skipConflicting`; they are offered again by the next merge); `excluded`: source records
 * never offered (`UNDOES_TARGET_RECORDS`); `source_state`: the source branch's state after
 * the merge. A retry of a committed merge answers from the log: its records and their
 * origins, `skipped` and `excluded` empty, `dry_run` false. Nothing left to merge is an
 * empty answer, not an error.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "MergeReport".
 */
export interface MergeReport {
  committed: CommittedTx[];
  dryRun: boolean;
  excluded: Conflict[];
  mergeId: string;
  merged: string[];
  skipped: Conflict[];
  sourceState: BranchState;
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
 * One group of quarantined records ([`Store::recover_groups`](crate::Store::recover_groups)).
 *
 * - `id`: the first record's `txHash`, 12 hex digits (stable on every server);
 * - `records_branch`: the branch the records were on;
 * - `branch`, `fork_seq`, `fork_hash`: the fork `create` uses: an existing branch, the seq
 *   and its record; or, with `fork_group`, the branch that group makes (`branch` its name,
 *   `fork_hash` absent: its record is written then); none for a group that cannot be made
 *   for want of a fork (`NO_COMMON_HISTORY`);
 * - `records`, `reasons`: how many, and per quarantine reason; `origin`: the first row's;
 * - `server`: the writer's server id (`unknown` when it cannot be told);
 * - `first_committed_at`: unix ms; `name`: the branch `create` makes (empty for a group
 *   that makes none: it has neither records with ops nor checkpoints, and its child groups
 *   fork where it does);
 * - `children`: the groups forking from it, or made by its `branch` records;
 * - `recoverable`, `problems`: why `create` leaves it whole (fix each, or dismiss it);
 * - `notes`: rows `create` leaves in quarantine while making the rest (`NAME_TAKEN`,
 *   `BRANCH_NOT_FOUND`, …; they come back as a group of their own), and copies whose source
 *   is not here (`SOURCE_NOT_FOUND`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RecoverGroup".
 */
export interface RecoverGroup {
  branch: string;
  children: string[];
  firstCommittedAt: number;
  forkGroup?: string | null;
  forkHash?: string | null;
  forkSeq: number;
  id: string;
  name: string;
  notes: Conflict[];
  origin: string;
  problems: Conflict[];
  reasons: {
    [k: string]: number;
  };
  records: number;
  recordsBranch: string;
  recoverable: boolean;
  server: string;
}
/**
 * [`Store::recover`](crate::Store::recover)'s answer: the groups as found (before `create`),
 * the branches made and every record written (`branch` records on the parents, the copies,
 * checkpoints, `branchSet` records), in order, for publishing; `imported`: quarantine rows
 * `import_dir` added or made current again, and `skipped_files` the files it did not read;
 * `dismissed`: rows `dismiss` set aside from listing and counting; `manual`: the procedure
 * for a read-only state recovery cannot lift by itself (`CHAIN_FORK`, `CONFLICT_COPY`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RecoverReport".
 */
export interface RecoverReport {
  committed: CommittedTx[];
  created: BranchView[];
  dismissed: number;
  dryRun: boolean;
  groups: RecoverGroup[];
  imported: number;
  manual?: string | null;
  skippedFiles: SkippedFile[];
}
/**
 * A file `import_dir` did not read: its path and why (`not a segment: …`, `another
 * project`, `unreadable: …`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SkippedFile".
 */
export interface SkippedFile {
  path: string;
  reason: string;
}
/**
 * A record kept in quarantine ([`TxStatus::SetAside`](crate::TxStatus)). `reason`:
 * `SUPERSEDED`, `DIVERGED_TAIL`, `SALVAGED`, `MISSING_FROM_FOLDER`, `STALE_EPOCH`,
 * `MALFORMED`, `CONFLICT_COPY`, `ID_COLLISION`, `IMPORTED`; `origin`: `local`, `folder`,
 * `foreign`, `salvage`.
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
 * An author's undo and redo stacks on a branch, bottom first (the top is the last entry).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Stacks".
 */
export interface Stacks {
  redo: UnitEntry[];
  undo: UnitEntry[];
}
/**
 * One entry of an undo or redo stack.
 *
 * - `id`: what an `undo` names in `undoes` (a record's txId, a merge group's `mergeId`, the
 *   redo record's txId for a redo unit), or, on the redo stack, the undo record's txId;
 * - `records`: the txIds of its records, oldest first: whose stored `inverse` an undo (or,
 *   for an undo record, a redo) takes;
 * - `last_seq`: the seq of its last record, the frame its inverses apply in.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "UnitEntry".
 */
export interface UnitEntry {
  id: string;
  lastSeq: number;
  records: string[];
  /**
   * The record whose stored `inverse` an undo uses: the unit's records, or the redo
   * record that recreated it (Decision 23).
   */
  source:
    | {
        kind: "records";
      }
    | {
        kind: "redo";
        txId: string;
      };
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
