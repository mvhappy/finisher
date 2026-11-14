// agent 0001 (7A-2 Task 3b, plan 7A Decision 1, spec §3.3): the agent runtime's product state.
// Frozen once merged: never edit, add 0002. No foreign key leaves the chain.
//
// - `sessions`: one row per session; `runId` and `approvals` (JSON) hold the state machine's
//   active run and open approvals (session.ts), `usage` the session's running total.
// - `runs`, `nodes`: one row per run and per conversation item. A node's payload is inline
//   (`payload`, ≤ 4 KiB of JSON) or a blob beside the session (`blobRef`, a BLAKE3 hex).
//   `nodes_call` lets a call have one `toolCall` and one `toolResult` node only, so a
//   synthetic result written at recovery can never be written twice.
// - `session_txs`: every commit, undo, redo, revert, cherry-pick and merge an agent principal
//   makes, keyed by the txId (spec §5.2).
// - `session_state`: the per-session default branch and `fin try` list (spec §5.1).
// - `approvals`, `budgets`, `budget_days` (the server's money per UTC day).
// - `session_outbox`: the effects outside `control.db` a transition owes (journal phases,
//   credential revocation and refresh, synthetic tool results to journal), written in the
//   transition's transaction and deleted once done, so a crash between the two re-does them
//   at the next start (the job engine's `announce` marker).
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const agent0001: Migration = {
  n: 1,
  up: (db: DatabaseSync) => {
    db.exec(`
      CREATE TABLE sessions (
        id TEXT PRIMARY KEY, projectId TEXT NOT NULL, harness TEXT NOT NULL, provider TEXT NOT NULL,
        model TEXT NOT NULL, weights TEXT NOT NULL, role TEXT NOT NULL, mode TEXT NOT NULL, rootBranch TEXT,
        baseBranch TEXT NOT NULL, baseSeq INTEGER NOT NULL, parent TEXT, status TEXT NOT NULL,
        statusReason TEXT, runId TEXT, approvals TEXT NOT NULL DEFAULT '[]', credentialId TEXT NOT NULL,
        providerRef TEXT, budget TEXT NOT NULL, usage TEXT NOT NULL, createdBy TEXT NOT NULL,
        createdAt INTEGER NOT NULL, endedAt INTEGER, title TEXT);
      CREATE INDEX sessions_project ON sessions (projectId, id);
      CREATE INDEX sessions_live ON sessions (status) WHERE status IN ('starting', 'running', 'waitingApproval');
      CREATE INDEX sessions_ended ON sessions (endedAt) WHERE endedAt IS NOT NULL;
      CREATE TABLE runs (
        id TEXT PRIMARY KEY, sessionId TEXT NOT NULL REFERENCES sessions (id), seq INTEGER NOT NULL,
        trigger TEXT NOT NULL, status TEXT NOT NULL, startedAt INTEGER NOT NULL, endedAt INTEGER,
        headAtStart INTEGER, stopReason TEXT, usage TEXT NOT NULL);
      CREATE UNIQUE INDEX runs_session ON runs (sessionId, seq);
      CREATE INDEX runs_open ON runs (status) WHERE status = 'running';
      CREATE TABLE nodes (
        id TEXT PRIMARY KEY, runId TEXT NOT NULL REFERENCES runs (id), sessionId TEXT NOT NULL,
        seq INTEGER NOT NULL, kind TEXT NOT NULL, providerId TEXT, callId TEXT, payload TEXT, blobRef TEXT,
        createdAt INTEGER NOT NULL,
        CHECK ((payload IS NULL) <> (blobRef IS NULL)));
      CREATE UNIQUE INDEX nodes_session ON nodes (sessionId, seq);
      CREATE INDEX nodes_run ON nodes (runId, seq);
      CREATE UNIQUE INDEX nodes_call ON nodes (callId, kind) WHERE callId IS NOT NULL AND kind IN ('toolCall', 'toolResult');
      CREATE INDEX nodes_blob ON nodes (sessionId, blobRef) WHERE blobRef IS NOT NULL;
      CREATE TABLE session_txs (
        txId TEXT PRIMARY KEY, sessionId TEXT NOT NULL, callId TEXT, attribution TEXT NOT NULL,
        at INTEGER NOT NULL, outcome TEXT) WITHOUT ROWID;
      CREATE INDEX session_txs_session ON session_txs (sessionId, at);
      CREATE INDEX session_txs_call ON session_txs (callId) WHERE callId IS NOT NULL;
      CREATE TABLE session_state (
        sessionId TEXT PRIMARY KEY REFERENCES sessions (id), defaultBranch TEXT NOT NULL,
        tries TEXT NOT NULL DEFAULT '[]') WITHOUT ROWID;
      CREATE TABLE approvals (
        id TEXT PRIMARY KEY, sessionId TEXT NOT NULL REFERENCES sessions (id), kind TEXT NOT NULL,
        status TEXT NOT NULL, subject TEXT NOT NULL, argsHash TEXT NOT NULL, requestedAt INTEGER NOT NULL,
        expiresAt INTEGER NOT NULL, decision TEXT, by TEXT, answeredAt INTEGER, consumedAt INTEGER);
      CREATE INDEX approvals_session ON approvals (sessionId, requestedAt);
      CREATE INDEX approvals_open ON approvals (status) WHERE status = 'open';
      CREATE TABLE budgets (
        sessionId TEXT NOT NULL, kind TEXT NOT NULL, "limit" INTEGER, used INTEGER NOT NULL DEFAULT 0,
        reserved INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (sessionId, kind)) WITHOUT ROWID;
      CREATE TABLE budget_days (
        day TEXT PRIMARY KEY, used INTEGER NOT NULL DEFAULT 0, reserved INTEGER NOT NULL DEFAULT 0) WITHOUT ROWID;
      CREATE TABLE session_outbox (
        id INTEGER PRIMARY KEY, sessionId TEXT NOT NULL, effect TEXT NOT NULL, at INTEGER NOT NULL);
      CREATE INDEX session_outbox_session ON session_outbox (sessionId, id);
    `);
  },
};
