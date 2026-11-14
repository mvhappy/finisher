// The closed scope vocabulary (spec §4.5, plan 5A Decision 18), every RPC's rule (`RPC_SCOPES`:
// its scopes, all required; its resolvers; its escalations; the payload fields naming the
// branch it writes) and the human-only scopes (spec §4.6). Defaults per principal kind are
// `defaultScopes` in `auth.ts` (Decision 20).
import { Schema } from "effect";
import type { Rpc, RpcGroup } from "effect/rpc";
import type { PrincipalKind } from "./auth.ts";
import type { FinRpcs } from "./rpc.ts";

export const SCOPES = [
  "public",
  "server.read",
  "project.read",
  "project.create",
  "project.write",
  "project.admin",
  "project.confirm",
  "fonts.fetch",
  "branch.protected",
  "review.write",
  "review.decide",
  "jobs.read",
  "jobs.submit",
  "jobs.control",
  "library.read",
  "library.scan",
  "library.admin",
  "diagnostics.read",
  "diagnostics.record",
  "diagnostics.admin",
  "settings.admin",
  "auth.admin",
  "plugin.read",
  "plugin.install",
  "plugin.admin",
  "plugin.suggest",
  "agent.read",
  "agent.run",
  "agent.approve",
  "agent.admin",
] as const;

export const ScopeSchema = Schema.Literals(SCOPES);
export type Scope = typeof ScopeSchema.Type;

/** Scopes only a person holds (spec §4.6): never `local`, `agent` or `mcp`. */
export const HUMAN_ONLY: ReadonlySet<Scope> = new Set<Scope>([
  "project.confirm",
  "library.admin",
  "diagnostics.admin",
  "settings.admin",
  "auth.admin",
  "plugin.install",
  "plugin.admin",
  // Sub-project 7 (§3.5): answering approvals is a person's; deleting sessions and the provider
  // settings are the owner's. `agent.run` and `agent.read` are not human-only: `fin` drives
  // sessions as `local`, and 7B lets a parent agent drive its own children.
  "agent.approve",
  "agent.admin",
]);

/** The `library.set` fields only a person changes (`library.admin`; the only ones audited). */
export const LIBRARY_ADMIN_FIELDS = ["hydrate", "bandwidthMiBs", "acceptVolume", "acceptUnmounted", "acceptEmptyRoot", "aliases"] as const;

export type FinRpcTag = RpcGroup.Rpcs<typeof FinRpcs>["_tag"];

/**
 * What the chain resolves before it authorises (spec §4.4 stage 3):
 * - `project`: the payload's project (`projectId`, `fonts.fetch.pinned`), checked for reach;
 * - `openDir` / `createDir`: `dir`, canonicalised; `openDir` reads its project's id
 *   (`finisher.project.json`) and whether that project is open here;
 * - `probePath` / `importDir`: the path, canonical and absolute (`INVALID_PATH` otherwise);
 * - `outDir`: the payload's `out`, an absolute folder the call writes files into (optional),
 *   canonical; for an agent or MCP principal it must lie inside the call's project folder, for
 *   a device inside the server's allowed roots (5B Decision 32);
 * - `libraryRoot` / `registryPaths`: `library.add`'s root, `registry.add`'s folder and root
 *   file: canonical, checked against a device's roots, never rewritten;
 * - `branch`: the payload's branch references (a name, an id or `name#id`), as ids through
 *   the store, when a policy reads them (5A: the write targets of `agent`/`mcp` principals
 *   with branch limits; 5E: protected branches);
 * - `jobProject`: the payload's `project` (a job's, 5C-2), checked for reach; unlike `project` it
 *   opens nothing, so it adds no reopen rule;
 * - `jobKind`: the payload's `kind` as that kind's own scopes (`RuleFacts.kindScopes`), for a
 *   kind clients may submit;
 * - `job`: the payload's `job` as whose it is to the caller (`RuleFacts.job`);
 * - `review` (5E): the payload's `itemId` as the item's project (reach), `source` and `target`
 *   branch ids (added to the resolved payload, so `writes: ["target"]` finds them); an unknown
 *   item and one outside the caller's reach are both `REVIEW_NOT_FOUND`. A rule with
 *   `protectedNeeds` also gets `RuleFacts.protectedTarget` from the project's policy.
 */
/** `libraryRoot`, `registryPaths`: `library.add`'s root and `registry.add`'s folder and root file, checked against a device's roots (5B Decision 32). */
/** `pluginCommand`: `plugin.command.run`'s `name` as whether that command writes (`RuleFacts.commandWrites`). */
export type ResolverName = "project" | "openDir" | "createDir" | "probePath" | "importDir" | "outDir" | "libraryRoot" | "registryPaths" | "branch" | "jobProject" | "jobKind" | "job" | "review" | "pluginCommand";

