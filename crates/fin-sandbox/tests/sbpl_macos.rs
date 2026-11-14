//! The Seatbelt generator (plan 7C Task 8), pure, on every Unix: each profile's output against
//! committed goldens (`tests/golden/macos/*.sb`; `FIN_UPDATE_GOLDEN=1` rewrites them), hostile
//! path names, and the rules against the reference model of `SandboxPolicy::{readable,
//! writable}` over random policies (last match wins, as Seatbelt evaluates them).
#![cfg(unix)]

use std::path::{Path, PathBuf};

use fin_core::events::payloads::agent::{AgentMode, Harness};
use fin_sandbox::macos::sbpl::{Filter, Rule, SYSTEM_READ, Use, render, rules};
use fin_sandbox::profile::for_os;
use fin_sandbox::{
    AgentInput, FinEndpoint, HookFin, HookInput, Limits, Net, Os, PluginData, PluginInput, Profile,
    ReadPolicy, SandboxPolicy, WorkerInput, WorkerScope,
};
use proptest::prelude::*;

fn id(p: &Path, _: Use) -> Result<PathBuf, fin_sandbox::ApplyError> {
    Ok(p.to_path_buf())
}

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

fn agent_input() -> AgentInput {
    AgentInput {
        session_id: "s1".into(),
        mode: AgentMode::Draft,
        harness: Harness::ClaudeCode,
        project_dir: p("/u/proj"),
        workspace_dir: p("/u/proj/workspace/s1"),
        shared_dir: Some(p("/u/proj/workspace/shared")),
        library_roots: vec![p("/u/lib")],
        session_dir: p("/u/data/agents/s1"),
        sign_in_home: None,
        sign_in_store: Some(p("/u/data/providers/claudeCode/home/.credentials.json")),
        toolchains: vec![p("/usr"), p("/bin"), p("/private/etc")],
        server_data: p("/u/data"),
        home: p("/u/home"),
        fin: Some(FinEndpoint::UnixSocket {
            path: p("/u/data/run/fin.sock"),
        }),
    }
}

fn policies() -> Vec<(&'static str, SandboxPolicy)> {
    let os = Os::Macos;
    let hook = HookInput {
        project_dir: Some(p("/u/proj")),
        hook_file: p("/u/proj/.hooks/check.sh"),
        toolchains: vec![p("/usr"), p("/bin")],
        tmp: p("/u/data/agents/s1/tmp/hook"),
        server_data: p("/u/data"),
        home: p("/u/home"),
        fin: Some(HookFin {
            credential: p("/u/data/agents/s1/hook-credential.json"),
            endpoint: FinEndpoint::UnixSocket {
                path: p("/u/data/run/fin.sock"),
            },
        }),
        timeout_ms: None,
    };
    let worker = WorkerInput {
        tmp: p("/u/data/jobs/j1/tmp"),
        caches: vec![p("/u/data/fonts")],
        devices: vec![],
        server_data: p("/u/data"),
        scope: WorkerScope::Cold {
            job_dir: p("/u/data/jobs/j1"),
            sources: vec![p("/u/proj/sources/a.mov")],
        },
    };
    let plugin = PluginInput {
        store_dir: p("/u/data/plugins/store/acme.x/1.0.0"),
        runtime: vec![p("/u/app/node")],
        system: vec![p("/usr/lib"), p("/System/Library")],
        onnx_runtime: None,
        models: vec![p("/u/data/plugins/models/m.onnx")],
        call_dir: p("/u/data/plugins/calls/c1"),
        tmp: p("/u/data/plugins/calls/c1/tmp"),
        plugin_data: Some(PluginData {
            dir: p("/u/proj/.finisher/plugin-data/acme.x"),
            write: true,
        }),
        gpu: vec![],
        memory_bytes: None,
        server_data: p("/u/data"),
        home: p("/u/home"),
    };
    vec![
        ("agent", for_os::agent(os, &agent_input()).unwrap()),
        ("hook", for_os::hook(os, &hook).unwrap()),
        ("worker", for_os::worker(os, &worker).unwrap()),
        ("plugin", for_os::plugin(os, &plugin).unwrap()),
    ]
}

#[test]
fn each_profile_matches_its_golden() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/macos");
    let update = std::env::var("FIN_UPDATE_GOLDEN").as_deref() == Ok("1");
    for (name, policy) in policies() {
        let got = render(&policy, &id).unwrap().golden();
        let file = dir.join(format!("{name}.sb"));
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&file, &got).unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("{}: {e} (FIN_UPDATE_GOLDEN=1 writes it)", file.display()));
        assert_eq!(got, want, "{name}: the generated profile changed");
    }
}

#[test]
fn every_profile_denies_by_default_and_names_no_path_in_its_text() {
    for (name, policy) in policies() {
        let sbpl = render(&policy, &id).unwrap();
        assert!(sbpl.text.contains("(deny default)"), "{name}");
        assert!(!sbpl.text.contains("/u/"), "{name}: a path in the text");
        assert!(!sbpl.text.contains("(allow mach-lookup)\n"), "{name}");
        // Apple Events, TCC and the pasteboard are refused by `(deny default)` alone.
        assert!(!sbpl.text.contains("appleevent"), "{name}");
        let forks = sbpl.text.contains("(allow process-fork)");
        assert_eq!(forks, policy.profile != Profile::Worker, "{name}");
    }
}

