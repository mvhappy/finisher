//! The four profile builders (spec §8.10, plan 7C Decisions 6–9 and Task 1).

use std::path::PathBuf;

use fin_core::events::payloads::agent::{AgentMode, Harness};
use fin_sandbox::profile::{agent, hook, plugin, worker};
use fin_sandbox::{
    AgentInput, DeviceAccess, DeviceRule, FinEndpoint, HookFin, HookInput, Net, PluginData,
    PluginInput, PolicyError, ReadPolicy, SandboxPolicy, WorkerInput, WorkerScope,
};

fn abs(rel: &str) -> PathBuf {
    let mut p = PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" });
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(part);
    }
    p
}

fn reads(p: &SandboxPolicy) -> &[PathBuf] {
    match &p.read {
        ReadPolicy::Only(r) => r,
        #[allow(unreachable_patterns)]
        _ => unreachable!("profiles read by allow-list only"),
    }
}

fn input_error(r: Result<SandboxPolicy, PolicyError>, field: &str) {
    match r {
        Err(PolicyError::Input { field: f, .. })
        | Err(PolicyError::NotAbsolute { field: f, .. }) => {
            assert!(f.starts_with(field), "{f} is not {field}")
        }
        other => panic!("expected an input error on {field}, got {other:?}"),
    }
}

fn session(harness: Harness, mode: AgentMode) -> AgentInput {
    AgentInput {
        session_id: "s1".into(),
        mode,
        harness,
        project_dir: abs("projects/p"),
        workspace_dir: abs("projects/p/workspace/s1"),
        shared_dir: Some(abs("projects/p/workspace/shared")),
        library_roots: vec![abs("media/lib")],
        session_dir: abs("data/agents/s1"),
        sign_in_store: Some(abs("data/providers/codex/home/auth.json")),
        toolchains: vec![abs("usr/bin"), abs("usr/lib")],
        server_data: abs("data"),
        home: abs("home/u"),
        fin: None,
    }
}

#[test]
fn agent_plan_has_no_shared_write() {
    let plan = agent(&session(Harness::Codex, AgentMode::Plan)).unwrap();
    assert!(!plan.write.contains(&abs("projects/p/workspace/shared")));
    assert!(!plan.writable(&abs("projects/p/workspace/shared/notes.md")));
    assert!(plan.readable(&abs("projects/p/workspace/shared/notes.md")));
    assert!(plan.writable(&abs("projects/p/workspace/s1/notes.md")));
    for mode in [AgentMode::Draft, AgentMode::Auto] {
        let p = agent(&session(Harness::Codex, mode)).unwrap();
        assert!(
            p.writable(&abs("projects/p/workspace/shared/notes.md")),
            "{mode:?}"
        );
    }
}

#[test]
fn agent_reads_only_allow_list() {
    let p = agent(&session(Harness::Codex, AgentMode::Draft)).unwrap();
    let mut expected: Vec<PathBuf> = [
        "usr/bin",
        "usr/lib",
        "projects/p",
        "media/lib",
        "data/agents/s1/credential.json",
        "data/agents/s1/settings",
        "data/agents/s1/tmp",
        "data/agents/s1/provider",
        "data/agents/s1/git",
        "data/providers/codex/home/auth.json",
    ]
    .iter()
    .map(|r| abs(r))
    .collect();
    expected.sort();
    assert_eq!(reads(&p), expected.as_slice());
    let mut writes: Vec<PathBuf> = [
        "projects/p/workspace/s1",
        "projects/p/workspace/shared",
        "data/agents/s1/tmp",
        "data/agents/s1/provider",
        "data/providers/codex/home/auth.json",
    ]
    .iter()
    .map(|r| abs(r))
    .collect();
    writes.sort();
    assert_eq!(p.write, writes);
    assert!(matches!(p.net, Net::Open { ref loopback } if loopback.is_empty()));

    for no in [
        "",
        "home/u",
        "home/u/notes.txt",
        "home/u/.ssh/id_ed25519",
        "home/u/.claude/settings.json",
        "home/u/.codex/config.toml",
        "data/auth/local.key",
        "data/auth/console.key",
        "data/auth/credential.master",
        "data/settings.json",
        "data/control.db",
        "data/journal/seg-1",
        "data/agents/s2/credential.json",
        "data/agents/s1/blobs/x",
        "data/providers/codex/home/config.toml",
        "data/providers/claudeCode/home/.credentials.json",
        "projects/other/sources/a.mov",
    ] {
        assert!(!p.readable(&abs(no)), "{no} is readable");
    }
    for yes in [
        "data/agents/s1/credential.json",
        "data/agents/s1/git/config",
        "data/providers/codex/home/auth.json",
        "usr/bin/git",
    ] {
        assert!(p.readable(&abs(yes)), "{yes} is not readable");
    }
    // The second layer holds a widened toolchain entry too.
    for d in ["data", "home/u/.ssh", "home/u/.claude", "home/u/.codex"] {
        assert!(p.deny_read.contains(&abs(d)), "{d} is not denied");
    }
}

