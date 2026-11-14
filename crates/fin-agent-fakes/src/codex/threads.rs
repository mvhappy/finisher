//! Threads: ids, the persisted record under `$CODEX_HOME/sessions/`, and the `Thread` and `Turn`
//! views the protocol shows. A record is one JSON file per thread (the real binary keeps
//! rollouts; a resume in a new process only needs this fake to find the conversation again).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A UUIDv7-shaped id: the millisecond clock, then bits from a counter and the process id.
pub fn new_id() -> String {
    static N: AtomicU64 = AtomicU64::new(1);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let mut x = (n << 32 | u64::from(std::process::id())) ^ 0x9E37_79B9_7F4A_7C15;
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    let t = now_ms() as u64 & 0xFFFF_FFFF_FFFF;
    format!(
        "{:08x}-{:04x}-7{:03x}-{:04x}-{:012x}",
        t >> 16,
        t & 0xFFFF,
        x & 0xFFF,
        0x8000 | ((x >> 12) & 0x3FFF),
        ((x >> 26) ^ (n << 8)) & 0xFFFF_FFFF_FFFF
    )
}

pub fn path_of(home: &Path, id: &str) -> PathBuf {
    home.join("sessions").join(format!("rollout-{id}.json"))
}

/// A stored thread. A turn still `inProgress` belongs to a process that died mid-turn, and is
/// shown `interrupted`.
/// UNOBSERVED(7E Task 0 Step 3): how 0.167.0 shows the turn a crash cut short after a resume.
pub fn load(home: &Path, id: &str) -> Option<Value> {
    // An id is a path component: refuse anything else.
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return None;
    }
    let mut t: Value =
        serde_json::from_str(&std::fs::read_to_string(path_of(home, id)).ok()?).ok()?;
    for turn in t["turns"].as_array_mut().into_iter().flatten() {
        if turn["status"] == "inProgress" {
            turn["status"] = json!("interrupted");
            turn["itemsView"] = json!("full");
        }
    }
    Some(t)
}

/// Persists a non-ephemeral thread (temp file, then rename).
pub fn save(home: &Path, thread: &Value) {
    if thread["ephemeral"] == true {
        return;
    }
    let Some(id) = thread["id"].as_str() else {
        return;
    };
    let p = path_of(home, id);
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, thread.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// The stored record of a new thread.
pub fn create(id: &str, cwd: &str, model: &str, ephemeral: bool, tools: Vec<Value>) -> Value {
    let now = now_ms() / 1000;
    json!({"id": id, "cwd": cwd, "model": model, "ephemeral": ephemeral, "createdAt": now,
        "updatedAt": now, "dynamicTools": tools, "turns": []})
}

/// `Thread` as the protocol shows it (`status` is the caller's: `idle`, `active`,
/// `systemError`, or `notLoaded` for a stored thread this process has not loaded); `turns`
/// only when asked for.
pub fn view(home: &Path, t: &Value, version: &str, with_turns: bool, status: Value) -> Value {
    let id = t["id"].as_str().unwrap_or("");
    let cwd = t["cwd"].clone();
    let turns = t["turns"].as_array().cloned().unwrap_or_default();
    let preview = turns
        .first()
        .and_then(|u| u["items"][0]["content"][0]["text"].as_str())
        .unwrap_or("")
        .to_string();
    json!({"id": id,
        "environments": [{"environmentId": "local", "cwd": cwd, "runtimeWorkspaceRoots": [cwd]}],
        "extra": null, "sessionId": id, "forkedFromId": null, "parentThreadId": null,
        "preview": preview, "ephemeral": t["ephemeral"], "section": null, "sectionEnteredAt": null,
        "projectId": null, "historyMode": "paginated", "modelProvider": "openai",
        "model": t["model"], "reasoningEffort": null, "createdAt": t["createdAt"],
        "updatedAt": t["updatedAt"], "recencyAt": t["updatedAt"], "status": status,
        "path": if t["ephemeral"] == true { Value::Null } else { json!(path_of(home, id)) },
        "cwd": cwd, "cliVersion": version, "originator": "finisher", "source": "vscode",
        "canAcceptDirectInput": true, "threadSource": null, "agentNickname": null,
        "agentRole": null, "gitInfo": null, "name": null, "daybreakEnabled": null,
        "turns": if with_turns { Value::Array(turns) } else { json!([]) }})
}

/// `SandboxMode` (a request's `sandbox`) as the `SandboxPolicy` the response shows.
pub fn sandbox_policy(mode: &Value) -> Value {
    match mode.as_str() {
        Some("workspace-write") => json!({"type": "workspaceWrite", "writableRoots": [],
            "networkAccess": false, "excludeTmpdirEnvVar": false, "excludeSlashTmp": false}),
        Some("danger-full-access") => json!({"type": "dangerFullAccess"}),
        _ => json!({"type": "readOnly", "networkAccess": false}),
    }
}

/// A turn. With `items` it is the thread's record (`itemsView: "full"`); without, it is what a
/// notification carries: 0.167.0's `turn/started`, `turn/completed` (failed and interrupted
/// seen) and `turn/start` answer carry no items (`notLoaded`).
/// UNOBSERVED(7E Task 0 Step 3): a `completed` turn's `turn/completed` items.
pub fn turn_view(
    id: &str,
    status: &str,
    items: Option<&[Value]>,
    error: Value,
    started_s: Option<i64>,
    done_ms: i64,
) -> Value {
    let open = status == "inProgress";
    json!({"id": id, "rootTurnId": id, "items": items.unwrap_or(&[]),
        "itemsView": if items.is_some() { "full" } else { "notLoaded" }, "status": status,
        "error": error, "startedAt": started_s,
        "completedAt": if open { Value::Null } else { json!(now_ms() / 1000) },
        "durationMs": if open { Value::Null } else { json!(done_ms) }})
}
