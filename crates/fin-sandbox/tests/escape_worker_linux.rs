//! The `worker` profile's Landlock phase (plan 7C Task 2, sp6 §3.5.1): `fin-sandbox-probe
//! --self` applies a `SelfSandbox {profile: "worker", args: WorkerInput}` through
//! `fin_sandbox::self_apply`, as `fin-workers` does before `hello`, then makes one attempt.
//! A denial passes only on an observed `EACCES`/`EPERM`/`EXDEV`.
#![cfg(target_os = "linux")]

mod linux_support;

use std::path::PathBuf;

use fin_jobs::sandbox::SelfSandbox;
use fin_sandbox::{DeviceAccess, DeviceRule, WorkerInput, WorkerScope};
use linux_support::{Outcome, json_file, landlock_or_skip, listener, probe, s, tempdir, write};

/// Server data with `auth/`, two jobs and a font cache; sources outside it.
struct Tree {
    _d: tempfile::TempDir,
    t: PathBuf,
}

impl Tree {
    fn new() -> Tree {
        let (_d, t) = tempdir();
        for (p, text) in [
            ("srv/auth/local.key", "k"),
            ("srv/settings.json", "{}"),
            ("srv/jobs/j1/in/manifest.json", "{}"),
            ("srv/jobs/j1/marker", "m"),
            ("srv/jobs/j2/in/manifest.json", "{}"),
            ("srv/jobs/w1/jobs/j3/in/manifest.json", "{}"),
            ("srv/cache/fonts/a.ttf", "font"),
            ("src/a.mp4", "a"),
            ("src/b.mp4", "b"),
            ("lib/photos/c.jpg", "c"),
            ("other-lib/d.jpg", "d"),
            ("home/.ssh/id_ed25519", "x"),
            ("dev/renderD128", ""),
            ("dev/card0", ""),
        ] {
            write(&t.join(p), text);
        }
        for d in ["srv/jobs/j1/out", "srv/jobs/j1/tmp", "srv/jobs/w1/tmp"] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
        Tree { _d, t }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.t.join(rel)
    }

    fn cold(&self) -> WorkerInput {
        WorkerInput {
            tmp: self.p("srv/jobs/j1/tmp"),
            caches: vec![self.p("srv/cache/fonts")],
            devices: vec![
                DeviceRule {
                    path: self.p("dev/renderD128"),
                    access: DeviceAccess::ReadWrite,
                },
                DeviceRule {
                    path: self.p("dev/card0"),
                    access: DeviceAccess::Read,
                },
            ],
            server_data: self.p("srv"),
            scope: WorkerScope::Cold {
                job_dir: self.p("srv/jobs/j1"),
                sources: vec![self.p("src/a.mp4")],
            },
        }
    }

    fn warm(&self) -> WorkerInput {
        WorkerInput {
            tmp: self.p("srv/jobs/w1/tmp"),
            caches: vec![],
            devices: vec![],
            server_data: self.p("srv"),
            scope: WorkerScope::Warm {
                worker_dir: self.p("srv/jobs/w1"),
                library_roots: vec![self.p("lib")],
            },
        }
    }

    /// One attempt under the worker profile built from `input`.
    fn attempt(&self, input: &WorkerInput, op: &[&str], thread: bool) -> Outcome {
        let req = SelfSandbox {
            profile: "worker".into(),
            args: serde_json::to_value(input).unwrap(),
        };
        let mut flags = vec![];
        if thread {
            flags.push("--thread".to_string());
        }
        flags.push("--self".into());
        flags.push(json_file(&self.t, "self.json", &req));
        let op: Vec<String> = op.iter().map(|a| a.replace("$T", &s(&self.t))).collect();
        probe(&flags, &op)
    }
}