#[test]
fn agent_reads_same_project_workspaces_only() {
    let p = agent(&session(Harness::Codex, AgentMode::Draft)).unwrap();
    // Owner decision 2026-11-08: another session's workspace of the same project is readable.
    assert!(p.readable(&abs("projects/p/workspace/s2/draft.md")));
    assert!(!p.writable(&abs("projects/p/workspace/s2/draft.md")));
    assert!(p.writable(&abs("projects/p/workspace/s1/draft.md")));
    assert!(!p.readable(&abs("projects/q/workspace/s1/draft.md")));
    assert!(!p.writable(&abs("projects/q/workspace/s1/draft.md")));
}

#[test]
fn agent_claude_denies_claude_md() {
    let mut i = session(Harness::ClaudeCode, AgentMode::Draft);
    i.sign_in_store = Some(abs("data/providers/claudeCode/home/.credentials.json"));
    let claude = agent(&i).unwrap();
    for f in [
        "CLAUDE.md",
        "CLAUDE.local.md",
        ".claude/settings.json",
        ".claude/hooks/x.sh",
    ] {
        assert!(!claude.readable(&abs(&format!("projects/p/{f}"))), "{f}");
    }
    assert!(claude.readable(&abs("projects/p/sources/a.mov")));
    let codex = agent(&session(Harness::Codex, AgentMode::Draft)).unwrap();
    assert!(codex.readable(&abs("projects/p/CLAUDE.md")));
    // The store of one harness is not the other's.
    input_error(
        agent(&AgentInput {
            harness: Harness::Codex,
            ..i
        }),
        "signInStore",
    );
}

#[test]
fn agent_protects_project_state() {
    let p = agent(&session(Harness::Codex, AgentMode::Auto)).unwrap();
    for f in [
        "projects/p/.finisher",
        "projects/p/assets",
        "projects/p/finisher.project.json",
        "projects/p/finisher.rules.json",
        "data/agents/s1/credential.json",
        "data/agents/s1/settings",
    ] {
        assert!(p.protect.contains(&abs(f)), "{f} is not protected");
        assert!(!p.writable(&abs(f)), "{f} is writable");
        assert!(!p.writable(&abs(&format!("{f}/x"))), "{f}/x is writable");
    }
}

#[test]
fn agent_inputs_are_checked() {
    let s = || session(Harness::Codex, AgentMode::Draft);
    input_error(
        agent(&AgentInput {
            harness: Harness::Builtin,
            ..s()
        }),
        "harness",
    );
    input_error(
        agent(&AgentInput {
            harness: Harness::Plugin,
            ..s()
        }),
        "harness",
    );
    for id in ["", "..", "a/b", "a\\b", "shared"] {
        input_error(
            agent(&AgentInput {
                session_id: id.into(),
                ..s()
            }),
            "sessionId",
        );
    }
    input_error(
        agent(&AgentInput {
            workspace_dir: abs("projects/p/workspace/s2"),
            ..s()
        }),
        "workspaceDir",
    );
    input_error(
        agent(&AgentInput {
            shared_dir: Some(abs("projects/p/shared")),
            ..s()
        }),
        "sharedDir",
    );
    input_error(
        agent(&AgentInput {
            session_dir: abs("data/agents/s2"),
            ..s()
        }),
        "sessionDir",
    );
    input_error(
        agent(&AgentInput {
            sign_in_store: Some(abs("data/providers/codex/home")),
            ..s()
        }),
        "signInStore",
    );
    input_error(
        agent(&AgentInput {
            toolchains: vec![abs("data/run")],
            ..s()
        }),
        "toolchains",
    );
    input_error(
        agent(&AgentInput {
            toolchains: vec![abs("home/u/.ssh/bin")],
            ..s()
        }),
        "toolchains",
    );
    input_error(
        agent(&AgentInput {
            library_roots: vec![abs("data/library")],
            ..s()
        }),
        "libraryRoots",
    );
    let in_data = AgentInput {
        project_dir: abs("data/p"),
        workspace_dir: abs("data/p/workspace/s1"),
        shared_dir: None,
        ..s()
    };
    input_error(agent(&in_data), "projectDir");
    input_error(
        agent(&AgentInput {
            toolchains: vec![PathBuf::from("usr/bin")],
            ..s()
        }),
        "toolchains",
    );
}

