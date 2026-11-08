//! Shared fixtures of `events.rs`: the test key, class constructors and one payload of every
//! kind (moved out to keep `events.rs` under the test file cap).

#![allow(dead_code)]

use fin_core::events::names::{self, CLI_COMMANDS, MEDIA_TYPES, OP_KINDS};
use fin_core::events::names_jobs;
use fin_core::events::payloads::*;
use fin_core::events::*;
use serde_json::{Value, json};

pub const TX: &str = "01JC0000000000000000000002";
pub const SESSION: &str = "01JC0000000000000000000003";

/// A test key: BLAKE3 keyed with a fixed key, truncated to 16 bytes (as the journal's).
pub fn key(data: &[u8]) -> [u8; 16] {
    let full = blake3::keyed_hash(&[9; 32], data);
    full.as_bytes()[..16].try_into().unwrap()
}

pub fn ulid(s: &str) -> Ulid {
    Ulid::parse(s).unwrap()
}

pub fn h(c: char) -> Hashed {
    Hashed::parse(&format!("k1:{}", c.to_string().repeat(32))).unwrap()
}

pub fn id(s: &str) -> IdRef {
    IdRef::classify(s).unwrap()
}

pub fn code(s: &str) -> Code {
    Code::parse(s).unwrap()
}

pub fn blob() -> BlobRef {
    BlobRef {
        hash: BlobHash::from_bytes(&[7; 32]),
        bytes: 1234,
        media_type: MEDIA_TYPES.get("application/json").unwrap(),
    }
}

pub fn tx() -> fin_core::Tx {
    serde_json::from_value(json!({
        "txId": TX, "author": "a person", "branch": "main", "baseRev": 41, "schemaVersion": 2,
        "ops": [
            { "op": "item.trim", "item": "c1", "edge": "start", "delta": "1", "mode": "ripple" },
            { "op": "item.trim", "item": "c2", "edge": "end", "delta": "-1/2", "mode": "ripple" },
        ],
    }))
    .unwrap()
}

/// A payload from its JSON (the agent samples, which have many fields).
pub fn typed<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}

pub fn counts<K: Ord>(pairs: impl IntoIterator<Item = (K, u64)>) -> Counts<K> {
    Counts(pairs.into_iter().collect())
}

pub fn log_commit() -> LogCommit {
    LogCommit {
        record: CommitRecord::Edit,
        outcome: CommitOutcome::Rebased,
        code: None,
        ops: counts([(OP_KINDS.get("item.trim").unwrap(), 2)]),
        base_rev: Some(39),
        rebased_over: Some(2),
        conflicts: Counts::default(),
        links: vec![],
        micros: 812,
        intent: Some(Content(TxIntent::from_txs([tx()], &key))),
        intent_omitted: None,
    }
}

pub fn hist() -> Histogram {
    Histogram {
        start: 1_794_571_200_000,
        window_ms: 60_000,
        count: 12,
        errors: counts([(code("CONFLICT"), 1)]),
        max_micros: 9000,
        buckets: vec![0, 0, 1, 3, 8],
    }
}

#[path = "../events_support/plugin.rs"]
mod plugin_samples;

/// One payload of every kind, content fields filled.
pub fn samples() -> Vec<AnyPayload> {
    let mut v = samples_before_plugins();
    plugin_samples::insert(&mut v, h('7'), code("PLUGIN_MANIFEST_INVALID"));
    v
}

