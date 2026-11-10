// `asset.reprobe` (6B-1 plan Task 11): probes a `media` asset's file again and commits one
// `asset.reprobe` op (model schema 4) with what the probe found, so an asset imported before
// schema 4 gains its display size, rotation and pixel aspect.
//
// The handler reads the branch head once (the asset, its path, its fingerprint, the model's
// schema version), probes the file through `probeFor` (the probe `asset.probe` and so `fin
// asset add` use: a restricted principal's open stays beneath its root), refuses a file whose
// fingerprint moved (`ASSET_CHANGED`: that is a relink, not a reprobe), and with `deep` runs
// `media.probeDeep` first, refusing an asset this machine cannot decode (`CODEC_UNAVAILABLE`,
// with the decoder's `failure`). It then commits the op as a client tx at the head it read,
// under the caller's `txId` and `argsHash`, through the same commit path as `tx.commit`.
// A `txId` already in the log is answered from its record before anything is probed (a retry
// whose first answer was lost meets neither a moved file nor a full job queue); a probe that
// finds the streams the model holds commits nothing (`unchanged: true`).
import { isAbsolute, join } from "node:path";
import type { FinError, JobOwner, Principal, Probe } from "@finisher/contracts";
import type { CommittedTx } from "@finisher/core-types";
import { Effect } from "effect";
import type { ChainProjects } from "../chain/resolve.ts";
import type { CallMeta } from "../diagnostics.ts";
import { finError } from "../errors.ts";
import { probeFor } from "../probe.ts";
import type { ProjectRegistryShape } from "../projects.ts";
import { type ProbeDeepDeps, probeDeep } from "./probe-deep.ts";

export interface ReprobeRequest {
  readonly projectId: string;
  readonly asset: string;
  readonly branch?: string | undefined;
  readonly deep?: boolean | undefined;
  readonly txId: string;
  readonly author: string;
  readonly dryRun?: boolean | undefined;
  readonly argsHash?: string | undefined;
}

export interface ReprobeDeps extends ProbeDeepDeps {
  readonly registry: ProbeDeepDeps["registry"] & Pick<ProjectRegistryShape, "commit" | "dirOf" | "txGet">;
  readonly projects: ChainProjects;
}

interface AssetJson {
  readonly kind?: string;
  readonly path?: string;
  readonly fingerprint?: string;
  readonly video?: unknown;
  readonly audio?: unknown;
}

interface ReprobeOp {
  readonly op?: string;
  readonly asset?: string;
  readonly video?: unknown;
  readonly audio?: unknown;
}

/** JSON with object keys sorted: equal values, equal strings. */
function canonical(v: unknown): string {
  return JSON.stringify(v, (_, x: unknown) =>
    x !== null && typeof x === "object" && !Array.isArray(x) ? Object.fromEntries(Object.entries(x as Record<string, unknown>).sort(([a], [b]) => a.localeCompare(b))) : x,
  );
}

/** The answer of a record already committed under `req.txId`, or `undefined` when there is none. */
function fromLog(req: ReprobeRequest, found: unknown): Effect.Effect<unknown, FinError> | undefined {
  const c = found as Partial<CommittedTx> | null;
  if (c === null || typeof c !== "object" || c.tx === undefined || typeof c.rev !== "number") return undefined;
  const op = (c.tx.ops?.[0] ?? {}) as ReprobeOp;
  const same = c.tx.ops?.length === 1 && op.op === "asset.reprobe" && op.asset === req.asset && c.tx.argsHash === req.argsHash;
  if (!same) {
    return Effect.fail(
      finError("IDEM_KEY_REUSED", `transaction ${req.txId} was committed by another command`, { txId: req.txId }, "run the command with a new --idem-key"),
    );
  }
  return Effect.succeed({ asset: req.asset, ...(op.video === undefined ? {} : { video: op.video }), ...(op.audio === undefined ? {} : { audio: op.audio }), rev: c.rev, txId: c.tx.txId });
}

interface ProbeDeepAnswer {
  readonly decodable?: boolean;
  readonly codec?: unknown;
  readonly failure?: unknown;
}

/**
 * `asset.reprobe`. Errors: `PROJECT_NOT_FOUND`, `BRANCH_NOT_FOUND`, `ASSET_NOT_FOUND`,
 * `ASSET_TYPE_UNSUPPORTED` (not a `media` asset), the probe's (`ASSET_UNREADABLE`,
 * `PATH_NOT_ALLOWED`, `UNSUPPORTED_CONTAINER`, …), `ASSET_CHANGED` (`reason: fingerprint`, or
 * `kind` for a file that is no longer media), with `deep` `CODEC_UNAVAILABLE` and
 * `media.probeDeep`'s, `IDEM_KEY_REUSED` for a `txId` another command committed, then the
 * commit's. Answers `{asset, video?, audio?, deep?, rev, txId}`, `unchanged: true` and the
 * head's `rev` when nothing changed, or `dryRun: true`.
 */
