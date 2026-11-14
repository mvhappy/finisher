//! Media workers under 7C's Linux Landlock `worker` profile (plan 6B-1 Task 18, sp6 §3.5.1).
//! Each worker gets the `WorkerInput` the job host's `SandboxSpawner` maps a media job's
//! grants to (`fin_sandbox::spawner::grants::worker_input`, family `media`: `in/` read, `out/`
//! and the private temp written, the job's sources, the font cache and the named media cache
//! entries read, the GPU rows), applied by `fin-workers media` itself before `hello`.
//!
//! The escape rows run inside a real `fin-workers media` process (`media.testEscape`, a
//! `test-hooks` kind that only makes file-system attempts) and follow 7C's escape suite: each
//! denial is an observed `EACCES`/`EPERM`/`EXDEV`, and every denied attempt succeeds in an
//! unconfined media worker on a fresh tree (the control), so the denial is the profile's. The
//! positive rows run `media.look`, `media.proxy` and `media.thumbs` under the profile on the
//! GPU `FIN_GPU` names (lavapipe in CI) and compare every output byte with an unconfined run.
//! Skipped without Landlock (or without a GPU for the media rows) unless
//! `FIN_REQUIRE_LANDLOCK=1` (`FIN_REQUIRE_GPU=1`).
#![cfg(target_os = "linux")]
// Tests build fixtures and read outputs by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

mod media_support;
mod support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fin_jobhost::spawner::{PathGrant, Profile, Program, Qos, Role, SpawnRequest};
use fin_jobs::sandbox::{ENV, NONE, SelfSandbox};
use fin_jobs::{EXIT_SELF_SANDBOX, FileMode, HostMsg};
use fin_media::backend::VendorClass;
use fin_media::jobs::{
    ManifestProxy, ProxyCodec, ProxyParams, ProxySourceAudio, ProxyTier, RenderManifest,
    ThumbsMode, ThumbsParams, proxy_key, thumbs_key,
};
use fin_sandbox::WorkerInput;
use fin_sandbox::linux::landlock::{Support, query};
use fin_sandbox::spawner::grants::worker_input;
use fin_sandbox::status::TEST_ENV;
use media_support::{Fixture, gpu_or_skip, make_proxy, r, worker_env};
use serde_json::{Value, json};
use support::{End, Job, Proc, Run, no_calls, spawn_with};

const EPERM: i32 = 1;
const EACCES: i32 = 13;
const EXDEV: i32 = 18;

fn landlock(test: &str) -> bool {
    match query() {
        Support::Abi(_) => true,
        other => {
            assert_ne!(
                std::env::var("FIN_REQUIRE_LANDLOCK").as_deref(),
                Ok("1"),
                "{test}: FIN_REQUIRE_LANDLOCK=1 but Landlock answers {other:?}"
            );
            eprintln!("{test}: skipped, Landlock is not available ({other:?})");
            false
        }
    }
}

/// A server, a project and a home, canonical: server data with a cold job, another job, the
/// font cache and two media cache entries (one the job names); the project's `sources/` with
/// the job's source and another; `~/.ssh` of a test home.
struct Tree {
    _d: tempfile::TempDir,
    base: PathBuf,
    job: Job,
}

const NAMED_KEY: &str = "ab11111111111111111111111111111111111111111111111111111111111111";
const OTHER_KEY: &str = "cd22222222222222222222222222222222222222222222222222222222222222";
const FONT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