#[test]
fn agent_reaches_fin_by_its_endpoint_only() {
    let tcp = agent(&AgentInput {
        fin: Some(FinEndpoint::Tcp { port: 41234 }),
        ..session(Harness::Codex, AgentMode::Draft)
    })
    .unwrap();
    assert_eq!(
        tcp.net,
        Net::Open {
            loopback: vec![41234]
        }
    );
    assert!(tcp.unix_sockets.is_empty());
    let sock = abs("data/run/fin.sock");
    let unix = agent(&AgentInput {
        fin: Some(FinEndpoint::UnixSocket { path: sock.clone() }),
        ..session(Harness::Codex, AgentMode::Draft)
    })
    .unwrap();
    assert_eq!(unix.unix_sockets, vec![sock.clone()]);
    assert!(
        !unix.readable(&sock),
        "connecting to the socket is not a read grant"
    );
}

fn cold() -> WorkerInput {
    WorkerInput {
        tmp: abs("data/jobs/j1/tmp"),
        caches: vec![abs("data/cache/fonts")],
        devices: vec![],
        server_data: abs("data"),
        scope: WorkerScope::Cold {
            job_dir: abs("data/jobs/j1"),
            sources: vec![abs("projects/p/sources/a.mov")],
        },
    }
}

#[test]
fn worker_cold_vs_warm() {
    let c = worker(&cold()).unwrap();
    assert!(c.readable(&abs("projects/p/sources/a.mov")));
    assert!(
        !c.readable(&abs("projects/p/sources/b.mov")),
        "a source not in the manifest"
    );
    assert!(c.readable(&abs("data/jobs/j1/in/x")) && !c.writable(&abs("data/jobs/j1/in/x")));
    assert!(c.writable(&abs("data/jobs/j1/out/x")) && c.writable(&abs("data/jobs/j1/tmp/x")));
    assert!(
        !c.writable(&abs("data/jobs/j1/x")),
        "the job directory itself"
    );
    assert!(!c.readable(&abs("data/jobs/j2/in/x")), "a sibling job");
    assert!(
        c.readable(&abs("data/cache/fonts/f.ttf")) && !c.writable(&abs("data/cache/fonts/f.ttf"))
    );

    let warm = |dir: &str| WorkerInput {
        tmp: abs(&format!("{dir}/tmp")),
        scope: WorkerScope::Warm {
            worker_dir: abs(dir),
            library_roots: vec![abs("projects/p/sources"), abs("media/lib")],
        },
        ..cold()
    };
    let w = worker(&warm("data/jobs/w-look-1")).unwrap();
    assert!(w.writable(&abs("data/jobs/w-look-1/jobs/j9/out/x")));
    assert!(
        !w.readable(&abs("data/jobs/j1/in/x")),
        "a job outside the worker's directory"
    );
    assert!(
        !w.readable(&abs("data/jobs/w-look-2/jobs/j3/in/x")),
        "another warm worker"
    );
    assert!(
        w.readable(&abs("projects/p/sources/b.mov"))
            && !w.writable(&abs("projects/p/sources/b.mov"))
    );
    assert!(
        !w.readable(&abs("projects/q/sources/b.mov")),
        "another project"
    );
    // Never the jobs root, nor anything above it.
    input_error(worker(&warm("data/jobs")), "scope.workerDir");
    input_error(worker(&warm("data")), "scope.workerDir");
    input_error(
        worker(&WorkerInput {
            scope: WorkerScope::Cold {
                job_dir: abs("data/jobs"),
                sources: vec![],
            },
            ..cold()
        }),
        "scope.jobDir",
    );
    input_error(
        worker(&WorkerInput {
            scope: WorkerScope::Cold {
                job_dir: abs("data/jobs/j1"),
                sources: vec![abs("data/auth/local.key")],
            },
            ..cold()
        }),
        "scope.sources",
    );
}

#[test]
fn worker_net_none_processes_one() {
    let render = DeviceRule {
        path: abs("dev/dri/renderD128"),
        access: DeviceAccess::ReadWrite,
    };
    let p = worker(&WorkerInput {
        devices: vec![render.clone()],
        ..cold()
    })
    .unwrap();
    assert_eq!(p.net, Net::None);
    assert!(p.unix_sockets.is_empty());
    assert_eq!(p.limits.processes, Some(1));
    assert_eq!(p.devices, vec![render]);
    assert!(!p.readable(&abs("data/auth/local.key")) && !p.readable(&abs("data/control.db")));
}