fn samples_before_plugins() -> Vec<AnyPayload> {
    let inline = |v: Value| Some(Content(Body::Inline(v)));
    vec![
        SessionStart {
            app: Version::parse("0.4.0").unwrap(),
            protocol: 1,
            os: OsFamily::Linux,
            os_major: 6,
            arch: Arch::X86_64,
            cores: 8,
            mem_gi_b: 16,
            profile: BuildProfile::Release,
        }
        .into(),
        SessionEnd { uptime_s: 3600 }.into(),
        SessionUnclean {
            previous: id(SESSION),
        }
        .into(),
        ConsentChanged {
            from: Purposes::ALL,
            to: Purposes::DIAGNOSTICS,
            basis: Basis::User,
            rev: 4,
        }
        .into(),
        JournalDropped {
            count: 3,
            bytes: 900,
            since: 1_794_571_200_000,
        }
        .into(),
        log_commit().into(),
        CliCommand {
            name: CLI_COMMANDS.get("item.trim").unwrap(),
            flags: vec![
                names::CLI_FLAGS.get("dry-run").unwrap(),
                names::CLI_FLAGS.get("item").unwrap_or(Name::OTHER),
            ],
            args_hash: Some(h('a')),
            micros: 41_000,
            exit: 0,
            code: None,
            online: true,
            dry_run: false,
            tx_ids: vec![id(TX), IdRef::from_hashed(h('b'))],
        }
        .into(),
        RpcStats {
            key: names::RPC_METHODS.get("tx.commit").unwrap(),
            histogram: hist(),
        }
        .into(),
        EvalStats {
            key: names::EVAL_KEYS.get("eval.frame").unwrap(),
            histogram: hist(),
        }
        .into(),
        FontsFetch {
            entry: h('c'),
            bundled: false,
            bytes: 120_000,
            micros: 300_000,
            outcome: FetchOutcome::Failed,
            code: Some(code("FONT_FETCH_FAILED")),
        }
        .into(),
        StorageState {
            state: StorageMode::ReadOnly,
            reason: code("LEASE_LOST"),
        }
        .into(),
        ErrorInternal {
            call: names::RPC_METHODS.get("tx.commit").unwrap(),
            code: code("INTERNAL"),
            details: vec![names::DETAIL_KEYS.get("cause").unwrap()],
        }
        .into(),
        ErrorPanic {
            at: PanicWhere::FinNode,
            location: SrcLoc::parse("crates/fin-node/src/tasks.rs:120"),
            third_party: false,
            message_hash: h('d'),
            thread: PanicThread::Worker,
            message: Some(Content(Scrubbed::new(
                "index out of range at /home/someone/x",
                &ScrubCtx::default(),
            ))),
        }
        .into(),
        JobDone {
            kind: names_jobs::JOB_KINDS.get("test.crash").unwrap(),
            family: names_jobs::WORKER_FAMILIES.get("test").unwrap(),
            outcome: JobOutcome::Failed,
            code: Some(code("POISON_ITEM")),
            attempts: 2,
            priority: JobPriority::Normal,
            warm: false,
            queued_ms: 120,
            run_micros: 1_500_000,
            items: Some(8),
            items_failed: Some(1),
            isolated: true,
            helpers: 0,
        }
        .into(),
        WorkerExit {
            role: ProcessRole::Helper,
            family: names_jobs::WORKER_FAMILIES.get("media").unwrap(),
            helper_kind: Some(ProcessHelperKind::Plugin),
            plugin: Some(h('e')),
            reason: ExitReason::Crashed,
            exit_code: None,
            signal: names_jobs::SIGNALS.get("segv"),
            jobs_served: 3,
            lifetime_s: 42,
            sandboxed: false,
        }
        .into(),
        typed::<AgentSession>(json!({
            "sessionId": SESSION, "phase": "ended", "harness": "builtin", "provider": "openrouter",
            "model": "other", "modelHash": h('1'), "weights": "unknown", "role": "editor",
            "mode": "draft", "trigger": "user", "reason": "BUDGET_EXCEEDED", "totals": {
                "turns": 4, "toolCalls": 9, "txs": 2, "inputTokens": 48_000,
                "outputTokens": 3_100, "costMicros": 91_000, "wallS": 340 },
        }))
        .into(),
        AgentTurn {
            system: Some(Content(Body::Blob(blob()))),
            tools: Some(Content(Body::Blob(blob()))),
            messages_delta: Some(Content(Body::Blob(blob()))),
            output: inline(json!([{ "type": "text", "text": "trim the first clip" }])),
            ..typed(json!({
                "turnId": "01JC0000000000000000000005", "runId": "01JC0000000000000000000007",
                "seq": 2, "harness": "claudeCode", "provider": "anthropic",
                "model": "claude-opus-5-5", "weights": "closed", "inputTokens": 1200,
                "outputTokens": 300, "cacheReadTokens": 1000, "cacheWriteTokens": 0,
                "reasoningTokens": 80, "latencyMs": 2100, "ttftMs": 400, "stopReason": "toolUse",
                "costMicros": 4200, "costBasis": "subscription", "capture": "stream",
                "systemHash": h('e'), "toolsHash": h('f'),
            }))
        }
        .into(),
        typed::<AgentToolCall>(json!({
            "callId": "01JC0000000000000000000006", "turnId": "01JC0000000000000000000005",
            "tool": "fin", "command": "item.trim", "argsHash": h('a'),
            "args": { "inline": { "item": "c1", "delta": "1" } },
        }))
        .into(),
        typed::<AgentToolResult>(json!({
            "callId": "01JC0000000000000000000006", "ok": true, "exit": 0, "micros": 40_000,
            "txIds": [TX], "attribution": "exact", "output": { "inline": { "txId": TX } },
        }))
        .into(),
        typed::<AgentApproval>(json!({
            "approvalId": "01JC0000000000000000000008", "sessionId": SESSION, "kind": "budget",
            "decision": "allow", "by": "user", "latencyMs": 12_000,
            "subject": { "inline": { "kind": "money", "add": 1_000_000 } },
        }))
        .into(),
        UiAction {}.into(),
        ReviewVerdict {}.into(),
    ]
}
