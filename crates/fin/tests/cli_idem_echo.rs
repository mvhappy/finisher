//! What a keyed retry echoes (review #48): the commit its key made, never a second, speculative
//! application of the command planned against the state that commit produced. Also the echo
//! of a commit the server rebased over a concurrent one. Against a real `finisherd`; tests
//! that need it print `SKIPPED <test>: <reason>` and pass when Node or fin-node is missing.

mod support;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::{Server, fin, fin_cmd, finish, media, media_av, ok};

/// A server and a project `film` holding `av.mp4` (10 s, video and audio) as a1 and `v.mp4`
/// (10 s, video only) as a2, with c1 (a2, 0–4 s) and c2 (a2, 4–8 s) on t1.
fn project(test: &str) -> Option<(Server, PathBuf)> {
    let server = Server::start(test)?;
    let dir = server.root.join("film");
    ok(fin(
        &server.sd,
        &server.root,
        &["project", "create", "film", "--name", "Film"],
    ));
    media_av(&dir.join("av.mp4"), 10);
    media(&dir.join("v.mp4"), 10);
    for f in ["av.mp4", "v.mp4"] {
        ok(fin(&server.sd, &dir, &["asset", "add", f]));
    }
    for at in ["0", "end"] {
        ok(fin(
            &server.sd,
            &dir,
            &[
                "clip", "insert", "--track", "t1", "--at", at, "--asset", "a2", "--dur", "4s",
            ],
        ));
    }
    Some((server, dir))
}

macro_rules! setup {
    ($name:literal) => {{
        let Some((server, dir)) = project($name) else {
            return;
        };
        (server, dir)
    }};
}

struct T<'a>(&'a Server, &'a PathBuf);

impl T<'_> {
    fn run(&self, args: &[&str]) -> Value {
        ok(fin(&self.0.sd, self.1, args))
    }
    fn rev(&self) -> Value {
        self.run(&["status"])["rev"].clone()
    }
    fn batch(&self, key: &str, lines: &str) -> Value {
        use std::io::Write as _;
        let args = ["--idem-key", key, "batch"];
        let mut cmd = fin_cmd(&self.0.sd, self.1, &args);
        cmd.stdin(std::process::Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(lines.as_bytes())
            .unwrap();
        ok(finish(&args, child.wait_with_output().unwrap()))
    }
    /// Runs `args` twice (and once more as a `--dry-run`): each retry answers the first commit,
    /// echo included, and changes nothing.
    fn retried(&self, args: &[&str]) -> Value {
        let first = self.run(args);
        let rev = self.rev();
        assert_eq!(rev, first["rev"]);
        let again = self.run(args);
        assert_eq!(again, first, "a retry echoes the commit");
        let mut dry = vec!["--dry-run"];
        dry.extend_from_slice(args);
        assert_eq!(self.run(&dry), first, "a dry-run retry echoes the commit");
        assert_eq!(self.rev(), rev, "a retry commits nothing");
        first
    }
}

#[test]
fn keyed_slip_retry_echoes_the_committed_source_in() {
    let (server, dir) = setup!("keyed_slip_retry_echoes_the_committed_source_in");
    let t = T(&server, &dir);
    let first = t.retried(&["--idem-key", "slip-key", "item", "slip", "c1", "+1s"]);
    assert_eq!(first["resolved"]["sourceIn"], "1", "{first}");
}

#[test]
fn keyed_trim_retry_echoes_the_committed_edge() {
    let (server, dir) = setup!("keyed_trim_retry_echoes_the_committed_edge");
    let t = T(&server, &dir);
    let first = t.retried(&[
        "--idem-key",
        "trim-key",
        "item",
        "trim",
        "c1",
        "--end",
        "+1s",
    ]);
    assert_eq!(first["resolved"]["edgeAt"], "5", "{first}");
    let clip = t.retried(&[
        "--idem-key",
        "clip-trim",
        "clip",
        "trim",
        "c2",
        "--end",
        "-1s",
    ]);
    assert_eq!(clip["resolved"]["edgeAt"], "8", "{clip}");
}