/** Resolved facts an escalation may read. */
export interface RuleFacts {
  /** `project.open`: the folder's project is already open on this server (attaching). */
  readonly attached?: boolean;
  /**
   * A project the call names by id is not open here, so serving it reopens it from disk
   * (taking its lease): that needs `project.write` whatever the RPC's own scopes (Decision 15).
   */
  readonly reopens?: boolean;
  /** `jobs.submit`: the scopes of the kind named, a kind clients may submit (5C-2 Decision 5). */
  readonly kindScopes?: ReadonlyArray<Scope>;
  /**
   * The job a call names, as the caller sees it (5C-2 Decision 4): `own`, `other` (another
   * owner's, visible through `jobs.control`) or `none` (unknown, or not visible to the caller).
   */
  readonly job?: "own" | "other" | "none";
  /**
   * The branch a call with `protectedNeeds` writes into (`into`, or a review item's target) is
   * protected by the project's policy (5E Decision 17); absent when it could not be resolved.
   */
  readonly protectedTarget?: boolean;
  /**
   * `plugin.command.run`: the named command of an enabled plugin returns a transaction (its
   * manifest's `writes`), so running it needs `project.write` (8B Task 9). Absent for a name
   * that resolves to no command (the handler answers `PLUGIN_NOT_FOUND`).
   */
  readonly commandWrites?: boolean;
}

/** Acting on one job: one's own (or one the caller cannot see) takes `jobs.submit`, another's `jobs.control`. */
const jobControl = (facts: RuleFacts): ReadonlyArray<Scope> => (facts.job === "other" ? ["jobs.control"] : ["jobs.submit"]);

/** The decoded payload of RPC `T`. */
export type PayloadOf<T extends FinRpcTag> = Rpc.Payload<Rpc.ExtractTag<RpcGroup.Rpcs<typeof FinRpcs>, T>>;

/** The rule of RPC `T`: its escalation reads `T`'s own payload type. */
export interface RuleOf<T extends FinRpcTag> {
  /** Required, all of them (Decision 18). */
  readonly scopes: ReadonlyArray<Scope>;
  readonly resolve?: ReadonlyArray<ResolverName>;
  /** More scopes this call needs, from its payload and the resolved facts (Decision 21). */
  readonly escalate?: (payload: PayloadOf<T>, facts: RuleFacts) => ReadonlyArray<Scope>;
  /**
   * Set exactly on the RPCs that write (every call whose handler passes a write guard to the
   * store; `WRITE_RPCS`): the payload fields (dotted paths) naming a branch the call writes to,
   * for the policy stage. `[]`: it writes, but names no existing branch (a new branch, a
   * recovery): the guard still limits what it may change.
   */
  readonly writes?: ReadonlyArray<string>;
  /**
   * Branches the call changes state on without writing a record through the store's guard
   * (`project.acknowledgeRewind`): the payload fields naming them, checked by the policy stage
   * like `writes` (agent branch limits; review of Task 6, finding 7).
   */
  readonly targets?: ReadonlyArray<string>;
  /**
   * The scope that takes the place of `branch.protected` for a write into a protected branch
   * (5E Decision 16): `review.decide` for `history.drop` and the calls that decide a review.
   */
  readonly protectedNeeds?: Scope;
  /**
   * The payload field holding the author a new branch records (`createdBy`): for a principal
   * with branch limits the server sets it to the principal's own author, so `"own"` cannot be
   * claimed by naming someone else (review of Task 5, M4).
   */
  readonly author?: string;
  /**
   * Whenever a call needs a human-only scope, a `console` principal must send the challenge code
   * of a fresh `console.challenge` of the same connection (`challenge`; Decision 30), or it is
   * `CONFIRMATION_REQUIRED`: on by default (owner decision, review of Task 7, L2). A rule opts
   * out only as `readOnly` (it changes nothing: reading content, asking for a code, listing the
   * audit) or `service` (`server.stop`, which service managers call without a terminal, spec
   * §4.7). A `device` (5B) confirms exactly the `service` rules: the opt-out is the console's,
   * and stopping the server from the app takes the person's code (`needsConfirmation`).
   */
  readonly unconfirmed?: "readOnly" | "service";
  /**
   * A payload-dependent opt-out, read like `unconfirmed` when it answers (5B Decision 21: a
   * console's `self` pairing offer, which the desktop shell's main process makes without a
   * terminal). `undefined`: the rule's own `unconfirmed` stands.
   */
  readonly unconfirmedIf?: (payload: PayloadOf<T>) => "readOnly" | "service" | undefined;
  /**
   * The principal kinds that may call it, whatever scopes they hold; any other kind is
   * `SCOPE_DENIED` (reason `principalKind`), before the human-only check. `agent.self` is the
   * calling session's own, so only an `agent` credential has one; in 7A the session-driving
   * calls are a person's, so an `agent` credential cannot hold them (7B opens them to a parent
   * for its own children).
   */
  readonly kinds?: ReadonlyArray<PrincipalKind>;
  /**
   * True for a payload only a `device` may send (the app's onboarding question: training on).
   * Every other principal is `CONSENT_UI_ONLY`, before the human-only check (Decision 21).
   */
  readonly deviceOnly?: (payload: PayloadOf<T>) => boolean;
  /**
   * True for a payload that acts beyond any one project (`jobs.pause|resume {all}`,
   * `jobs.workers`): only a principal whose `projects` is `"*"` may send it; any other is
   * `SCOPE_DENIED {reason: reach}`, after its scopes are checked (5C-2 review M2).
   */
  readonly allProjects?: (payload: PayloadOf<T>) => boolean;
}

