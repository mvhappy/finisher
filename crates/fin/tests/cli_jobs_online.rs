//! `fin job …` and `fin wait` against a real finisherd (plan 5C-2 Task 2): a `fin look
//! --no-wait` job waited on (exit 0 with a GPU adapter, 4 `JOB_FAILED` with `GPU_UNAVAILABLE`
//! without one), a queued job with dispatch paused (listed, timed out, cancelled, waited on), an
//! agent credential that sees none of `local`'s jobs, and the usage errors.

mod support;

use std::ffi::OsString;

use serde_json::{Value, json};
use support::{Run, Server, assert_error, fin, fin_env, media, ok};

fn gpu_required() -> bool {
    std::env::var("FIN_REQUIRE_GPU").is_ok_and(|v| v == "1")
}

fn job_id(doc: &Value) -> String {
    doc["job"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no job id: {doc}"))
        .to_string()
}

/// `fin wait` of a look: its job succeeded (exit 0), or failed only for want of a GPU adapter.
fn waited_look(run: &Run) -> &'static str {
    match run.code {
        0 => {
            assert_eq!(run.json["jobs"][0]["state"], "succeeded", "{}", run.stdout);
            "succeeded"
        }
        4 => {
            assert_error(run, 4, "JOB_FAILED");
            let failed = &run.json["error"]["details"]["failed"][0];
            assert_eq!(failed["code"], "GPU_UNAVAILABLE", "{}", run.stdout);
            assert!(
                !gpu_required(),
                "FIN_REQUIRE_GPU=1 but no adapter: {}",
                run.stdout
            );
            "failed"
        }
        _ => panic!("fin wait: {}", run.stdout),
    }
}

#[test]
fn look_no_wait_then_fin_wait_and_the_job_commands() {
    let env = vec![("FIN_GPU".to_string(), OsString::from("software"))];
    let Some(server) = Server::start_with("jobs_online", env, None) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    ok(fin(
        sd,
        root,
        &["project", "create", "film", "--name", "Film"],
    ));
    let dir = root.join("film");
    media(&dir.join("clip.mp4"), 2);
    let run = |args: &[&str]| fin(sd, &dir, args);
    ok(run(&["asset", "add", "clip.mp4"]));
    ok(run(&[
        "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "1s",
    ]));

    // `fin look --no-wait`, then `fin wait` on its job (6A-2 Task 13 left this to 5C-2).
    let id = job_id(&ok(run(&[
        "look",
        "--at",
        "0",
        "--size",
        "12x12",
        "--no-wait",
    ])));
    let outcome = waited_look(&run(&["wait", &id, "--timeout", "120"]));
    let shown = ok(run(&["job", "show", &id]));
    assert_eq!(shown["state"], outcome, "{shown}");
    assert_eq!(shown["owner"], json!({ "kind": "local", "id": "local" }));
    if outcome == "succeeded" {
        assert!(shown["result"]["frames"][0]["file"].is_string(), "{shown}");
    }

    // Dispatch paused: a look stays queued.
    assert_eq!(ok(run(&["job", "pause", "--all"])), json!({}));
    let queued = job_id(&ok(run(&[
        "look",
        "--at",
        "0",
        "--size",
        "13x13",
        "--no-wait",
    ])));
    let listed = ok(run(&[
        "job",
        "ls",
        "--state",
        "queued",
        "--kind",
        "media.look",
    ]));
    let ids: Vec<&str> = listed["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [queued.as_str()], "{listed}");
    assert_error(
        &run(&["wait", &queued, "--timeout", "1"]),
        5,
        "WAIT_TIMEOUT",
    );

    // An agent credential (the mint's default scopes: jobs.read and jobs.submit, no
    // jobs.control) sees none of `local`'s jobs: they answer as unknown ones.
    let mut console = support::console::Console::open(&server);
    let minted = console.confirmed_ok(
        "auth.credential.mint",
        json!({ "kind": "agent", "label": "jobs" }),
    );
    let creds = tempfile::tempdir().unwrap();
    let file = creds.path().join("agent.json");
    std::fs::write(&file, minted["file"].to_string()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let agent_env = [("FIN_CREDENTIAL", file.as_os_str())];
    let agent = |args: &[&str]| fin_env(sd, &dir, args, &agent_env);
    for args in [
        vec!["job", "show", &queued],
        vec!["job", "cancel", &queued],
        vec!["job", "pause", &queued],
        vec!["wait", &queued, "--timeout", "1"],
    ] {
        assert_error(&agent(&args), 3, "JOB_NOT_FOUND");
    }
    let unknown = agent(&["job", "show", "01zzzzzzzzzzzzzzzzzzzzzzzz"]);
    assert_error(&unknown, 3, "JOB_NOT_FOUND");
    assert_eq!(ok(agent(&["job", "ls"]))["jobs"], json!([]));
    assert_error(&agent(&["job", "pause", "--all"]), 7, "SCOPE_DENIED");
    // Its own look is its own.
    let mine = job_id(&ok(agent(&[
        "look",
        "--at",
        "0",
        "--size",
        "14x14",
        "--no-wait",
    ])));
    let own = ok(agent(&["job", "show", &mine]));
    assert_eq!(own["owner"]["kind"], "agent", "{own}");
    assert_eq!(ok(agent(&["job", "cancel", &mine]))["state"], "cancelled");

    // `local` cancels its queued look; waiting on it is then JOB_FAILED.
    assert_eq!(ok(run(&["job", "cancel", &queued]))["state"], "cancelled");
    let w = run(&["wait", &queued]);
    assert_error(&w, 4, "JOB_FAILED");
    assert_eq!(
        w.json["error"]["details"]["failed"][0]["state"], "cancelled",
        "{}",
        w.stdout
    );
    assert_eq!(ok(run(&["job", "resume", "--all"])), json!({}));
    // `local` holds jobs.control: it sees the agent's job too, or only its own with --mine.
    let all = ok(run(&["job", "ls"]));
    assert!(
        all["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|j| j["id"] == mine.as_str()),
        "{all}"
    );
    let only = ok(run(&["job", "ls", "--mine"]));
    assert!(
        only["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|j| j["owner"]["kind"] == "local"),
        "{only}"
    );

    // Usage errors never reach the server.
    for args in [
        vec!["job", "pause"],
        vec!["job", "resume", &queued, "--all"],
        vec!["job", "ls", "--state", "done"],
        vec!["wait", &queued, "--timeout", "601"],
    ] {
        let r = run(&args);
        assert_eq!(r.code, 2, "{args:?}: {}", r.stdout);
        assert_eq!(r.json["error"]["code"], "USAGE");
    }
}

/// `server.status` (through the console) reports the job host's caps and, unsandboxed, the warning.
#[test]
fn server_status_reports_job_caps_and_the_sandbox_warning() {
    let Some(server) = Server::start("jobs_status") else {
        return;
    };
    let mut console = support::console::Console::open(&server);
    let status = console.call("server.status", json!({}));
    assert_eq!(status["_tag"], "Success", "{status}");
    let value = &status["value"];
    let caps = &value["jobs"]["caps"];
    assert!(caps["killTree"].is_string(), "{status}");
    assert_eq!(caps["sandboxed"], false, "{status}");
    let warnings = value["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w["code"] == "SANDBOX_DISABLED" && w["sandbox"] == "none"),
        "{status}"
    );
}