fn plugin_input() -> PluginInput {
    PluginInput {
        store_dir: abs("data/plugins/store/acme.denoise/1.0.0"),
        runtime: vec![abs("opt/finisher/node")],
        system: vec![abs("usr/lib"), abs("lib")],
        onnx_runtime: Some(abs("opt/finisher/lib/libonnxruntime.so")),
        models: vec![
            abs("data/plugins/store/acme.denoise/1.0.0/models/m.onnx"),
            abs("data/models/shared.onnx"),
        ],
        call_dir: abs("data/jobs/j1"),
        tmp: abs("data/jobs/j1/tmp"),
        plugin_data: Some(PluginData {
            dir: abs("data/plugins/data/acme.denoise/proj-1"),
            write: true,
        }),
        gpu: vec![],
        memory_bytes: None,
        server_data: abs("data"),
        home: abs("home/u"),
    }
}

#[test]
fn plugin_matches_sp8_16_1() {
    let p = plugin(&plugin_input()).unwrap();
    // (path, readable, writable): every row of sp8 §16.1's files and network lists.
    let rows: &[(&str, bool, bool)] = &[
        (
            "data/plugins/store/acme.denoise/1.0.0/index.js",
            true,
            false,
        ),
        (
            "data/plugins/store/acme.denoise/1.0.0/models/m.onnx",
            true,
            false,
        ),
        (
            "data/plugins/store/other.plugin/2.0.0/index.js",
            false,
            false,
        ),
        ("opt/finisher/node/bin/node", true, false),
        ("usr/lib/libc.so.6", true, false),
        ("lib/ld-linux.so", true, false),
        ("opt/finisher/lib/libonnxruntime.so", true, false),
        ("data/models/shared.onnx", true, false),
        ("data/jobs/j1/in/frame.raw", true, false),
        ("data/jobs/j1/out/mask.raw", true, true),
        ("data/jobs/j1/tmp/scratch", true, true),
        (
            "data/plugins/data/acme.denoise/proj-1/state.json",
            true,
            true,
        ),
        (
            "data/plugins/data/acme.denoise/proj-2/state.json",
            false,
            false,
        ),
        (
            "data/plugins/data/other.plugin/proj-1/state.json",
            false,
            false,
        ),
        ("data/auth/local.key", false, false),
        ("data/settings.json", false, false),
        ("data/control.db", false, false),
        ("data/jobs/j2/in/frame.raw", false, false),
        ("projects/p/sources/a.mov", false, false),
        ("home/u/.ssh/id_ed25519", false, false),
        ("home/u/Documents/letter.txt", false, false),
    ];
    for (path, r, w) in rows {
        assert_eq!(
            (p.readable(&abs(path)), p.writable(&abs(path))),
            (*r, *w),
            "{path}"
        );
    }
    assert_eq!(p.protect, vec![abs("data/plugins/store")]);
    assert_eq!(p.net, Net::None);
    assert!(p.unix_sockets.is_empty() && p.mach.is_empty() && p.devices.is_empty());
    assert_eq!(
        (p.limits.processes, p.limits.memory, p.limits.open_files),
        (Some(8), Some(2 << 30), Some(1024))
    );

    // Read-only plugin data; GPU only as granted and never a display node; the memory cap.
    let ro = plugin(&PluginInput {
        plugin_data: Some(PluginData {
            dir: abs("data/plugins/data/acme.denoise/proj-1"),
            write: false,
        }),
        ..plugin_input()
    })
    .unwrap();
    assert!(
        ro.readable(&abs("data/plugins/data/acme.denoise/proj-1/x"))
            && !ro.writable(&abs("data/plugins/data/acme.denoise/proj-1/x"))
    );
    let render = DeviceRule {
        path: abs("dev/dri/renderD128"),
        access: DeviceAccess::ReadWrite,
    };
    assert_eq!(
        plugin(&PluginInput {
            gpu: vec![render.clone()],
            ..plugin_input()
        })
        .unwrap()
        .devices,
        vec![render]
    );
    let card = DeviceRule {
        path: abs("dev/dri/card0"),
        access: DeviceAccess::Read,
    };
    input_error(
        plugin(&PluginInput {
            gpu: vec![card],
            ..plugin_input()
        }),
        "gpu",
    );
    assert_eq!(
        plugin(&PluginInput {
            memory_bytes: Some(16 << 30),
            ..plugin_input()
        })
        .unwrap()
        .limits
        .memory,
        Some(16 << 30)
    );
    input_error(
        plugin(&PluginInput {
            memory_bytes: Some((16 << 30) + 1),
            ..plugin_input()
        }),
        "memoryBytes",
    );
    // The store is read-only to every grant; no grant in a personal folder.
    let in_store = PluginData {
        dir: abs("data/plugins/store/acme.denoise/data"),
        write: true,
    };
    assert!(matches!(
        plugin(&PluginInput {
            plugin_data: Some(in_store),
            ..plugin_input()
        }),
        Err(PolicyError::WriteUnderProtect { .. })
    ));
    input_error(
        plugin(&PluginInput {
            store_dir: abs("data/plugins/other"),
            ..plugin_input()
        }),
        "storeDir",
    );
    input_error(
        plugin(&PluginInput {
            models: vec![abs("home/u/Documents/m.onnx")],
            ..plugin_input()
        }),
        "models",
    );
    input_error(
        plugin(&PluginInput {
            runtime: vec![abs("data/node")],
            ..plugin_input()
        }),
        "runtime",
    );
}