export function reprobe(
  deps: ReprobeDeps,
  principal: Principal | undefined,
  owner: JobOwner,
  meta: CallMeta,
  req: ReprobeRequest,
): Effect.Effect<unknown, FinError> {
  return Effect.gen(function* () {
    yield* deps.registry.ensure(req.projectId);
    if (req.dryRun !== true) {
      const logged = fromLog(req, yield* deps.registry.txGet(req.projectId, req.txId));
      if (logged !== undefined) return yield* logged;
    }
    const branch = yield* deps.registry.resolveBranch(req.projectId, req.branch ?? "main");
    const snap = yield* deps.registry.snapshot(req.projectId, branch);
    const project = (snap.project ?? {}) as { schemaVersion?: unknown; assets?: Record<string, AssetJson> };
    const assets = project.assets ?? {};
    const a = Object.hasOwn(assets, req.asset) ? assets[req.asset] : undefined;
    if (a === undefined) {
      return yield* Effect.fail(finError("ASSET_NOT_FOUND", `no asset ${req.asset} on ${branch}`, { asset: req.asset, branch }, "use the id of an existing asset"));
    }
    if (a.kind !== "media" || typeof a.path !== "string" || typeof a.fingerprint !== "string") {
      return yield* Effect.fail(finError("ASSET_TYPE_UNSUPPORTED", `asset ${req.asset} is not a media asset`, { asset: req.asset, kind: a.kind ?? null }, "reprobe a video or audio asset"));
    }
    const dir = deps.registry.dirOf(req.projectId);
    const path = isAbsolute(a.path) || dir === undefined ? a.path : join(dir, a.path);
    const probed: Probe = yield* probeFor(principal, deps.projects, req.projectId, path);
    if (probed.fingerprint !== a.fingerprint || probed.kind !== "media") {
      const reason = probed.fingerprint !== a.fingerprint ? "fingerprint" : "kind";
      return yield* Effect.fail(
        finError(
          "ASSET_CHANGED",
          `the file of asset ${req.asset} changed since it was added`,
          { asset: req.asset, reason, expected: a.fingerprint, actual: probed.fingerprint, ...(reason === "kind" ? { kind: probed.kind } : {}) },
          "relink the asset to its file, or add the new file as an asset (fin asset add)",
        ),
      );
    }
    let deep: unknown;
    if (req.deep === true) {
      deep = yield* probeDeep(deps, owner, { projectId: req.projectId, asset: req.asset, branch });
      const d = deep as ProbeDeepAnswer;
      if (d.decodable !== true) {
        return yield* Effect.fail(
          finError(
            "CODEC_UNAVAILABLE",
            `asset ${req.asset} cannot be decoded on this machine`,
            { asset: req.asset, codec: d.codec ?? null, direction: "decode", failure: d.failure ?? null },
            "see fin media caps for what this machine decodes; reprobe without --deep to record the file's geometry anyway",
          ),
        );
      }
    }
    const op = {
      op: "asset.reprobe",
      asset: req.asset,
      ...(probed.video == null ? {} : { video: probed.video }),
      ...(probed.audio == null ? {} : { audio: probed.audio }),
    };
    const answer = { asset: req.asset, ...(probed.video == null ? {} : { video: probed.video }), ...(probed.audio == null ? {} : { audio: probed.audio }), ...(deep === undefined ? {} : { deep }) };
    if (req.dryRun === true) return { ...answer, dryRun: true };
    if (canonical(a.video ?? null) === canonical(probed.video ?? null) && canonical(a.audio ?? null) === canonical(probed.audio ?? null)) {
      return { ...answer, unchanged: true, rev: snap.rev };
    }
    if (typeof project.schemaVersion !== "number") {
      return yield* Effect.fail(finError("INTERNAL", "the branch head has no schemaVersion", { projectId: req.projectId, branch }));
    }
    const tx = {
      txId: req.txId,
      author: req.author,
      branch,
      baseRev: snap.rev,
      // The head's model version is this engine's: the op exists from schema 4 on.
      schemaVersion: project.schemaVersion,
      ops: [op],
      ...(req.argsHash === undefined ? {} : { argsHash: req.argsHash }),
    };
    const committed: CommittedTx = yield* deps.registry.commit(req.projectId, tx, undefined, meta);
    return { ...answer, rev: committed.rev, txId: committed.tx.txId };
  });
}