/** A rule with its RPC's types erased, as the chain reads it. */
export interface RpcRule {
  readonly scopes: ReadonlyArray<Scope>;
  readonly resolve?: ReadonlyArray<ResolverName>;
  readonly escalate?: (payload: never, facts: RuleFacts) => ReadonlyArray<Scope>;
  readonly writes?: ReadonlyArray<string>;
  readonly targets?: ReadonlyArray<string>;
  readonly protectedNeeds?: Scope;
  readonly author?: string;
  readonly unconfirmed?: "readOnly" | "service";
  readonly unconfirmedIf?: (payload: never) => "readOnly" | "service" | undefined;
  readonly kinds?: ReadonlyArray<PrincipalKind>;
  readonly deviceOnly?: (payload: never) => boolean;
  readonly allProjects?: (payload: never) => boolean;
}

const PROJECT: ReadonlyArray<ResolverName> = ["project"];
const BRANCH: ReadonlyArray<ResolverName> = ["project", "branch"];
const REVIEW: ReadonlyArray<ResolverName> = ["review"];
/** Who drives agent sessions in 7A: people, through `fin` (`local`), the console and the app (Decision 15). */
const SESSION_DRIVERS: ReadonlyArray<PrincipalKind> = ["local", "console", "device"];
/** The only kind with a session of its own. */
const SESSION_OWN: ReadonlyArray<PrincipalKind> = ["agent"];
/**
 * Who drives a session's children (7B Decision 16): people, and a parent session, which
 * `agentChainHooks().authorize` admits for its own children only.
 */
const CHILD_DRIVERS: ReadonlyArray<PrincipalKind> = [...SESSION_DRIVERS, "agent"];
/** People's own principals (7B Decision 7: rule writes and project trust; never `local`, `agent`, `mcp`). */
const PEOPLE: ReadonlyArray<PrincipalKind> = ["console", "device"];

/**
 * Every RPC's rule (spec §4.5 and §4.8); the mapped type makes a missing RPC a type error and
 * types each escalation by its RPC's payload; `scopes.test.ts` compares the keys with `FinRpcs`
 * and the writers with `WRITE_RPCS`.
 */
