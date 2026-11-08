//! Client-side file arguments under an agent session's credential (plan 7A Task 8b, B4;
//! Decision 14c): a path inside the session's workspace or the project folder works; one outside
//! (absolute, `..`, a symlink to the server's key) is `PATH_NOT_ALLOWED` before the file is
//! opened; without an agent credential nothing changes.

mod support;

use std::path::Path;

use support::agent_host::{AgentHost, Session};
use support::{Run, assert_error, ok};

fn refused(run: &Run) {
    assert_error(run, 7, "PATH_NOT_ALLOWED");
}

fn batch(host: &AgentHost, s: &Session, cwd: &Path, line: &str) -> Run {
    use std::io::Write as _;
    let mut cmd = support::fin_cmd(&host.server.sd, cwd, &["batch"]);
    cmd.env("FIN_CREDENTIAL", &s.credential)
        .env("FIN_SERVER", &host.server.socket)
        .stdin(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(line.as_bytes())
        .unwrap();
    support::finish(&["batch"], child.wait_with_output().unwrap())
}

#[test]
fn file_args_confined() {
    let Some(host) = AgentHost::start("file_args_confined") else {
        return;
    };
    let (dir, project) = host.project();
    let s = host.open(&project, "draft");
    let ws = host.workspace(&dir, &s);
    // Inside: the workspace (relative to it) and the project folder.
    support::media(&ws.join("take.mp4"), 2);
    ok(host.agent(&s, &ws, &["asset", "add", "take.mp4"]));
    ok(host.agent(&s, &ws, &["asset", "add", "../../clip.mp4"]));
    let words = ws.join("w.json");
    std::fs::write(&words, "[]").unwrap();
    let import = |path: &str| {
        host.agent(
            &s,
            &ws,
            &[
                "caption",
                "import",
                "--track",
                "t9",
                "--words",
                path,
                "--style",
                "preset:bold",
            ],
        )
    };
    // Past the confinement (the track does not exist): anything but PATH_NOT_ALLOWED.
    let inside = import("w.json");
    assert_ne!(
        inside.json["error"]["code"], "PATH_NOT_ALLOWED",
        "{}",
        inside.stdout
    );

    // Outside: absolute, `..` escapes, and a symlink to the server's own key.
    let key = host.server.sd.join("auth").join("local.key");
    let key_text = key.display().to_string();
    refused(&import("/etc/hosts"));
    refused(&import("../../../server-data/auth/local.key"));
    refused(&import(&key_text));
    refused(&host.agent(&s, &ws, &["asset", "add", "/etc/hosts"]));
    refused(&host.agent(
        &s,
        &ws,
        &[
            "caption",
            "import",
            "--track",
            "t9",
            "--srt",
            "/etc/hosts",
            "--style",
            "preset:bold",
        ],
    ));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&key, ws.join("key.json")).unwrap();
        refused(&import("key.json"));
        std::os::unix::fs::symlink(host.server.sd.join("auth"), ws.join("auth")).unwrap();
        refused(&host.agent(&s, &ws, &["asset", "add", "auth/local.key"]));
    }
    // A `batch` line's path is confined too.
    let run = batch(
        &host,
        &s,
        &ws,
        &format!("{{\"cmd\":\"asset.add\",\"args\":{{\"path\":\"{key_text}\"}}}}\n"),
    );
    refused(&run);
    // Before the file is opened: an unreadable file outside is refused as outside, not as
    // unreadable (fin never tried to read it).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let closed = host.server.root.join("closed.json");
        std::fs::write(&closed, "[]").unwrap();
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
        let run = import(&closed.display().to_string());
        refused(&run);
        assert_eq!(run.json["error"]["details"]["reason"], "outside");
    }
    // The key never reaches a project: no asset names it.
    let view = ok(host.fin(&dir, &["view", "--branch", s.root.as_ref().unwrap()]));
    assert!(!view.to_string().contains("local.key"), "{view}");

    // Without an agent credential nothing changes: a person's fin reads where it likes.
    let person = host.fin(
        &dir,
        &[
            "caption",
            "import",
            "--track",
            "t9",
            "--words",
            "/etc/hosts",
            "--style",
            "preset:bold",
        ],
    );
    assert_ne!(
        person.json["error"]["code"], "PATH_NOT_ALLOWED",
        "{}",
        person.stdout
    );
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