fn hook_input() -> HookInput {
    HookInput {
        project_dir: abs("projects/p"),
        hook_file: abs("projects/p/.finisher/hooks/check.sh"),
        toolchains: vec![abs("usr/bin")],
        tmp: abs("data/hooks/h1/tmp"),
        server_data: abs("data"),
        home: abs("home/u"),
        fin: None,
    }
}

#[test]
fn hook_net_none_unless_declared() {
    let credential = abs("data/hooks/h1/credential.json");
    let none = hook(&hook_input()).unwrap();
    assert_eq!(none.net, Net::None);
    assert!(none.unix_sockets.is_empty());
    assert!(
        !none.readable(&credential),
        "no credential unless fin is declared"
    );
    assert!(
        none.readable(&abs("projects/p/sources/a.mov"))
            && !none.writable(&abs("projects/p/sources/a.mov"))
    );
    assert!(none.readable(&abs("projects/p/.finisher/hooks/check.sh")));
    assert!(none.writable(&abs("data/hooks/h1/tmp/x")));
    assert!(!none.readable(&abs("data/auth/local.key")) && !none.readable(&abs("home/u/.ssh/id")));

    let with = |endpoint| {
        hook(&HookInput {
            fin: Some(HookFin {
                credential: credential.clone(),
                endpoint,
            }),
            ..hook_input()
        })
        .unwrap()
    };
    let tcp = with(FinEndpoint::Tcp { port: 41234 });
    assert_eq!(tcp.net, Net::Loopback { ports: vec![41234] });
    assert!(tcp.readable(&credential) && !tcp.writable(&credential));
    let sock = abs("data/run/fin.sock");
    let unix = with(FinEndpoint::UnixSocket { path: sock.clone() });
    assert_eq!((unix.net, unix.unix_sockets), (Net::None, vec![sock]));
    let pipe = with(FinEndpoint::Pipe);
    assert_eq!(pipe.net, Net::None);
    input_error(
        hook(&HookInput {
            tmp: abs("home/u/.ssh/tmp"),
            ..hook_input()
        }),
        "tmp",
    );
}

#[test]
fn inputs_schema_is_generated() {
    let mut g = schemars::SchemaGenerator::default();
    fin_sandbox::register_schemas(&mut g);
    let defs = g.definitions();
    for name in [
        "AgentInput",
        "HookInput",
        "HookFin",
        "WorkerInput",
        "WorkerScope",
        "PluginInput",
        "PluginData",
        "FinEndpoint",
        "DeviceRule",
        "SandboxProfile",
    ] {
        assert!(defs.contains_key(name), "{name} is not registered");
    }
    // camelCase on the wire, unknown fields refused.
    let json = serde_json::json!({
        "sessionId": "s1", "mode": "draft", "harness": "codex",
        "projectDir": abs("projects/p"), "workspaceDir": abs("projects/p/workspace/s1"),
        "sessionDir": abs("data/agents/s1"), "serverData": abs("data"), "home": abs("home/u"),
        "fin": {"kind": "tcp", "port": 41234}
    });
    let i: AgentInput = serde_json::from_value(json.clone()).unwrap();
    assert!(agent(&i).is_ok());
    let mut extra = json;
    extra["readEverything"] = serde_json::json!(true);
    assert!(serde_json::from_value::<AgentInput>(extra).is_err());
    let w: WorkerInput = serde_json::from_value(serde_json::json!({
        "tmp": abs("data/jobs/j1/tmp"), "serverData": abs("data"),
        "scope": {"kind": "cold", "jobDir": abs("data/jobs/j1")}
    }))
    .unwrap();
    assert!(worker(&w).is_ok());
}