impl Tree {
    fn new() -> Tree {
        let d = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(d.path()).unwrap();
        for (p, text) in [
            ("srv/auth/local.key", "key"),
            ("srv/settings.json", "{}"),
            ("srv/jobs/other/in/manifest.json", "{}"),
            (&format!("srv/fonts/{FONT}"), "font"),
            (&format!("srv/cache/media/ab/{NAMED_KEY}/proxy.mp4"), "p"),
            (&format!("srv/cache/media/ab/{NAMED_KEY}/table.bin"), "t"),
            (&format!("srv/cache/media/ab/{NAMED_KEY}/meta.json"), "{}"),
            (&format!("srv/cache/media/cd/{OTHER_KEY}/proxy.mp4"), "q"),
            ("project/project.json", "{}"),
            ("project/sources/a.mp4", "a"),
            ("project/sources/b.mp4", "b"),
            ("home/.ssh/id_ed25519", "secret"),
        ] {
            let p = base.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let job = server_job(&base.join("srv"));
        std::fs::write(job.path("in/manifest.json"), "{}").unwrap();
        std::fs::write(job.path("out/frame.png"), "png").unwrap();
        Tree { _d: d, base, job }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.base.join(rel)
    }

    fn entry(&self, key: &str) -> PathBuf {
        self.p(&format!("srv/cache/media/{}/{key}", &key[..2]))
    }

    /// A cold look's grants as `finisherd` makes them: its source, the font cache and the files
    /// of the proxy entry it lists.
    fn input(&self) -> WorkerInput {
        let entry = self.entry(NAMED_KEY);
        media_input(
            &self.p("srv"),
            &self.job,
            &[
                self.p("project/sources/a.mp4"),
                self.p("srv/fonts"),
                entry.join("proxy.mp4"),
                entry.join("table.bin"),
                entry.join("meta.json"),
            ],
        )
    }
}

/// A job directory `<srv>/jobs/<random>/` with `in/`, `out/` and `tmp/`.
fn server_job(srv: &Path) -> Job {
    std::fs::create_dir_all(srv.join("jobs")).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("job-")
        .tempdir_in(srv.join("jobs"))
        .unwrap();
    for d in ["in", "out", "tmp"] {
        std::fs::create_dir_all(dir.path().join(d)).unwrap();
    }
    Job { dir }
}

/// The `worker` input the job host's spawner maps a cold media job reading `reads` to.
fn media_input(srv: &Path, job: &Job, reads: &[PathBuf]) -> WorkerInput {
    let g = |path: PathBuf, mode| PathGrant { path, mode };
    let mut grants = vec![
        g(job.path("in/manifest.json"), FileMode::Read),
        g(job.path("in/snapshot.json"), FileMode::Read),
        g(job.out(), FileMode::ReadWrite),
        g(job.path("tmp"), FileMode::ReadWrite),
    ];
    grants.extend(reads.iter().map(|p| g(p.clone(), FileMode::Read)));
    let req = SpawnRequest {
        role: Role::Worker,
        profile: Profile::Worker,
        family: "media".into(),
        program: Program::Bundled("fin-workers".into()),
        args: vec![],
        env: vec![(OsString::from("TMPDIR"), job.path("tmp").into())],
        cwd: job.dir.path().to_path_buf(),
        grants,
        qos: Qos::Normal,
        label: "media".into(),
        limits: Default::default(),
    };
    let input = worker_input(&req, srv).expect("the spawner maps a media job");
    assert!(
        !input.devices.is_empty(),
        "the media family gets the GPU rows"
    );
    input
}

fn request(input: &WorkerInput) -> (&'static str, String) {
    let req = SelfSandbox {
        profile: "worker".into(),
        args: serde_json::to_value(input).unwrap(),
    };
    (ENV, serde_json::to_string(&req).unwrap())
}

/// `fin-workers media` under `input` (`None`: unconfined), past its `hello`.
fn media(input: Option<&WorkerInput>, extra: &[(&'static str, String)]) -> Proc {
    let mut env = worker_env();
    env.extend(input.map(request));
    env.extend_from_slice(extra);
    let mut p = spawn_with(&["media"], &env);
    let h = p.hello();
    let want = input.map(|_| "worker");
    if extra.iter().all(|(k, _)| *k != TEST_ENV) {
        assert_eq!(h.self_sandbox.as_deref(), want, "the worker's hello");
    }
    p
}

/// Runs `kind` once on a fresh worker.
fn run(input: Option<&WorkerInput>, job: &Job, kind: &str, params: Value, files: &[&str]) -> Run {
    let mut p = media(input, &[]);
    p.send(&HostMsg::Start(job.start(kind, params, files)));
    p.drive(no_calls)
}

/// The attempts' `errno`s (`None`: it succeeded), made by a media worker under `input`.
fn attempts(input: Option<&WorkerInput>, job: &Job, list: &[Value]) -> Vec<Option<i32>> {
    let run = run(
        input,
        job,
        "media.testEscape",
        json!({ "attempts": list }),
        &[],
    );
    serde_json::from_value(run.result()["errnos"].clone()).unwrap()
}

/// Each case under the profile (fresh tree), then each denied one unconfined (fresh tree).
fn check(test: &str, cases: &[(Value, bool)]) {
    let ops = |t: &Tree| -> Vec<Value> {
        let base = t.base.to_str().unwrap().to_owned();
        let job = t.job.dir.path().to_str().unwrap().to_owned();
        cases
            .iter()
            .map(|(op, _)| {
                let s = op.to_string().replace("$T", &base).replace("$J", &job);
                serde_json::from_str(&s).unwrap()
            })
            .collect()
    };
    let tree = Tree::new();
    let got = attempts(Some(&tree.input()), &tree.job, &ops(&tree));
    let control_tree = Tree::new();
    let control = attempts(None, &control_tree.job, &ops(&control_tree));
    let mut failures = vec![];
    for (((op, allow), got), control) in cases.iter().zip(got).zip(control) {
        let denied = matches!(got, Some(EACCES | EPERM | EXDEV));
        if *allow && got.is_some() {
            failures.push(format!("{op} should succeed: errno {got:?}"));
        }
        if !*allow && !denied {
            failures.push(format!("{op} should be denied: {got:?}"));
        }
        if !*allow && control.is_some() {
            failures.push(format!("{op} unconfined (control): errno {control:?}"));
        }
    }
    assert!(failures.is_empty(), "{test}: {failures:#?}");
}

fn create(path: &str) -> Value {
    json!({ "op": "create", "path": path })
}

fn read(path: &str) -> Value {
    json!({ "op": "read", "path": path })
}

const ALLOW: bool = true;
const DENY: bool = false;

#[test]
fn media_writes_outside_out_are_denied() {
    if !landlock("media_writes_outside_out_are_denied") {
        return;
    }
    // `/tmp` (the system temp) outside the worker's private temp.
    let elsewhere = tempfile::tempdir().unwrap();
    let outside = elsewhere
        .path()
        .join("planted")
        .to_str()
        .unwrap()
        .to_owned();
    check(
        "media_writes_outside_out_are_denied",
        &[
            (create("$J/out/new.png"), ALLOW),
            (create("$J/tmp/scratch"), ALLOW),
            (create("$J/in/x"), DENY),
            (
                json!({ "op": "write", "path": "$J/in/manifest.json" }),
                DENY,
            ),
            (create("$J/x"), DENY),
            (
                create(&format!("$T/srv/cache/media/ab/{NAMED_KEY}/x")),
                DENY,
            ),
            (
                json!({ "op": "write", "path": format!("$T/srv/cache/media/ab/{NAMED_KEY}/proxy.mp4") }),
                DENY,
            ),
            (create("$T/srv/cache/media/x"), DENY),
            (create("$T/srv/fonts/x"), DENY),
            (create("$T/project/planted.txt"), DENY),
            (create("$T/project/sources/planted.mp4"), DENY),
            (
                json!({ "op": "write", "path": "$T/project/sources/a.mp4" }),
                DENY,
            ),
            (create(&outside), DENY),
        ],
    );
}

#[test]
fn media_reads_outside_the_manifest_are_denied() {
    if !landlock("media_reads_outside_the_manifest_are_denied") {
        return;
    }
    check(
        "media_reads_outside_the_manifest_are_denied",
        &[
            (read("$T/project/sources/a.mp4"), ALLOW),
            (read("$J/in/manifest.json"), ALLOW),
            (read(&format!("$T/srv/fonts/{FONT}")), ALLOW),
            (
                read(&format!("$T/srv/cache/media/ab/{NAMED_KEY}/proxy.mp4")),
                ALLOW,
            ),
            // Another source in the same library root, and the project's own files.
            (read("$T/project/sources/b.mp4"), DENY),
            (read("$T/project/project.json"), DENY),
            // A cache entry the job does not name.
            (
                read(&format!("$T/srv/cache/media/cd/{OTHER_KEY}/proxy.mp4")),
                DENY,
            ),
            // Server data and another job.
            (read("$T/srv/auth/local.key"), DENY),
            (read("$T/srv/settings.json"), DENY),
            (read("$T/srv/jobs/other/in/manifest.json"), DENY),
            // A test home's `~/.ssh`.
            (read("$T/home/.ssh/id_ed25519"), DENY),
        ],
    );
}

#[test]
fn a_link_out_of_out_is_denied() {
    if !landlock("a_link_out_of_out_is_denied") {
        return;
    }
    let elsewhere = tempfile::tempdir().unwrap();
    let outside = elsewhere.path().join("moved").to_str().unwrap().to_owned();
    let link = |path: &str, to: &str| json!({ "op": "link", "path": path, "to": to });
    check(
        "a_link_out_of_out_is_denied",
        &[
            // Out of `out/`: a hard link or a rename of an output anywhere else.
            (link("$J/out/frame.png", "$J/in/frame.png"), DENY),
            (link("$J/out/frame.png", "$T/project/frame.png"), DENY),
            (
                link("$J/out/frame.png", "$T/srv/cache/media/frame.png"),
                DENY,
            ),
            // Into `out/`: a link to a file the worker may only read, or may not read at all.
            (link("$T/project/sources/a.mp4", "$J/out/a.mp4"), DENY),
            (link("$T/srv/auth/local.key", "$J/out/k"), DENY),
            // A symbolic link in `out/` is made, but reading through it is still refused.
            (
                json!({ "op": "symlink", "target": "$T/home/.ssh/id_ed25519", "to": "$J/out/l" }),
                ALLOW,
            ),
            (read("$J/out/l"), DENY),
            // Last: it moves the output away (in the control too).
            (
                json!({ "op": "rename", "path": "$J/out/frame.png", "to": outside }),
                DENY,
            ),
        ],
    );
}

/// Every file below `dir`, by relative path, with its bytes.
fn tree_bytes(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(dir).unwrap().to_str().unwrap().to_owned();
                out.push((rel, std::fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

/// Server data with a job holding the fixture's inputs (`manifest` when given).
struct Served {
    _d: tempfile::TempDir,
    srv: PathBuf,
    job: Job,
}

fn served(fx: &Fixture, manifest: Option<&RenderManifest>) -> Served {
    let d = tempfile::tempdir().unwrap();
    let srv = std::fs::canonicalize(d.path()).unwrap();
    let job = server_job(&srv);
    let plain = fx.job(manifest);
    for f in ["in/snapshot.json", "in/manifest.json"] {
        std::fs::copy(plain.path(f), job.path(f)).unwrap();
    }
    Served { _d: d, srv, job }
}

/// Runs `kind` under the profile (reads `reads`) and unconfined, each in a fresh job of its
/// own server, and asserts both succeed with the same bytes in `out/`.
fn same_bytes_confined(
    fx: &Fixture,
    manifest: Option<&RenderManifest>,
    kind: &str,
    params: Value,
    reads: impl Fn(&Path) -> Vec<PathBuf>,
) {
    let files = ["in/snapshot.json", "in/manifest.json", "out/"];
    let confined = served(fx, manifest);
    let input = media_input(&confined.srv, &confined.job, &reads(&confined.srv));
    let a = run(Some(&input), &confined.job, kind, params.clone(), &files);
    let open = served(fx, manifest);
    let b = run(None, &open.job, kind, params, &files);
    for (run, how) in [(&a, "confined"), (&b, "unconfined")] {
        assert!(
            matches!(run.end, End::Result(_)),
            "{kind} {how}: {:?}",
            run.end
        );
        let failed: Vec<_> = run
            .events
            .iter()
            .filter(|e| matches!(e, support::Ev::ItemFailed(..)))
            .collect();
        assert!(failed.is_empty(), "{kind} {how}: {failed:?}");
    }
    assert_eq!(a.result(), b.result(), "{kind}: the results");
    let (x, y) = (tree_bytes(&confined.job.out()), tree_bytes(&open.job.out()));
    assert!(!x.is_empty(), "{kind}: no output");
    assert_eq!(
        x.iter().map(|f| &f.0).collect::<Vec<_>>(),
        y.iter().map(|f| &f.0).collect::<Vec<_>>(),
        "{kind}: the files"
    );
    for ((name, a), (_, b)) in x.iter().zip(&y) {
        if name.ends_with("meta.json") {
            // The entry's creation time is the one field a second run may not repeat.
            let parsed = |b: &[u8]| {
                let mut v: Value = serde_json::from_slice(b).unwrap();
                v.as_object_mut().unwrap().remove("createdAt");
                v
            };
            assert_eq!(parsed(a), parsed(b), "{kind}: {name}");
        } else {
            assert!(a == b, "{kind}: {name} differs from the unconfined run");
        }
    }
}

fn sources(fx: &Fixture) -> Vec<PathBuf> {
    vec![
        std::fs::canonicalize(&fx.av1).unwrap(),
        std::fs::canonicalize(&fx.h264).unwrap(),
    ]
}

#[test]
fn look_proxy_and_thumbs_run_under_the_profile_with_identical_bytes() {
    let test = "look_proxy_and_thumbs_run_under_the_profile_with_identical_bytes";
    if !landlock(test) || !gpu_or_skip(test) {
        return;
    }
    let fx = Fixture::new();
    let fp = fx.manifest().asset(&fx.av1_id).unwrap().fingerprint.clone();
    // Before 2 s: the H.264 clip after it has no decoder on Linux.
    let times = [r("0"), r("1")];
    // `media.look` from the originals.
    let look = serde_json::to_value(fx.params(&times)).unwrap();
    same_bytes_confined(&fx, None, "media.look", look.clone(), |_| sources(&fx));

    // `media.look` from a proxy entry in the media cache, granted file by file.
    let proxy = make_proxy(&fx);
    let placed = |srv: &Path| -> PathBuf {
        let dir = srv
            .join("cache/media")
            .join(&proxy.key[..2])
            .join(&proxy.key);
        if !dir.exists() {
            std::fs::create_dir_all(&dir).unwrap();
            for f in ["proxy.mp4", "table.bin", "meta.json"] {
                std::fs::copy(proxy.dir.join(f), dir.join(f)).unwrap();
            }
        }
        dir
    };
    // The manifest names the entry under each server's own cache: written per run below.
    let with_proxy = |srv: &Path| -> RenderManifest {
        let mut m = fx.manifest();
        for a in &mut m.assets {
            if a.asset == fx.av1_id {
                a.proxies = vec![ManifestProxy {
                    tier: ProxyTier::Preview,
                    key: proxy.key.clone(),
                    dir: placed(srv).to_str().unwrap().into(),
                    codec: ProxyCodec::Av1,
                }];
            }
        }
        m
    };
    let mut outs = vec![];
    for confined in [true, false] {
        let s = served(&fx, None);
        let m = with_proxy(&s.srv);
        std::fs::write(
            s.job.path("in/manifest.json"),
            serde_json::to_vec(&m).unwrap(),
        )
        .unwrap();
        let entry = placed(&s.srv);
        let mut reads = sources(&fx);
        reads.extend(["proxy.mp4", "table.bin", "meta.json"].map(|f| entry.join(f)));
        let input = media_input(&s.srv, &s.job, &reads);
        let files = ["in/snapshot.json", "in/manifest.json", "out/"];
        let run = run(
            confined.then_some(&input),
            &s.job,
            "media.look",
            look.clone(),
            &files,
        );
        let res = run.result();
        assert_eq!(res["skipped"], json!([]), "{res}");
        // The proxy was read, not the original: no fallback was recorded.
        for f in res["frames"].as_array().unwrap() {
            assert!(f.get("proxyFallbacks").is_none(), "{f}");
        }
        outs.push((res.clone(), tree_bytes(&s.job.out())));
    }
    assert_eq!(outs[0], outs[1], "a proxy look: confined and unconfined");

    // `media.proxy` of the AV1 asset.
    let one_asset = RenderManifest {
        assets: vec![fx.manifest().asset(&fx.av1_id).unwrap().clone()],
        ..fx.manifest()
    };
    let tier = ProxyTier::Scrub;
    let p = ProxyParams {
        fingerprint: fp.clone(),
        tier,
        codec: ProxyCodec::Av1,
        decoder_class: VendorClass::Software,
        audio: ProxySourceAudio::None,
        key: proxy_key(
            &fp,
            tier,
            ProxyCodec::Av1,
            VendorClass::Software,
            ProxySourceAudio::None,
        )
        .to_string(),
        asset: Some(fx.av1_id.clone()),
        file: None,
        project_id: None,
        branch: None,
        display: None,
        intra_ok: false,
    };
    let av1 = vec![std::fs::canonicalize(&fx.av1).unwrap()];
    same_bytes_confined(
        &fx,
        Some(&one_asset),
        "media.proxy",
        serde_json::to_value(&p).unwrap(),
        |_| av1.clone(),
    );

    // `media.thumbs`: two thumbnails and a filmstrip.
    for mode in [
        ThumbsMode::Thumbs {
            times: vec![r("0"), r("1")],
            max_side: 256,
        },
        ThumbsMode::Strip {
            tile_height: 90,
            sprites: 1,
        },
    ] {
        let t = ThumbsParams {
            fingerprint: fp.clone(),
            asset: fx.av1_id.clone(),
            key: thumbs_key(&fp, &mode, VendorClass::Software).to_string(),
            mode,
            decoder_class: VendorClass::Software,
            only: None,
            project_id: None,
            branch: None,
            display: None,
        };
        same_bytes_confined(
            &fx,
            Some(&one_asset),
            "media.thumbs",
            serde_json::to_value(&t).unwrap(),
            |_| av1.clone(),
        );
    }
}

#[test]
fn capabilities_name_the_sandbox_in_force() {
    let test = "capabilities_name_the_sandbox_in_force";
    if !landlock(test) {
        return;
    }
    let tree = Tree::new();
    let caps = |input: Option<&WorkerInput>| {
        run(input, &tree.job, "media.capabilities", json!({}), &[]).result()["worker"]["sandbox"]
            .clone()
    };
    assert_eq!(caps(Some(&tree.input())), "landlock");
    assert_eq!(caps(None), "none");
}

/// F2 (7C's test hook reports Landlock disabled): the media worker applies nothing, says so
/// in `hello` and in `media.capabilities`, and its jobs run (unconfined, visibly so). The
/// server's `SANDBOX_DISABLED {profile: worker}` is `media-sandbox.test.ts`'s.
#[test]
fn landlock_disabled_runs_with_warning() {
    let tree = Tree::new();
    let input = tree.input();
    let disabled = [(TEST_ENV, "disabled".to_string())];
    let mut p = media(Some(&input), &disabled);
    p.send(&HostMsg::Start(tree.job.start(
        "media.capabilities",
        json!({}),
        &[],
    )));
    let run = p.drive(no_calls);
    assert_eq!(run.result()["worker"]["sandbox"], "none");
    let mut p = spawn_with(&["media"], &[request(&input), disabled[0].clone()]);
    assert_eq!(p.hello().self_sandbox.as_deref(), Some(NONE));
    // Unconfined indeed: a file outside the profile reads.
    let key = tree.p("srv/auth/local.key");
    p.send(&HostMsg::Start(tree.job.start(
        "media.testEscape",
        json!({ "attempts": [read(key.to_str().unwrap())] }),
        &[],
    )));
    assert_eq!(p.drive(no_calls).result()["errnos"], json!([null]));
    // And a real media job succeeds.
    if !gpu_or_skip("landlock_disabled_runs_with_warning") {
        return;
    }
    let fx = Fixture::new();
    let s = served(&fx, None);
    let input = media_input(&s.srv, &s.job, &sources(&fx));
    let mut p = media(Some(&input), &disabled);
    p.send(&HostMsg::Start(s.job.start(
        "media.look",
        serde_json::to_value(fx.params(&[r("0")])).unwrap(),
        &["in/snapshot.json", "in/manifest.json", "out/"],
    )));
    let run = p.drive(no_calls);
    assert_eq!(run.result()["skipped"], json!([]));
}

/// A host with Landlock enabled that cannot apply the profile fails the job: the worker exits
/// 78 before `hello` (which the job host turns into `SANDBOX_UNAVAILABLE`), having sent
/// nothing and run nothing.
#[test]
fn apply_failure_fails_the_job() {
    if !landlock("apply_failure_fails_the_job") {
        return;
    }
    let tree = Tree::new();
    // A source granted through a symbolic link: Landlock would grant its target, so the
    // profile refuses to apply.
    let link = tree.p("project/sources/link.mp4");
    std::os::unix::fs::symlink(tree.p("project/sources/a.mp4"), &link).unwrap();
    let mut input = tree.input();
    if let fin_sandbox::WorkerScope::Cold { sources, .. } = &mut input.scope {
        sources.push(link);
    }
    let mut env = worker_env();
    env.push(request(&input));
    let mut cmd = std::process::Command::new(support::BIN);
    cmd.arg("media").env_clear().env("FIN_TEST_MARKER", "1");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.stdin(std::process::Stdio::null()).output().unwrap();
    assert_eq!(out.status.code(), Some(EXIT_SELF_SANDBOX));
    assert!(out.stdout.is_empty(), "nothing is sent before the sandbox");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("is a symbolic link"), "{stderr}");
}
