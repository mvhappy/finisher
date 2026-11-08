//! `fin` under an agent session's credential (plan 7A Task 8b; Decisions 14a, 14b, 14d): the
//! session's root is the default branch, `fin try` state lives on the server per session and
//! `fin-config.json` is never touched, there is no fallback to the local key, and the author
//! is the server's. Against a real `finisherd` that opens sessions (`support::agent_host`).

mod support;

use std::ffi::OsStr;
use std::path::Path;

use serde_json::Value;
use support::agent_host::{AgentHost, Session};
use support::{assert_error, ok, view_items};

const INSERT: &[&str] = &[
    "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "1s",
];

fn items_on(host: &AgentHost, dir: &Path, branch: &str) -> usize {
    view_items(&ok(host.fin(dir, &["view", "--branch", branch]))).len()
}

fn default_of(host: &AgentHost, s: &Session, cwd: &Path) -> String {
    let d = ok(host.agent(s, cwd, &["branch", "default"]));
    d["default"].as_str().unwrap().to_string()
}

fn config_bytes(host: &AgentHost) -> Option<Vec<u8>> {
    std::fs::read(host.server.sd.join("fin-config.json")).ok()
}

#[test]
fn agent_default_branch_is_session_root() {
    let Some(host) = AgentHost::start("agent_default_branch_is_session_root") else {
        return;
    };
    let (dir, project) = host.project();
    // A person's default is another branch: an agent must not follow it, nor `main`.
    ok(host.fin(&dir, &["branch", "new", "other"]));
    ok(host.fin(&dir, &["branch", "default", "other"]));
    let s = host.open(&project, "draft");
    let root = s.root.clone().unwrap();
    let ws = host.workspace(&dir, &s);
    let committed = ok(host.agent(&s, &ws, INSERT));
    assert!(committed["txId"].is_string(), "{committed}");
    assert_eq!(items_on(&host, &dir, &root), 1);
    assert_eq!(items_on(&host, &dir, "main"), 0);
    assert_eq!(items_on(&host, &dir, "other"), 0);
    assert_eq!(default_of(&host, &s, &ws), root);
    // Reads default to the root too.
    assert_eq!(view_items(&ok(host.agent(&s, &ws, &["view"]))).len(), 1);
    // `--branch main` is refused: outside the session's subtree.
    let mut on_main = INSERT.to_vec();
    on_main.extend(["--branch", "main"]);
    assert_error(&host.agent(&s, &ws, &on_main), 7, "BRANCH_NOT_ALLOWED");
    // The commit is the session's.
    let txs = host.txs(&s);
    assert_eq!(txs.len(), 1, "{txs:?}");
    assert_eq!(txs[0]["txId"], committed["txId"]);
}