fn check(test: &str, input: impl Fn(&Tree) -> WorkerInput, cases: &[(&[&str], bool)]) {
    if landlock_or_skip(test).is_none() {
        return;
    }
    let mut failures = vec![];
    for (op, allow) in cases {
        // A fresh tree per case: cases create, rename and unlink.
        let tree = Tree::new();
        let got = tree.attempt(&input(&tree), op, false);
        let ok = if *allow {
            got == Outcome::Ran(None)
        } else {
            got.denied()
        };
        if !ok {
            failures.push(format!("{op:?} (allow: {allow}): {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{test}: {failures:#?}");
}

const ALLOW: bool = true;
const DENY: bool = false;

#[test]
fn worker_cold_reads_only_manifest() {
    check(
        "worker_cold_reads_only_manifest",
        Tree::cold,
        &[
            (&["read", "$T/src/a.mp4"], ALLOW),
            (&["read", "$T/srv/jobs/j1/in/manifest.json"], ALLOW),
            (&["read", "$T/srv/cache/fonts/a.ttf"], ALLOW),
            // A source not in the manifest, and the directory of one that is (per-file grant).
            (&["read", "$T/src/b.mp4"], DENY),
            (&["list", "$T/src"], DENY),
            // Server data: auth, settings, another job, the job directory's own files.
            (&["read", "$T/srv/auth/local.key"], DENY),
            (&["read", "$T/srv/settings.json"], DENY),
            (&["read", "$T/srv/jobs/j2/in/manifest.json"], DENY),
            (&["read", "$T/srv/jobs/j1/marker"], DENY),
            (&["list", "$T/srv/jobs"], DENY),
            (&["read", "$T/home/.ssh/id_ed25519"], DENY),
            (&["read", "/etc/passwd"], DENY),
            // Devices: read-write and read-only nodes.
            (&["write", "$T/dev/renderD128"], ALLOW),
            (&["read", "$T/dev/card0"], ALLOW),
            (&["write", "$T/dev/card0"], DENY),
        ],
    );
}

#[test]
fn worker_writes_only_out_and_tmp() {
    check(
        "worker_writes_only_out_and_tmp",
        Tree::cold,
        &[
            (&["create", "$T/srv/jobs/j1/out/frame.png"], ALLOW),
            (&["create", "$T/srv/jobs/j1/tmp/scratch"], ALLOW),
            (&["mkdir", "$T/srv/jobs/j1/tmp/d"], ALLOW),
            // `in/` is read-only, the job directory itself is not writable.
            (&["create", "$T/srv/jobs/j1/in/x"], DENY),
            (&["write", "$T/srv/jobs/j1/in/manifest.json"], DENY),
            (&["truncate", "$T/srv/jobs/j1/in/manifest.json"], DENY),
            (&["unlink", "$T/srv/jobs/j1/in/manifest.json"], DENY),
            (&["create", "$T/srv/jobs/j1/x"], DENY),
            (&["rename", "$T/srv/jobs/j1/out", "$T/srv/jobs/j1/o2"], DENY),
            (&["create", "$T/srv/x"], DENY),
            (&["write", "$T/src/a.mp4"], DENY),
            (&["create", "$T/srv/cache/fonts/b.ttf"], DENY),
            // No hard link or rename brings a read-only or unlisted file into `out/`.
            (&["link", "$T/src/a.mp4", "$T/srv/jobs/j1/out/a"], DENY),
            (
                &["link", "$T/srv/auth/local.key", "$T/srv/jobs/j1/out/k"],
                DENY,
            ),
            (
                &[
                    "rename",
                    "$T/srv/jobs/j1/in/manifest.json",
                    "$T/srv/jobs/j1/out/m",
                ],
                DENY,
            ),
            // No FIFO, no socket file, no device node.
            (&["mkfifo", "$T/srv/jobs/j1/tmp/p"], DENY),
        ],
    );
}

#[test]
fn worker_warm_sees_only_its_dir() {
    check(
        "worker_warm_sees_only_its_dir",
        Tree::warm,
        &[
            (&["read", "$T/srv/jobs/w1/jobs/j3/in/manifest.json"], ALLOW),
            (&["create", "$T/srv/jobs/w1/jobs/j3/out"], ALLOW),
            (&["read", "$T/lib/photos/c.jpg"], ALLOW),
            (&["list", "$T/lib/photos"], ALLOW),
            // A sibling job under the jobs root, another library, server data.
            (&["read", "$T/srv/jobs/j1/in/manifest.json"], DENY),
            (&["read", "$T/srv/jobs/j2/in/manifest.json"], DENY),
            (&["list", "$T/srv/jobs"], DENY),
            (&["read", "$T/other-lib/d.jpg"], DENY),
            (&["read", "$T/srv/auth/local.key"], DENY),
            (&["create", "$T/lib/photos/new.jpg"], DENY),
        ],
    );
}

#[test]
fn worker_cannot_exec_or_reach_the_network() {
    if landlock_or_skip("worker_cannot_exec_or_reach_the_network").is_none() {
        return;
    }
    let (_l, port) = listener();
    let port = port.to_string();
    let tree = Tree::new();
    let input = tree.cold();
    let own = std::env::current_exe().unwrap();
    for op in [
        vec!["exec", "/bin/sh"],
        vec!["exec", "/bin/true"],
        // Not even a copy of a program in its own write root.
        vec!["exec", own.to_str().unwrap()],
        vec!["connect-tcp", port.as_str()],
        vec!["bind-tcp", "0"],
        vec!["signal-parent"],
    ] {
        let got = tree.attempt(&input, &op, false);
        assert!(got.denied(), "{op:?}: {got:?}");
    }
    let copy = tree.p("srv/jobs/j1/tmp/prog");
    std::fs::copy("/bin/true", &copy).unwrap();
    let got = tree.attempt(&input, &["exec", copy.to_str().unwrap()], false);
    assert!(got.denied(), "exec from tmp: {got:?}");
}

#[test]
fn worker_profile_reaches_threads_started_before_it() {
    let Some(abi) = landlock_or_skip("worker_profile_reaches_threads_started_before_it") else {
        return;
    };
    let tree = Tree::new();
    let got = tree.attempt(&tree.cold(), &["read", "$T/srv/auth/local.key"], true);
    if abi.0 >= fin_sandbox::linux::landlock::TSYNC_ABI {
        assert!(got.denied(), "{got:?}");
    } else {
        assert!(
            matches!(got, Outcome::Apply(ref e) if e.contains("threads")),
            "{got:?}"
        );
    }
}

#[test]
fn self_apply_refuses_what_it_cannot_build() {
    // Pure: no kernel involved. Each is an `Err`, which `fin-jobs` turns into exit 78.
    let tree = Tree::new();
    let mut bad = tree.cold();
    bad.tmp = PathBuf::from("relative/tmp");
    for (req, want) in [
        (
            SelfSandbox {
                profile: "worker".into(),
                args: serde_json::Value::Null,
            },
            "unreadable worker input",
        ),
        (
            SelfSandbox {
                profile: "worker".into(),
                args: serde_json::to_value(&bad).unwrap(),
            },
            "not absolute",
        ),
        (
            SelfSandbox {
                profile: "agent".into(),
                args: serde_json::Value::Null,
            },
            "not applied by the process itself",
        ),
    ] {
        let e = fin_sandbox::self_apply::policy(&req).unwrap_err();
        assert!(e.contains(want), "{e}");
    }
    // A job directory outside the jobs root is refused before anything is applied.
    let mut outside = tree.cold();
    outside.scope = WorkerScope::Cold {
        job_dir: tree.p("src"),
        sources: vec![],
    };
    let req = SelfSandbox {
        profile: "worker".into(),
        args: serde_json::to_value(&outside).unwrap(),
    };
    assert!(fin_sandbox::self_apply::policy(&req).is_err());
}