#[test]
fn keyed_slide_and_linked_delete_retries_echo_the_commit() {
    let (server, dir) = setup!("keyed_slide_and_linked_delete_retries_echo_the_commit");
    let t = T(&server, &dir);
    // c1 0–4, c2 4–8, c3 8–12: sliding c2 trims both neighbours.
    t.run(&[
        "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a2", "--dur", "4s",
    ]);
    let slide = t.retried(&["--idem-key", "slide-key", "item", "slide", "c2", "+1s"]);
    assert_eq!(slide["resolved"]["start"], "5", "{slide}");
    // A linked pair: the retry's echo still names both members the delete took.
    t.run(&[
        "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "2s",
    ]);
    let delete = t.retried(&["--idem-key", "del-key", "item", "delete", "c4"]);
    assert_eq!(delete["resolved"]["items"], json!(["c4", "c5"]), "{delete}");
}

#[test]
fn keyed_batch_retry_echoes_the_commit() {
    let (server, dir) = setup!("keyed_batch_retry_echoes_the_commit");
    let t = T(&server, &dir);
    let lines = concat!(
        r#"{"cmd":"clip.trim","args":{"clip":"c1","end":"+1s"}}"#,
        "\n",
        r#"{"cmd":"clip.insert","args":{"track":"t1","at":"end","asset":"a2","dur":"1s"}}"#,
        "\n",
        r#"{"cmd":"clip.trim","args":{"clip":"c2","end":"+1s"}}"#,
        "\n",
    );
    let first = t.batch("batch-key", lines);
    assert_eq!(first["resolved"][0]["resolved"]["edgeAt"], "5", "{first}");
    assert_eq!(first["resolved"][1]["resolved"]["at"], "9", "{first}");
    assert_eq!(first["resolved"][2]["resolved"]["edgeAt"], "10", "{first}");
    let again = t.batch("batch-key", lines);
    assert_eq!(again["rev"], first["rev"]);
    assert_eq!(again["diff"], first["diff"]);
    assert_eq!(again["bindings"], first["bindings"]);
    for (n, line) in again["resolved"].as_array().unwrap().iter().enumerate() {
        let was = &first["resolved"][n]["resolved"];
        for (k, v) in line["resolved"].as_object().unwrap() {
            // A value the commit cannot tell (a line's edge before later lines moved it) is
            // null; anything else is what the commit made.
            assert!(*v == was[k] || v.is_null(), "line {n} {k}: {v} after {was}");
        }
    }
    // The last line's edge is the commit's.
    assert_eq!(again["resolved"][2]["resolved"]["edgeAt"], "10", "{again}");
    assert_eq!(again["resolved"][1], first["resolved"][1]);
    assert_eq!(t.rev(), first["rev"]);
}

#[test]
fn rebased_trim_echoes_the_edge_it_committed() {
    let (server, dir) = setup!("rebased_trim_echoes_the_edge_it_committed");
    let gate = server.root.join("gate");
    let args = ["--idem-key", "trim-1", "item", "trim", "c1", "--end", "+1s"];
    let writer = fin_cmd(&server.sd, &dir, &args)
        .env("FIN_TEST_COMMIT_GATE", &gate)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let ready = || {
        std::fs::read_dir(&server.root)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("gate.ready."))
    };
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "writer never reached the commit gate"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Planned against c1 at 0–4 (edge 5), but a ripple insert at 0 moves c1 to 2–6 first.
    ok(fin(
        &server.sd,
        &dir,
        &[
            "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a2", "--dur", "2s",
        ],
    ));
    std::fs::write(&gate, b"").unwrap();
    let first = ok(finish(&args, writer.wait_with_output().unwrap()));
    assert_eq!(first["resolved"]["edgeAt"], "7", "{first}");
    let again = ok(fin(&server.sd, &dir, &args));
    assert_eq!(again, first);
}