export const RPC_SCOPES: { readonly [T in FinRpcTag]: RuleOf<T> } = {
  "server.hello": { scopes: ["public"] },
  "auth.prove": { scopes: ["public"] },
  "auth.ticket": { scopes: ["server.read"] },
  "auth.whoami": { scopes: ["server.read"] },
  // No id: a credential can revoke only itself.
  "auth.revokeSelf": { scopes: ["server.read"] },
  // The typed confirmations (Decision 30); the handler serves the human principals only
  // (`console` and, since 5B, `device`).
  "console.challenge": { scopes: ["auth.admin"], unconfirmed: "readOnly" },
  // 5A Task 8. Stopping is the owner's, but service managers call it without a terminal.
  "server.status": { scopes: ["server.read"] },
  "server.stop": { scopes: ["auth.admin"], unconfirmed: "service" },
  "auth.audit.list": { scopes: ["auth.admin"], unconfirmed: "readOnly" },
  "auth.audit.purge": { scopes: ["auth.admin"] },
  // 5A Task 9: minting and revoking are the owner's, with a challenge code; listing reads only.
  "auth.credential.mint": { scopes: ["auth.admin"] },
  "auth.credential.list": { scopes: ["auth.admin"], unconfirmed: "readOnly" },
  "auth.credential.revoke": { scopes: ["auth.admin"] },
  // Review of Tasks 8–9, M3: a new `credential.master`, which ends every credential.
  "auth.credential.rotateMaster": { scopes: ["auth.admin"] },
  "project.create": { scopes: ["project.create"], resolve: ["createDir"] },
  // Attaching to a project already open here only reads it; loading one may take its lease.
  "project.open": {
    scopes: ["project.read"],
    resolve: ["openDir"],
    escalate: (p, facts) => [...(facts.attached === true ? [] : (["project.write"] as const)), ...(p.relocate === true ? (["project.confirm"] as const) : [])],
  },
  // Protocol 2: the probe is for `projectId`; an agent or MCP principal probes only inside its folder.
  "asset.probe": { scopes: ["project.write"], resolve: ["project", "probePath"] },
  "tx.commit": { scopes: ["project.write"], resolve: BRANCH, writes: ["tx.branch"] },
  "snapshot.get": { scopes: ["project.read"], resolve: BRANCH },
  "ops.subscribe": { scopes: ["project.read"], resolve: BRANCH },
  "project.status": { scopes: ["project.read"], resolve: PROJECT },
  // Ends every client's streams of the project (they reconnect, which reopens it).
  "project.close": { scopes: ["project.admin"], resolve: PROJECT },
  "project.takeover": { scopes: ["project.admin"], resolve: PROJECT, escalate: (p) => (p.confirm ? ["project.confirm"] : []) },
  "project.verify": { scopes: ["project.read"], resolve: PROJECT },
  "project.rebuild": { scopes: ["project.admin"], resolve: PROJECT },
  "project.compact": { scopes: ["project.admin"], resolve: PROJECT },
  // Recovery makes new branches and may re-apply a `branchSet` to a live one: the guard limits it.
  "project.recover": { scopes: ["project.admin"], resolve: ["project", "importDir"], writes: [], author: "author" },
  // A new branch is the writer's own; its parent is only read.
  "branch.create": { scopes: ["project.write"], resolve: BRANCH, writes: [], author: "author" },
  "branch.list": { scopes: ["project.read"], resolve: PROJECT },
  "branch.set": { scopes: ["project.write"], resolve: BRANCH, writes: ["branch"] },
  "history.undo": { scopes: ["project.write"], resolve: BRANCH, writes: ["branch"] },
  "history.redo": { scopes: ["project.write"], resolve: BRANCH, writes: ["branch"] },
  "history.checkpoint": { scopes: ["project.write"], resolve: BRANCH, writes: ["branch"] },
  "history.merge": { scopes: ["project.write"], resolve: BRANCH, writes: ["into"] },
  "history.cherryPick": { scopes: ["project.write"], resolve: BRANCH, writes: ["into"] },
  "history.revert": { scopes: ["project.write"], resolve: BRANCH, writes: ["branch"] },
  // 5E Decision 17: a drop into a protected branch is a reviewer's (`review.decide`), elsewhere a write.
  "history.drop": {
    scopes: ["project.read"],
    resolve: BRANCH,
    writes: ["into"],
    protectedNeeds: "review.decide",
    escalate: (_, facts) => (facts.protectedTarget === true ? ["review.decide"] : ["project.write"]),
  },
  // Clears a branch's rewind warning: a branch-limited principal only on its own branches.
  "project.acknowledgeRewind": { scopes: ["project.write"], resolve: BRANCH, targets: ["branch"] },
  "project.acknowledgeSalvage": { scopes: ["project.write"], resolve: PROJECT },
  "project.events": { scopes: ["project.read"], resolve: PROJECT },
  "tx.get": { scopes: ["project.read"], resolve: PROJECT },
  "history.log": { scopes: ["project.read"], resolve: BRANCH },
  "eval.frame": { scopes: ["project.read"], resolve: BRANCH },
  "eval.audio": { scopes: ["project.read"], resolve: BRANCH },
  "fonts.catalog": { scopes: ["server.read"] },
  "fonts.status": { scopes: ["server.read"] },
  "fonts.fetch": { scopes: ["fonts.fetch"], resolve: PROJECT },
  "fonts.plan": { scopes: ["project.read"], resolve: BRANCH },
  // Plan 6A Task 13. A look reads the project and submits a job (`jobs.submit` and the kind's own
  // `project.read`, spec §6.7); its frames go to `out`, which an agent or MCP principal may name only
  // inside its project's folder. It commits nothing, so it takes no branch write guard.
  // A read-only token gets the default place only: naming a folder (`out`) writes files where
  // the caller chooses, which takes `project.write` (owner decision, review of Tasks 11-12, M1).
  "media.look": {
    scopes: ["project.read", "jobs.submit"],
    resolve: [...BRANCH, "outDir"],
    escalate: (p) => (p.out === undefined ? [] : ["project.write"]),
  },
  // What this machine's media worker can decode and draw on.
  "media.capabilities": { scopes: ["server.read"] },
  // The media cache (6B-1 Task 13): its status is the server's to read; evicting is the owner's, as
  // the other server-wide settings are (no new scope).
  "media.cache.status": { scopes: ["server.read"] },
  "media.cache.gc": { scopes: ["settings.admin"] },
  // 6B-1 Task 17: reach is the handler's (the entry's projects come from the cache index, not the
  // payload): `project.read`, and over one of the entry's projects, else `NOT_FOUND`.
  "media.url": { scopes: ["project.read"] },
  // Plan 6B-1 Task 11: reads the project and submits a job; commits nothing.
  "media.probeDeep": { scopes: ["project.read", "jobs.submit"], resolve: BRANCH },
  // 6B-1 Task 14: a proxy reads the project and may submit a job; its status only reads.
  "media.proxy": { scopes: ["project.read", "jobs.submit"], resolve: BRANCH },
  "media.proxy.status": { scopes: ["project.read"], resolve: BRANCH },
  // Thumbnails and filmstrips (6B-1 Task 15): read the project, submit the job.
  "media.thumbs": { scopes: ["project.read", "jobs.submit"], resolve: BRANCH },
  // 6C Task 18: an asset's PCM feed reads the project and may submit a job.
  "media.pcm": { scopes: ["project.read", "jobs.submit"], resolve: BRANCH },
  // 6C Task 19: an asset's waveform peaks, the same.
  "media.waveform": { scopes: ["project.read", "jobs.submit"], resolve: BRANCH },
  // 6C Task 21: `fin listen` reads the project and submits a `media.listen` job.
  "media.listen": { scopes: ["project.read", "jobs.submit"], resolve: BRANCH },
  // 6B-1 Task 11: a write of the branch (one `asset.reprobe` record); `deep` also runs a job.
  "asset.reprobe": {
    scopes: ["project.write"],
    resolve: BRANCH,
    writes: ["branch"],
    escalate: (p) => (p.deep === true ? (["jobs.submit"] as const) : []),
  },
  // The job engine (5C-2, spec §6.7). None writes through the store's guard (no `writes`); none of
  // the job scopes is human-only, but a kind's own scopes join `jobs.submit` and may be (then the
  // confirmation applies by default). Ownership and reach are the handler's (`jobs/access.ts`).
  "jobs.submit": { scopes: ["jobs.submit"], resolve: ["jobProject", "jobKind"], escalate: (_, facts) => facts.kindScopes ?? [] },
  "jobs.get": { scopes: ["jobs.read"], resolve: ["job"] },
  "jobs.list": { scopes: ["jobs.read"], resolve: ["jobProject"] },
  "jobs.wait": { scopes: ["jobs.read"] },
  "jobs.watch": { scopes: ["jobs.read"] },
  "jobs.cancel": { scopes: ["jobs.read"], resolve: ["job"], escalate: (_, facts) => jobControl(facts) },
  // `{all}` and `jobs.workers` act on every project's jobs and processes: `jobs.control` and a
  // principal that reaches every project (review M2).
  "jobs.pause": {
    scopes: ["jobs.read"],
    resolve: ["job"],
    escalate: (p, facts) => (p.all === true ? ["jobs.control"] : jobControl(facts)),
    allProjects: (p) => p.all === true,
  },
  "jobs.resume": {
    scopes: ["jobs.read"],
    resolve: ["job"],
    escalate: (p, facts) => (p.all === true ? ["jobs.control"] : jobControl(facts)),
    allProjects: (p) => p.all === true,
  },
  "jobs.workers": { scopes: ["jobs.control"], allProjects: () => true },
  "diagnostics.status": { scopes: ["diagnostics.read"] },
  // Redacted for a caller that is not a person; the content itself is human-only (Decision 22).
  "diagnostics.export": {
    scopes: ["diagnostics.read"],
    escalate: (p) => (p.content === true ? ["diagnostics.admin"] : []),
    unconfirmed: "readOnly",
  },
  "diagnostics.purge": { scopes: ["diagnostics.admin"] },
  // Setting `never` stays open; clearing it binds every collaborator.
  "diagnostics.project": {
    scopes: ["diagnostics.read"],
    resolve: PROJECT,
    escalate: (p) => (p.collect === "default" ? ["diagnostics.admin"] : []),
  },
  // Every consent change needs a person; training on, the app's onboarding question alone.
  "diagnostics.consent": {
    scopes: ["diagnostics.admin"],
    deviceOnly: (p) => p.purpose === "training" && p.value === "on",
  },
  "diagnostics.record": { scopes: ["diagnostics.record"] },
  // Plugins and registries (sub-project 8A Decision 29). Reads and refreshes are open to any
  // principal with `plugin.read` (a refresh by a non-human installs nothing); the registries
  // themselves are the owner's (`plugin.admin` is human-only).
  "registry.list": { scopes: ["plugin.read"] },
  "registry.status": { scopes: ["plugin.read"] },
  "registry.add": { scopes: ["plugin.admin"], resolve: ["registryPaths"] },
  "registry.remove": { scopes: ["plugin.admin"] },
  "registry.forget": { scopes: ["plugin.admin"] },
  "registry.refresh": { scopes: ["plugin.read"] },
  "plugin.list": { scopes: ["plugin.read"] },
  "plugin.search": { scopes: ["plugin.read"] },
  "plugin.info": { scopes: ["plugin.read"] },
  "plugin.content.get": { scopes: ["plugin.read"] },
  // The lifecycle (9b): anyone with `plugin.read` may plan (nothing runs, and only a person's
  // plan installs); a downgrade is a person's choice. Installing, updating and removing are the
  // owner's (`plugin.install`), enabling and disabling too (`plugin.admin`). A suggestion is
  // about a project the caller reads.
  "plugin.plan": { scopes: ["plugin.read"], escalate: (p) => (p.allowDowngrade === true ? ["plugin.install"] : []) },
  // Giving up a plan is its maker's (the handler checks who made it); nothing else changes.
  "plugin.discard": { scopes: ["plugin.read"] },
  "plugin.install": { scopes: ["plugin.install"] },
  "plugin.remove": { scopes: ["plugin.install"] },
  "plugin.enable": { scopes: ["plugin.admin"] },
  "plugin.disable": { scopes: ["plugin.admin"] },
  "plugin.suggest": { scopes: ["plugin.suggest", "project.read"], resolve: PROJECT },
  // Plugin commands (8B Task 9): a run reads the project, and a command that writes needs
  // `project.write` too. Its commit goes through `tx.commit`'s own rule as the live invoker
  // (branch limits, author, guard), so the run itself names no `writes`. Listing is what the
  // caller can run, filtered to it, so it needs what a run needs, `project.read` (Task 10:
  // agent sessions hold no `plugin.read`, and `fin @<id> <command>` maps its line by the list).
  "plugin.commands": { scopes: ["project.read"] },
  "plugin.command.run": { scopes: ["project.read"], resolve: ["project", "branch", "pluginCommand"], escalate: (_, facts) => (facts.commandWrites === true ? ["project.write"] : []) },
  // Libraries (5D Decision 47). Adding and removing are the owner's, with a challenge code in the
  // console; `library.set` needs `library.scan` for the ordinary fields and `library.admin`
  // (human-only) for the ones only a person decides. Everything that reads needs `library.read`
  // and reaches the libraries the caller reads (`libraryReach`); a scan also needs `jobs.submit`,
  // and so does hashing candidates, which submits jobs.
  "library.add": { scopes: ["library.admin"], resolve: ["libraryRoot"] },
  "library.remove": { scopes: ["library.admin"] },
  "library.set": {
    scopes: ["library.scan"],
    escalate: (p) => (LIBRARY_ADMIN_FIELDS.some((f) => p[f] !== undefined) ? ["library.admin"] : []),
  },
  "library.list": { scopes: ["library.read"] },
  "library.status": { scopes: ["library.read"] },
  "library.files": { scopes: ["library.read"] },
  "library.file": { scopes: ["library.read"] },
  "library.search": { scopes: ["library.read"] },
  "library.lookup": { scopes: ["library.read"], escalate: (p) => (p.hashCandidates === true ? ["library.scan", "jobs.submit"] : []) },
  "library.scan": { scopes: ["library.scan", "jobs.submit"] },
  "library.watch": { scopes: ["library.read"] },
  // The agent runtime (sub-project 7A Task 2, spec §3.5). Starting and driving sessions is
  // `agent.run` and, in 7A, a person's through a human-driven kind (`SESSION_DRIVERS`); a start
  // creates the root branch as the starter and mints the session's credential from the mode
  // table, so the starter must itself hold `project.write` unless the session is `plan`.
  // Reading sessions is `agent.read`; answering an approval is a person's (`agent.approve`,
  // human-only), deleting a session the owner's (`agent.admin`), both with the challenge code. A
  // session's id is not a project: the handlers check the principal's reach.
  "agent.session.start": {
    scopes: ["agent.run"],
    resolve: PROJECT,
    escalate: (p) => (p.mode === "plan" ? ["project.read"] : ["project.read", "project.write"]),
    kinds: CHILD_DRIVERS,
  },
  // 7B Decision 16: a session starts children (`fin agent spawn`) and drives its own ones; a
  // delete stays people's, and so does a resume except of a child the parent itself interrupted
  // (owner decision 2026-11-13: stage 4, `child-resume.ts`).
  "agent.session.prompt": { scopes: ["agent.run"], kinds: CHILD_DRIVERS },
  "agent.session.steer": { scopes: ["agent.run"], kinds: CHILD_DRIVERS },
  "agent.session.interrupt": { scopes: ["agent.run"], kinds: CHILD_DRIVERS },
  "agent.session.resume": { scopes: ["agent.run"], kinds: CHILD_DRIVERS },
  "agent.session.end": { scopes: ["agent.run"], kinds: CHILD_DRIVERS },
  "agent.session.wait": { scopes: ["agent.run"], kinds: CHILD_DRIVERS },
  // 7B Decision 23 (owner decision 2026-11-08): people, and a parent for its own direct children
  // only (stage 4, `reach.ts`), never itself, a sibling or a grandchild.
  "agent.session.rollback": { scopes: ["agent.run"], kinds: CHILD_DRIVERS },
  "agent.session.list": { scopes: ["agent.read"], resolve: PROJECT },
  "agent.session.get": { scopes: ["agent.read"] },
  "agent.session.delete": { scopes: ["agent.admin"], kinds: SESSION_DRIVERS },
  "agent.watch": { scopes: ["agent.read"] },
  "agent.approval.list": { scopes: ["agent.read"], resolve: PROJECT },
  // A person's, from any device or the console: never an agent's (`agent.approve` is human-only).
  "agent.approval.answer": { scopes: ["agent.approve"] },
  // A session waits on its own cards (the handler checks it is its own), a person on any in reach.
  "agent.approval.wait": { scopes: ["agent.read"], kinds: [...SESSION_DRIVERS, "agent"] },
  // Server facts: detection, sign-in state as reported, whether keys are set (never a value).
  "agent.providers": { scopes: ["server.read"] },
  // The calling session's own default branch and tries; `branch` is checked against the session's
  // subtree by the handler (the payload names no project, so the policy stage cannot).
  "agent.self": { scopes: ["project.read"], kinds: SESSION_OWN },
  "agent.self.try": { scopes: ["project.write"], resolve: ["branch"], kinds: SESSION_OWN },
  // Command rules (7B Decision 7): reading them is `agent.read`; the `local` layer and project
  // trust are the owner's (`agent.admin`), people only (an agent is `SCOPE_DENIED` by kind), each
  // change with the typed code. A rule's or trust's project is checked for reach.
  "agent.rules.list": { scopes: ["agent.read"], resolve: PROJECT },
  "agent.rules.add": { scopes: ["agent.admin"], resolve: PROJECT, kinds: PEOPLE },
  "agent.rules.remove": { scopes: ["agent.admin"], kinds: PEOPLE },
  "agent.project.trust": { scopes: ["agent.admin"], resolve: PROJECT, kinds: PEOPLE },
  "agent.hooks.list": { scopes: ["agent.admin"], resolve: PROJECT, kinds: PEOPLE, unconfirmed: "readOnly" }, // 7F: the hooks, a person's to read
  // Saved prompts (7B Task 10, spec §6.4): anyone may read the list; running one starts a session
  // as the starter (so the root branch and mode's scopes need `project.write`), people only.
  "agent.savedPrompt.list": { scopes: ["agent.read"], resolve: PROJECT },
  "agent.savedPrompt.run": { scopes: ["agent.run", "project.read", "project.write"], resolve: PROJECT, kinds: SESSION_DRIVERS },
  // The owner's agent settings (Task 12): the key, the local endpoint (`agent.admin`) and the
  // counsel gate (`settings.admin`), each change with the challenge code; reading them needs none.
  // A key's project is checked for reach when named.
  "agent.key.set": { scopes: ["agent.admin"], resolve: PROJECT },
  "agent.key.list": { scopes: ["agent.admin"], unconfirmed: "readOnly" },
  "agent.key.remove": { scopes: ["agent.admin"] },
  "agent.local.set": { scopes: ["agent.admin"] },
  "agent.local.show": { scopes: ["agent.admin"], unconfirmed: "readOnly" },
  "agent.local.test": { scopes: ["agent.admin"], unconfirmed: "readOnly" },
  "agent.local.remove": { scopes: ["agent.admin"] },
  // 7D Decision 19: taking Claude Code's new account is a person's at the server's console (y/N),
  // never a device's or an agent's.
  "agent.account.confirm": { scopes: ["agent.admin"], kinds: ["console"] },
  "training.closedTargets.get": { scopes: ["server.read"] },
  "training.closedTargets.set": { scopes: ["settings.admin"] },
  // 5B (plan Decision 1, spec §5.9): pairing, devices and the environment are the owner's
  // (`auth.admin`, human-only); the streams and the device list only read. A console's `self`
  // offer (the desktop shell pairing its own renderer) needs no typed code (Decision 21).
  "auth.pair.offer": { scopes: ["auth.admin"], unconfirmedIf: (p) => (p.self === true ? "service" : undefined) },
  "auth.pair.approve": { scopes: ["auth.admin"] },
  "auth.pair.reject": { scopes: ["auth.admin"] },
  "auth.pair.pending": { scopes: ["auth.admin"], unconfirmed: "readOnly" },
  "auth.device.list": { scopes: ["auth.admin"], unconfirmed: "readOnly" },
  "auth.device.rename": { scopes: ["auth.admin"] },
  "auth.device.revoke": { scopes: ["auth.admin"] },
  "auth.devices.watch": { scopes: ["auth.admin"], unconfirmed: "readOnly" },
  "env.describe": { scopes: ["server.read"] },
  "env.name": { scopes: ["auth.admin"] },
  "env.adopt": { scopes: ["auth.admin"] },
  "env.reset": { scopes: ["auth.admin"] },
  "env.rotateIdentity": { scopes: ["auth.admin"] },
  "settings.write": { scopes: ["settings.admin"] },
  // The review workflow (5E Decision 23, spec §8.6). Opening one, commenting and refreshing are
  // `review.write` (an agent holds it); deciding, merging and closing are `review.decide` and
  // write the item's target (`drop`, `merge` or `branchSet` records) through the guard, so agent
  // branch limits and protected branches see it like any write's. The policy is human-only.
  "review.request": { scopes: ["review.write"], resolve: BRANCH },
  "review.comment": { scopes: ["review.write"], resolve: REVIEW },
  "review.refresh": { scopes: ["review.write"], resolve: REVIEW },
  "review.list": { scopes: ["project.read"], resolve: PROJECT },
  "review.get": { scopes: ["project.read"], resolve: REVIEW },
  "review.watch": { scopes: ["project.read"], resolve: PROJECT },
  "review.decide": { scopes: ["review.decide"], resolve: REVIEW, writes: ["target"], protectedNeeds: "review.decide" },
  "review.merge": { scopes: ["review.decide"], resolve: REVIEW, writes: ["target"], protectedNeeds: "review.decide" },
  "review.close": { scopes: ["review.decide"], resolve: REVIEW, writes: ["target"], protectedNeeds: "review.decide" },
  "review.policy": { scopes: ["auth.admin"], resolve: PROJECT },
  "review.policy.get": { scopes: ["project.read"], resolve: PROJECT },
};