#[test]
fn hostile_path_names_are_parameters_and_newlines_are_refused() {
    let mut i = agent_input();
    let odd = "/u/lib/a \"quoted\" \\back\\ (paren) ;semi";
    i.library_roots = vec![p(odd)];
    let policy = for_os::agent(Os::Macos, &i).unwrap();
    let sbpl = render(&policy, &id).unwrap();
    assert!(
        sbpl.params.iter().any(|(_, v)| v == odd),
        "{:?}",
        sbpl.params
    );
    assert!(!sbpl.text.contains("quoted"));
    assert!(sbpl.defines().iter().any(|d| d.ends_with(odd)));
    for bad in ["/u/lib/a\nb", "/u/lib/a\rb"] {
        let mut i = agent_input();
        i.library_roots = vec![p(bad)];
        let policy = for_os::agent(Os::Macos, &i).unwrap();
        let err = render(&policy, &id).unwrap_err();
        assert!(err.to_string().contains("NUL, CR or LF"), "{err}");
    }
}

#[test]
fn the_agent_reaches_only_fins_socket_and_the_resolver() {
    let (_, policy) = policies().remove(0);
    let r = rules(&policy, &id).unwrap();
    let sockets: Vec<&PathBuf> = r
        .iter()
        .filter(|r| r.allow)
        .flat_map(|r| &r.filters)
        .filter_map(|f| match f {
            Filter::UnixSocket(p) => Some(p),
            _ => None,
        })
        .collect();
    assert_eq!(
        sockets,
        vec![
            &p("/u/data/run/fin.sock"),
            &p("/private/var/run/mDNSResponder")
        ]
    );
    // Open egress unfiltered, loopback and every other Unix socket refused by a filtered deny
    // (which wins over the unfiltered allow), no loopback port reopened (no listener on macOS).
    let text = render(&policy, &id).unwrap().text;
    assert!(text.contains("(allow network-outbound)\n"));
    assert!(text.contains(
        "(deny network-outbound\n    (remote ip \"localhost:*\")\n    (remote unix-socket))"
    ));
    assert!(!text.contains("(remote ip \"*:*\")"));
    assert!(!text.contains("(allow network-outbound (remote ip \"localhost:"));
}

/// Seatbelt's evaluation: the last rule whose operation and filter match decides; none denies.
fn eval(rules: &[Rule], op: &str, path: &Path) -> bool {
    let op_matches = |o: &str| match o.strip_suffix('*') {
        Some(prefix) => op.starts_with(prefix),
        None => o == op,
    };
    let filter_matches = |f: &Filter| match f {
        Filter::Subpath(r) => path.starts_with(r),
        Filter::Literal(r) => path == r,
        _ => false,
    };
    rules
        .iter()
        .rev()
        .find(|r| {
            r.ops.iter().any(|o| op_matches(o))
                && (r.filters.is_empty() || r.filters.iter().any(filter_matches))
        })
        .is_some_and(|r| r.allow)
}

const NAMES: [&str; 3] = ["a", "b", "c"];

fn candidates() -> Vec<PathBuf> {
    let mut out = vec![p("/t")];
    let mut layer = vec![p("/t")];
    for _ in 0..3 {
        layer = layer
            .iter()
            .flat_map(|d| NAMES.iter().map(move |n| d.join(n)))
            .collect();
        out.extend(layer.iter().cloned());
    }
    out
}

fn pick(c: &[PathBuf], ix: &[usize]) -> Vec<PathBuf> {
    ix.iter().map(|&i| c[i % c.len()].clone()).collect()
}

fn idx() -> impl Strategy<Value = Vec<usize>> {
    prop::collection::vec(0usize..40, 0..4)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn the_rules_are_the_reference_model(
        write in idx(), read in idx(), protect in idx(), deny in idx(), worker in any::<bool>(),
    ) {
        let c = candidates();
        let (protect, deny) = (pick(&c, &protect), pick(&c, &deny));
        let write: Vec<_> = pick(&c, &write).into_iter().filter(|w| !protect.iter().any(|p| w.starts_with(p)) && !deny.contains(w)).collect();
        let read: Vec<_> = pick(&c, &read).into_iter().filter(|r| !deny.contains(r)).collect();
        let policy = SandboxPolicy {
            profile: if worker { Profile::Worker } else { Profile::Hook },
            write,
            read: ReadPolicy::Only(read),
            protect,
            deny_read: deny,
            unix_sockets: vec![],
            net: Net::None,
            mach: vec![],
            devices: vec![],
            limits: Limits { processes: worker.then_some(1), ..Limits::default() },
        };
        policy.validate().unwrap();
        let r = rules(&policy, &id).unwrap();
        for path in &c {
            prop_assert_eq!(eval(&r, "file-read-data", path), policy.readable(path), "read {:?}", path);
            prop_assert_eq!(eval(&r, "file-write-data", path), policy.writable(path), "write {:?}", path);
            // Removing or replacing an entry: the entry is writable as a whole and, for a write
            // root itself, so is its directory (as Landlock, which grants nothing on the parent).
            let root = policy.write.contains(path);
            let parent_ok = path.parent().is_some_and(|d| policy.writable(d));
            let whole = policy.writable(path) && (!root || parent_ok);
            for op in ["file-write-unlink", "file-write-create"] {
                prop_assert_eq!(eval(&r, op, path), whole, "{} {:?}", op, path);
            }
            // Metadata never needs more than the ancestors of a grant.
            if eval(&r, "file-read-metadata", path) {
                let granted = policy.write.iter().chain(policy.read_roots().iter()).any(|g| g.starts_with(path) || path.starts_with(g));
                prop_assert!(granted, "metadata of {:?}", path);
            }
        }
        // The Seatbelt base adds only system code outside the policy's tree.
        for s in SYSTEM_READ {
            prop_assert!(eval(&r, "file-read-data", Path::new(s)));
            prop_assert!(!eval(&r, "file-write-data", Path::new(s)));
        }
    }
}