#[test]
fn try_state_is_per_session() {
    let Some(host) = AgentHost::start("try_state_is_per_session") else {
        return;
    };
    let (dir, project) = host.project();
    ok(host.fin(&dir, &["branch", "new", "mine"]));
    ok(host.fin(&dir, &["branch", "default", "mine"]));
    let before = config_bytes(&host);
    let (a, b) = (host.open(&project, "draft"), host.open(&project, "draft"));
    let (wa, wb) = (host.workspace(&dir, &a), host.workspace(&dir, &b));
    let tried = ok(host.agent(&a, &wa, &["try", "alt"]));
    let try_id = tried["branch"]["id"].as_str().unwrap().to_string();
    assert_eq!(default_of(&host, &a, &wa), try_id);
    // The other session's default and tries are untouched.
    let root_b = b.root.clone().unwrap();
    assert_eq!(default_of(&host, &b, &wb), root_b);
    let state = host.request(serde_json::json!({ "op": "state", "sessionId": b.id }));
    assert_eq!(state["tries"], serde_json::json!([]), "{state}");
    ok(host.agent(&a, &wa, INSERT));
    ok(host.agent(&b, &wb, INSERT));
    assert_eq!(items_on(&host, &dir, &try_id), 1);
    assert_eq!(items_on(&host, &dir, &root_b), 1);
    assert_eq!(items_on(&host, &dir, a.root.as_ref().unwrap()), 0);
    // fin-config.json is byte-identical, and a person's fin keeps its own default.
    assert_eq!(config_bytes(&host), before);
    let person = ok(host.fin(&dir, &["branch", "default"]));
    let mine = ok(host.fin(&dir, &["branch", "ls"]));
    let mine_id = mine["branches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "mine")
        .map(|v| v["id"].clone())
        .unwrap();
    assert_eq!(person["default"], mine_id, "{person}");
}

#[test]
fn try_outside_subtree_refused() {
    let Some(host) = AgentHost::start("try_outside_subtree_refused") else {
        return;
    };
    let (dir, project) = host.project();
    let (a, b) = (host.open(&project, "draft"), host.open(&project, "draft"));
    let wa = host.workspace(&dir, &a);
    let other = b.root.clone().unwrap();
    assert_error(
        &host.agent(&a, &wa, &["branch", "default", &other]),
        7,
        "BRANCH_NOT_ALLOWED",
    );
    assert_error(
        &host.agent(&a, &wa, &["try", "x", "--branch", "main"]),
        7,
        "BRANCH_NOT_ALLOWED",
    );
    assert_eq!(default_of(&host, &a, &wa), a.root.clone().unwrap());
}

#[test]
fn abort_try_restores_default() {
    let Some(host) = AgentHost::start("abort_try_restores_default") else {
        return;
    };
    let (dir, project) = host.project();
    let s = host.open(&project, "draft");
    let ws = host.workspace(&dir, &s);
    let root = s.root.clone().unwrap();
    let tried = ok(host.agent(&s, &ws, &["try"]));
    let try_id = tried["branch"]["id"].as_str().unwrap().to_string();
    ok(host.agent(&s, &ws, INSERT));
    let dropped = ok(host.agent(&s, &ws, &["try", "drop"]));
    assert_eq!(dropped["switchedTo"], root.as_str(), "{dropped}");
    assert_eq!(default_of(&host, &s, &ws), root);
    // A second try is kept: merged into the root.
    let tried = ok(host.agent(&s, &ws, &["try"]));
    assert_ne!(tried["branch"]["id"], try_id.as_str());
    ok(host.agent(&s, &ws, INSERT));
    ok(host.agent(&s, &ws, &["try", "keep"]));
    assert_eq!(default_of(&host, &s, &ws), root);
    assert_eq!(items_on(&host, &dir, &root), 1);
    // Keep or drop with no try is a usage error naming the try.
    assert_error(&host.agent(&s, &ws, &["try", "drop"]), 2, "USAGE");
}

#[test]
fn fin_with_credential_never_uses_local_key() {
    let Some(host) = AgentHost::start("fin_with_credential_never_uses_local_key") else {
        return;
    };
    let (dir, project) = host.project();
    // The local key is right there and readable (`fin` as a person just used it).
    assert!(host.server.sd.join("auth").join("local.key").is_file());
    let missing = Session {
        id: "01JC0000000000000000000000".into(),
        credential: host.server.root.join("no-such-credential.json"),
        root: None,
    };
    for args in [&["view"][..], &["whoami"], INSERT] {
        let run = host.agent(&missing, &dir, args);
        assert_error(&run, 7, "UNAUTHENTICATED");
        assert_eq!(run.json["error"]["details"]["reason"], "credentialUnusable");
    }
    // FIN_SERVER_DATA is ignored while FIN_CREDENTIAL is set: no FIN_SERVER, no server.
    let mut cmd = support::fin_cmd(&host.server.sd, &dir, &["view"]);
    cmd.env("FIN_CREDENTIAL", &missing.credential);
    let run = support::finish(&["view"], cmd.output().unwrap());
    assert_error(&run, 5, "SERVER_UNAVAILABLE");
    // A malformed file is no credential either.
    let bad = host.server.root.join("bad.json");
    std::fs::write(&bad, "{\"v\":1}").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let malformed = Session {
        credential: bad,
        ..missing.clone()
    };
    assert_error(
        &host.agent(&malformed, &dir, &["view"]),
        7,
        "UNAUTHENTICATED",
    );
    // A revoked session's credential is refused by name, never replaced by the local key.
    let s = host.open(&project, "draft");
    let ws = host.workspace(&dir, &s);
    ok(host.agent(&s, &ws, &["view"]));
    host.revoke(&s);
    let run = host.agent(&s, &ws, INSERT);
    assert_error(&run, 7, "CREDENTIAL_REVOKED");
    assert_eq!(items_on(&host, &dir, s.root.as_ref().unwrap()), 0);
    // `--project` naming another project is not this session's.
    let other = host.server.root.join("other");
    ok(host.fin(
        &host.server.root,
        &["project", "create", "other", "--name", "Other"],
    ));
    let t = host.open(&project, "draft");
    let wt = host.workspace(&dir, &t);
    let o = other.display().to_string();
    assert_error(
        &host.agent(&t, &wt, &["view", "--project", &o]),
        3,
        "PROJECT_NOT_FOUND",
    );
}

#[test]
fn author_cannot_be_overridden() {
    let Some(host) = AgentHost::start("author_cannot_be_overridden") else {
        return;
    };
    let (dir, project) = host.project();
    let s = host.open(&project, "draft");
    let ws = host.workspace(&dir, &s);
    let root = s.root.clone().unwrap();
    let as_other = [("FIN_AUTHOR", OsStr::new("other"))];
    ok(host.agent_env(&s, &ws, INSERT, &as_other));
    // A person's edit on the session's branch, as author `other`.
    let mut on_root = INSERT.to_vec();
    on_root.extend(["--branch", &root]);
    ok(support::fin_env(&host.server.sd, &dir, &on_root, &as_other));
    assert_eq!(items_on(&host, &dir, &root), 2);
    // The agent's undo, even claiming `other`, undoes its own edit only.
    ok(host.agent_env(&s, &ws, &["undo"], &as_other));
    assert_eq!(items_on(&host, &dir, &root), 1);
    let run = host.agent_env(&s, &ws, &["undo"], &as_other);
    assert_error(&run, 3, "NOTHING_TO_UNDO");
    assert!(
        run.json["error"]["fix"]
            .as_str()
            .unwrap()
            .contains("its own edits"),
        "{}",
        run.stdout
    );
    // The person's `other` stack still holds their edit.
    ok(support::fin_env(
        &host.server.sd,
        &dir,
        &["undo", "--branch", &root],
        &as_other,
    ));
    assert_eq!(items_on(&host, &dir, &root), 0);
    let txs: Vec<Value> = host.txs(&s);
    assert_eq!(txs.len(), 2, "the commit and its undo: {txs:?}");
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