/** The RPCs that write through the store's guard: exactly the rules with `writes`. */
export const WRITE_RPCS: ReadonlySet<FinRpcTag> = new Set(
  (Object.keys(RPC_SCOPES) as FinRpcTag[]).filter((t) => (RPC_SCOPES[t] as RpcRule).writes !== undefined),
);

/** `tag`'s rule with its types erased. */
export function ruleOf(tag: FinRpcTag): RpcRule {
  return RPC_SCOPES[tag] as RpcRule;
}

/** The scopes `tag` needs before any escalation (its rule's own). */
export function baseScopes(tag: FinRpcTag): ReadonlyArray<Scope> {
  return ruleOf(tag).scopes;
}

/** The scopes `tag` needs beyond its own for `payload` and the resolved `facts`. */
export function escalatedScopes(tag: FinRpcTag, payload: unknown, facts: RuleFacts = {}): ReadonlyArray<Scope> {
  const r = ruleOf(tag);
  const more = [...(r.escalate?.(payload as never, facts) ?? []), ...(facts.reopens === true ? (["project.write"] as const) : [])];
  return [...new Set(more)].filter((s) => !r.scopes.includes(s));
}

/** The rule's opt-out for `payload`: its `unconfirmedIf` when that answers, else its `unconfirmed`. */
export function optOutOf(tag: FinRpcTag, payload: unknown): "readOnly" | "service" | undefined {
  const r = ruleOf(tag);
  return r.unconfirmedIf?.(payload as never) ?? r.unconfirmed;
}

