//! `fin review` against a real `finisherd` with an agent session on its own branch (5E plan
//! Task 10, Decisions 24 and 26). Tests that need the server print `SKIPPED <test>: <reason>`
//! and pass when Node or fin-node is missing.

mod support;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use support::console::Console;
use support::{Run, Server, assert_error, fin, fin_env, media, ok};

/// A server, a project `film` (asset a1 = `clip.mp4`) and an agent credential minted with the
/// defaults and `branches: ["own"]`: it creates branch `work` and edits only there.
struct World {
    server: Server,
    dir: PathBuf,
    credential: PathBuf,
    wide: PathBuf,
    project_id: String,
}

/// Mints an agent credential (the mint defaults plus `extra`) into `<root>/<name>`.
fn mint(server: &Server, name: &str, extra: Value) -> PathBuf {
    let mut payload = json!({ "kind": "agent", "label": name });
    for (k, v) in extra.as_object().unwrap() {
        payload[k] = v.clone();
    }
    let minted = Console::open(server).confirmed_ok("auth.credential.mint", payload);
    let file = server.root.join(name);
    std::fs::write(&file, minted["file"].to_string()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    file
}

impl World {
    fn start(test: &str) -> Option<World> {
        let server = Server::start(test)?;
        let root = server.root.clone();
        let created = ok(fin(
            &server.sd,
            &root,
            &["project", "create", "film", "--name", "Film"],
        ));
        let project_id = created["project"]["projectId"]
            .as_str()
            .unwrap()
            .to_string();
        let dir = root.join("film");
        media(&dir.join("clip.mp4"), 10);
        ok(fin(&server.sd, &dir, &["asset", "add", "clip.mp4"]));
        let credential = mint(&server, "own.json", json!({ "branches": ["own"] }));
        // Every branch, protected ones included: only the policy keeps it off main.
        let wide = mint(&server, "wide.json", json!({}));
        let w = World {
            server,
            dir,
            credential,
            wide,
            project_id,
        };
        ok(w.agent(&["branch", "new", "work"]));
        Some(w)
    }
    fn console(&self) -> Console {
        Console::open(&self.server)
    }
    fn local(&self, args: &[&str]) -> Run {
        fin(&self.server.sd, &self.dir, args)
    }
    fn agent(&self, args: &[&str]) -> Run {
        self.as_agent(&self.credential, args)
    }
    fn as_agent(&self, credential: &Path, args: &[&str]) -> Run {
        let socket = PathBuf::from(&self.server.socket);
        fin_env(
            &self.server.sd,
            &self.dir,
            args,
            &[
                ("FIN_CREDENTIAL", credential.as_os_str()),
                ("FIN_SERVER", socket.as_os_str()),
                ("FIN_PROJECT_DIR", self.dir.as_os_str()),
            ],
        )
    }
    /// The agent inserts a 1 s clip on branch `work`; its txId.
    fn edit(&self) -> String {
        let doc = ok(self.agent(&[
            "clip",
            "insert",
            "--track",
            "t1",
            "--at",
            "end",
            "--asset",
            "a1",
            "--no-audio",
            "--dur",
            "1s",
            "--branch",
            "work",
        ]));
        doc["txId"].as_str().unwrap().to_string()
    }
    fn request(&self) -> String {
        let doc = ok(self.agent(&[
            "review",
            "request",
            "--branch",
            "work",
            "--title",
            "Tighten the intro",
        ]));
        doc["item"]["id"].as_str().unwrap().to_string()
    }
    fn show(&self, id: &str) -> Value {
        ok(self.local(&["review", "show", id]))
    }
    fn origins(&self, id: &str) -> Vec<String> {
        self.show(id)["plan"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["status"] == "willMerge")
            .map(|e| e["origin"].as_str().unwrap().to_string())
            .collect()
    }
}

macro_rules! world {
    ($name:literal) => {{
        let Some(w) = World::start($name) else {
            return;
        };
        w
    }};
}

#[test]
fn fin_review_request_ls_show_comment_close() {
    let w = world!("fin_review_request_ls_show_comment_close");
    w.edit();
    w.edit();
    let first = ok(w.agent(&[
        "review",
        "request",
        "--branch",
        "work",
        "--title",
        "Tighten the intro",
    ]));
    assert_eq!(first["existing"], false, "{first}");
    let id = first["item"]["id"].as_str().unwrap().to_string();
    assert_eq!(first["item"]["kind"], "branchReview");
    assert_eq!(first["item"]["state"], "open");
    // A second request for the same pair answers the open item.
    let again = ok(w.agent(&["review", "request", "--branch", "work", "--title", "Again"]));
    assert_eq!(again["existing"], true, "{again}");
    assert_eq!(again["item"]["id"], id.as_str());

    let ls = ok(w.local(&["review", "ls", "--state", "open"]));
    assert_eq!(ls["items"].as_array().unwrap().len(), 1, "{ls}");
    let none = ok(w.local(&["review", "ls", "--kind", "mergeConflict"]));
    assert_eq!(none["items"], json!([]), "{none}");

    let view = w.show(&id);
    assert_eq!(view["stale"], false, "{view}");
    assert_eq!(view["plan"]["entries"].as_array().unwrap().len(), 2);
    assert_eq!(w.origins(&id).len(), 2);

    let origin = w.origins(&id)[0].clone();
    let c = ok(w.agent(&["review", "comment", &id, "looks fine", "--tx", &origin]));
    assert_eq!(c["comment"]["txId"], origin.as_str(), "{c}");
    assert_eq!(w.show(&id)["comments"].as_array().unwrap().len(), 1);

    // Closing is a reviewer's: --rejected or --dismissed, one of them.
    assert_error(&w.local(&["review", "close", &id]), 2, "USAGE");
    assert_error(
        &w.agent(&["review", "close", &id, "--dismissed"]),
        7,
        "SCOPE_DENIED",
    );
    let closed = ok(w.local(&["review", "close", &id, "--dismissed"]));
    assert_eq!(closed["item"]["state"], "dismissed", "{closed}");
    assert_error(
        &w.local(&["review", "show", "no-such-item"]),
        3,
        "REVIEW_NOT_FOUND",
    );
}

#[test]
fn fin_review_decide_merge_and_the_drop() {
    let w = world!("fin_review_decide_merge_and_the_drop");
    for _ in 0..3 {
        w.edit();
    }
    // The agent may not write the protected branch.
    let blocked = w.as_agent(
        &w.wide,
        &[
            "clip",
            "insert",
            "--track",
            "t1",
            "--at",
            "end",
            "--asset",
            "a1",
            "--no-audio",
            "--dur",
            "1s",
            "--branch",
            "main",
        ],
    );
    assert_error(&blocked, 7, "BRANCH_PROTECTED");
    let id = w.request();
    let origins = w.origins(&id);
    assert_eq!(origins.len(), 3);

    // The reviewer is the local person (owner decision 5): reject one, accept one.
    let decided = ok(w.local(&[
        "review",
        "decide",
        &id,
        "--reject",
        &origins[0],
        "--accept",
        &origins[1],
        "--reason",
        "no",
    ]));
    assert_eq!(
        decided["decisions"].as_array().unwrap().len(),
        2,
        "{decided}"
    );
    // The agent may not decide or merge.
    assert_error(
        &w.agent(&["review", "decide", &id, "--accept", &origins[2]]),
        7,
        "SCOPE_DENIED",
    );
    assert_error(&w.agent(&["review", "merge", &id]), 7, "SCOPE_DENIED");

    // With the policy asking for review, one undecided entry refuses the merge.
    w.console().confirmed_ok(
        "review.policy",
        json!({ "projectId": w.project_id, "requireReview": true }),
    );
    let undecided = w.local(&["review", "merge", &id, "--accept-pending"]);
    assert_error(&undecided, 3, "REVIEW_UNDECIDED");
    assert_eq!(undecided.json["error"]["details"]["pending"], 1);
    ok(w.local(&["review", "decide", &id, "--accept", &origins[2]]));

    let merged = ok(w.local(&["review", "merge", &id]));
    assert_eq!(merged["item"]["state"], "merged", "{merged}");
    assert_eq!(merged["targetMoved"], false);
    assert_eq!(
        merged["merge"]["merged"].as_array().unwrap().len(),
        2,
        "{merged}"
    );
    // The rejected one is a drop on main: a plain merge leaves it out.
    let source = merged["item"]["source"].as_str().unwrap();
    let plain = ok(w.local(&["merge", source, "--skip-conflicting"]));
    let excluded = plain["merge"]["excluded"].as_array().unwrap();
    assert!(
        excluded
            .iter()
            .any(|e| e["txId"] == origins[0].as_str() && e["reason"] == "DROPPED"),
        "{plain}"
    );
}

#[test]
fn fin_review_decide_as_local_works() {
    let w = world!("fin_review_decide_as_local_works");
    w.edit();
    let id = w.request();
    let o = w.origins(&id)[0].clone();
    let d = ok(w.local(&["review", "decide", &id, "--accept", &o]));
    assert_eq!(d["decisions"][0]["decision"], "accept", "{d}");
    let back = ok(w.local(&["review", "decide", &id, "--pending", &o]));
    assert_eq!(back["decisions"], json!([]), "{back}");
    let changes = ok(w.local(&[
        "review",
        "decide",
        &id,
        "--request-changes",
        "--reason",
        "redo",
    ]));
    assert_eq!(changes["item"]["state"], "changesRequested", "{changes}");
}

#[test]
fn fin_review_merge_as_agent_is_scope_denied() {
    let w = world!("fin_review_merge_as_agent_is_scope_denied");
    w.edit();
    let id = w.request();
    let denied = w.agent(&["review", "merge", &id, "--accept-pending"]);
    assert_error(&denied, 7, "SCOPE_DENIED");
    assert_eq!(w.show(&id)["item"]["state"], "open");
}

#[test]
fn expect_taken_from_fresh_get() {
    let w = world!("expect_taken_from_fresh_get");
    w.edit();
    let id = w.request();
    let old = w.show(&id)["item"]["snapshot"].to_string();
    let o = w.origins(&id)[0].clone();
    // A commit lands on the source after the review was planned: the view says stale, a
    // decision against the item's own snapshot still goes through.
    w.edit();
    assert_eq!(w.show(&id)["stale"], true);
    ok(w.local(&["review", "decide", &id, "--accept", &o]));
    // Someone refreshes the item after the old view: that snapshot is stale now, and the one
    // read at the start of the command is not.
    let refreshed = ok(w.local(&["review", "refresh", &id]));
    assert_eq!(refreshed["stale"], false, "{refreshed}");
    let explicit = w.local(&["review", "decide", &id, "--accept", &o, "--expect", &old]);
    assert_error(&explicit, 4, "REVIEW_STALE");
    assert_eq!(explicit.json["error"]["details"]["itemId"], id.as_str());
    let merge_old = w.local(&["review", "merge", &id, "--expect", &old]);
    assert_error(&merge_old, 4, "REVIEW_STALE");
    ok(w.local(&["review", "decide", &id, "--accept", &o]));
    assert_error(
        &w.local(&["review", "decide", &id, "--accept", &o, "--expect", "nope"]),
        2,
        "USAGE",
    );
    // The refreshed item merges both entries, the undecided one by --accept-pending.
    let merged = ok(w.local(&["review", "merge", &id, "--accept-pending"]));
    assert_eq!(
        merged["merge"]["merged"].as_array().unwrap().len(),
        2,
        "{merged}"
    );
}

#[test]
fn review_usage_errors() {
    let w = world!("review_usage_errors");
    w.edit();
    let id = w.request();
    let o = w.origins(&id)[0].clone();
    assert_error(&w.local(&["review", "decide", &id]), 2, "USAGE");
    assert_error(
        &w.local(&["review", "decide", &id, "--accept", &o, "--reject", &o]),
        2,
        "USAGE",
    );
    assert_error(&w.local(&["--dry-run", "review", "merge", &id]), 2, "USAGE");
    assert_error(&w.local(&["review", "request"]), 2, "USAGE");
}

#[test]
fn decide_takes_comma_lists_and_repeated_flags() {
    let w = world!("decide_takes_comma_lists_and_repeated_flags");
    for _ in 0..3 {
        w.edit();
    }
    let id = w.request();
    let o = w.origins(&id);
    // One id per flag, repeatable or comma-separated: the item id is never swallowed.
    let both = format!("{},{}", o[0], o[1]);
    let d = ok(w.local(&[
        "review", "decide", &id, "--accept", &both, "--reject", &o[2],
    ]));
    assert_eq!(d["decisions"].as_array().unwrap().len(), 3, "{d}");
    let again = ok(w.local(&[
        "review",
        "decide",
        &id,
        "--pending",
        &o[0],
        "--pending",
        &o[1],
    ]));
    assert_eq!(again["decisions"].as_array().unwrap().len(), 1, "{again}");
}

#[test]
fn merge_with_nothing_accepted_says_how_to_go_on() {
    let w = world!("merge_with_nothing_accepted_says_how_to_go_on");
    w.edit();
    let id = w.request();
    let r = w.local(&["review", "merge", &id]);
    assert_error(&r, 3, "REVIEW_UNDECIDED");
    assert_eq!(r.json["error"]["details"]["reason"], "nothingAccepted");
    let fix = r.json["error"]["fix"].as_str().unwrap();
    assert!(
        fix.contains("--accept-pending") && fix.contains("decide"),
        "{fix}"
    );
}

#[test]
fn decide_and_merge_pass_the_servers_errors_through() {
    let w = world!("decide_and_merge_pass_the_servers_errors_through");
    w.edit();
    let id = w.request();
    let o = w.origins(&id)[0].clone();
    ok(w.local(&["review", "close", &id, "--dismissed"]));
    assert_error(
        &w.local(&["review", "decide", &id, "--accept", &o]),
        4,
        "REVIEW_CLOSED",
    );
    assert_error(&w.local(&["review", "merge", &id]), 4, "REVIEW_CLOSED");
}

#[test]
fn a_drop_made_after_the_decision_is_reported() {
    let w = world!("a_drop_made_after_the_decision_is_reported");
    w.edit();
    w.edit();
    let id = w.request();
    let o = w.origins(&id);
    ok(w.local(&["review", "decide", &id, "--accept", &o[0]]));
    // Someone else drops the accepted transaction: the merge says which, before writing.
    ok(w.local(&["drop", &o[0], "--from", "work"]));
    let r = w.local(&["review", "merge", &id, "--accept-pending"]);
    assert_error(&r, 4, "REVIEW_STALE");
    assert_eq!(
        r.json["error"]["details"]["reason"], "acceptedDropped",
        "{}",
        r.stdout
    );
    assert_eq!(r.json["error"]["details"]["txIds"], json!([o[0]]));
    // Accepting it again brings it back, and the answer names the drop it cancelled.
    let d = ok(w.local(&["review", "decide", &id, "--accept", &o[0]]));
    let undropped = d["undropped"].as_array().unwrap();
    assert_eq!(undropped.len(), 1, "{d}");
    assert_eq!(undropped[0]["txId"], o[0].as_str());
    let merged = ok(w.local(&["review", "merge", &id, "--accept-pending"]));
    assert_eq!(
        merged["merge"]["merged"].as_array().unwrap().len(),
        2,
        "{merged}"
    );
}
