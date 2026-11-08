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
        .env("FIN_PROJECT_DIR", s.project_dir.as_ref().unwrap())
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
    // 6A-2's `look --out` names a folder the worker writes to: confined the same way, before anything is sent.
    refused(&host.agent(&s, &ws, &["look", "--at", "0", "--out", "/tmp"]));
    refused(&host.agent(&s, &ws, &["look", "--at", "0", "--out", "../../../outside"]));
    let inside = host.agent(&s, &ws, &["look", "--at", "0", "--out", "frames"]);
    assert_ne!(
        inside.json["error"]["code"], "PATH_NOT_ALLOWED",
        "{}",
        inside.stdout
    );
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

/// Review B1: the roots come from the runtime (`FIN_PROJECT_DIR`), never from the model's
/// `--project` or the working directory. `--project` naming any other folder (the filesystem
/// root, a folder with no `finisher.project.json`, anywhere between command words) is
/// `PROJECT_NOT_FOUND` before a file is read; naming the session's own folder works.
#[test]
fn project_flag_cannot_widen_the_roots() {
    let Some(host) = AgentHost::start("project_flag_cannot_widen_the_roots") else {
        return;
    };
    let (dir, project) = host.project();
    let s = host.open(&project, "draft");
    let ws = host.workspace(&dir, &s);
    std::fs::write(ws.join("w.json"), "[]").unwrap();
    let bare = host.server.root.join("bare");
    std::fs::create_dir_all(&bare).unwrap();
    std::fs::write(bare.join("w.json"), "[]").unwrap();
    let bare_text = bare.display().to_string();
    let bare_words = bare.join("w.json").display().to_string();
    let not_this = |run: &Run| assert_error(run, 3, "PROJECT_NOT_FOUND");
    for args in [
        // The review's shape: a global flag between the command words.
        &[
            "caption",
            "--project",
            "/",
            "import",
            "--words",
            "/etc/hosts",
            "--style",
            "s",
            "--track",
            "t1",
        ][..],
        &[
            "--project",
            "/",
            "caption",
            "import",
            "--words",
            "/etc/hosts",
            "--style",
            "s",
            "--track",
            "t1",
        ],
        &[
            "caption",
            "import",
            "--project=/",
            "--words",
            "/etc/hosts",
            "--style",
            "s",
            "--track",
            "t1",
        ],
        &[
            "caption",
            "import",
            "--words",
            "/etc/hosts",
            "--style",
            "s",
            "--track",
            "t1",
            "--project",
            "/",
        ],
        &[
            "caption",
            "--project",
            &bare_text,
            "import",
            "--words",
            &bare_words,
            "--style",
            "s",
            "--track",
            "t1",
        ],
        &["asset", "--project", &bare_text, "add", &bare_words],
        &["view", "--project", &bare_text],
    ] {
        let run = host.agent(&s, &ws, args);
        not_this(&run);
        assert!(
            !run.stdout.contains("127.0.0.1"),
            "{args:?}: {}",
            run.stdout
        );
    }
    // A working directory outside the project gives no root either.
    refused(&host.agent(
        &s,
        &bare,
        &[
            "caption", "import", "--words", "w.json", "--style", "s", "--track", "t1",
        ],
    ));
    // `--project` naming the session's own folder (another spelling of it) works.
    let own = dir.join("workspace").join("..").display().to_string();
    let run = host.agent(
        &s,
        &ws,
        &[
            "caption",
            "--project",
            &own,
            "import",
            "--words",
            "w.json",
            "--track",
            "t9",
            "--style",
            "preset:bold",
        ],
    );
    assert!(
        !["PATH_NOT_ALLOWED", "PROJECT_NOT_FOUND", "USAGE"]
            .contains(&run.json["error"]["code"].as_str().unwrap_or("")),
        "{}",
        run.stdout
    );
    // Without the runtime's FIN_PROJECT_DIR (or with a relative one) nothing is readable.
    let no_dir = support::agent_host::Session {
        project_dir: None,
        ..s.clone()
    };
    let run = host.agent(
        &no_dir,
        &ws,
        &[
            "caption", "import", "--words", "w.json", "--style", "s", "--track", "t9",
        ],
    );
    refused(&run);
    assert_eq!(run.json["error"]["details"]["reason"], "noProject");
    let relative = host.agent_env(
        &no_dir,
        &ws,
        &[
            "caption", "import", "--words", "w.json", "--style", "s", "--track", "t9",
        ],
        &[("FIN_PROJECT_DIR", std::ffi::OsStr::new("../.."))],
    );
    refused(&relative);
}

/// Review L2: `FIN_PROJECT_DIR` is not trusted on its own (a CLI harness sets its own
/// environment): its `finisher.project.json` must name the credential's session project. A
/// folder of another project, or one with no project file, makes nothing readable; the session's
/// own folder still works.
#[test]
fn project_dir_must_name_the_sessions_project() {
    let Some(host) = AgentHost::start("project_dir_must_name_the_sessions_project") else {
        return;
    };
    let (dir, project) = host.project();
    let s = host.open(&project, "draft");
    let ws = host.workspace(&dir, &s);
    std::fs::write(ws.join("w.json"), "[]").unwrap();
    // Another real project, with a words file inside it and inside a workspace-shaped folder.
    let root = &host.server.root;
    ok(host.fin(root, &["project", "create", "other", "--name", "Other"]));
    let other = root.join("other");
    let other_ws = other.join("workspace").join(&s.id);
    std::fs::create_dir_all(&other_ws).unwrap();
    std::fs::write(other.join("w.json"), "[]").unwrap();
    std::fs::write(other_ws.join("w.json"), "[]").unwrap();
    // A folder that is no project at all.
    let bare = root.join("bare");
    std::fs::create_dir_all(bare.join("workspace").join(&s.id)).unwrap();
    std::fs::write(bare.join("w.json"), "[]").unwrap();
    let import = |words: &str| {
        [
            "caption",
            "import",
            "--track",
            "t9",
            "--words",
            words,
            "--style",
            "preset:bold",
        ]
        .map(String::from)
    };
    let run_with = |cwd: &Path, project_dir: &Path, words: &str| {
        let args = import(words);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        host.agent_env(
            &s,
            cwd,
            &args,
            &[("FIN_PROJECT_DIR", project_dir.as_os_str())],
        )
    };
    for (cwd, project_dir, words) in [
        (&other_ws, &other, other.join("w.json")),
        (&other_ws, &other, other_ws.join("w.json")),
        (&ws, &other, other.join("w.json")),
        (&bare, &bare, bare.join("w.json")),
    ] {
        let run = run_with(cwd, project_dir, &words.display().to_string());
        refused(&run);
        assert_eq!(
            run.json["error"]["details"]["reason"], "projectMismatch",
            "{}",
            run.stdout
        );
    }
    // The session's own folder works (past the confinement: the track does not exist).
    let run = run_with(&ws, &dir, "w.json");
    assert_ne!(
        run.json["error"]["code"], "PATH_NOT_ALLOWED",
        "{}",
        run.stdout
    );
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