/**
 * Whether a call must carry its connection's typed code (`console.challenge`):
 * - a `console` principal's, when it needs a human-only scope and its rule does not opt out
 *   (`unconfirmed`, or `unconfirmedIf` for this payload; 5A Decision 30, review of Task 7 L2);
 * - a `device`'s, only for the rules a service manager calls without a terminal
 *   (`unconfirmed: "service"`: `server.stop`; owner decision of 2026-11-08, narrowing 5B
 *   Decision 22): a person in the app confirms stopping the server, and nothing else;
 * - a non-human's never (it holds no human-only scope); a human kind this code does not know is
 *   held to the console's rule, so a new kind fails closed.
 * `principal` defaults to the console.
 */
export function needsConfirmation(
  tag: FinRpcTag,
  payload: unknown,
  facts: RuleFacts = {},
  principal: { readonly kind: PrincipalKind; readonly human: boolean } = { kind: "console", human: true },
): boolean {
  if (!principal.human) return false;
  if (principal.kind === "device") return ruleOf(tag).unconfirmed === "service";
  return optOutOf(tag, payload) === undefined && requiredScopes(tag, payload, facts).some((s) => HUMAN_ONLY.has(s));
}

/** Every scope `tag` needs for `payload`, given the resolved `facts`. */
export function requiredScopes(tag: FinRpcTag, payload: unknown, facts: RuleFacts = {}): ReadonlyArray<Scope> {
  return [...new Set([...baseScopes(tag), ...escalatedScopes(tag, payload, facts)])];
}
