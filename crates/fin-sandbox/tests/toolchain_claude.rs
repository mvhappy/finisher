//! Claude Code's own toolchain lists (plan 7D Task 14): `data/<os>/toolchain-claudeCode.txt`
//! held to what Task 0 recorded (`tests/fixtures/claude-code-start.json`) and to the install
//! tree's layout. Pure: runs on every OS.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use fin_core::events::payloads::agent::{AgentMode, Harness};
use fin_sandbox::data::{toolchain_base, toolchain_harness};
use fin_sandbox::{AgentInput, Os};
use serde::Deserialize;

const LINUX_TEXT: &str = include_str!("../data/linux/toolchain-claudeCode.txt");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Start {
    denied_reads: Vec<String>,
    tolerated: BTreeMap<String, String>,
    install: Install,
}

#[derive(Deserialize)]
struct Install {
    link: String,
    target: String,
}

fn start() -> Start {
    serde_json::from_str(include_str!("fixtures/claude-code-start.json")).unwrap()
}

/// `path` is `root` or below it (whole components).
fn within(path: &str, root: &str) -> bool {
    Path::new(path).starts_with(root)
}

/// A recorded read as a path: `<dir>/*` (the names were not kept) is the directory.
fn recorded(read: &str) -> &str {
    read.strip_suffix("/*").unwrap_or(read)
}

#[test]
fn toolchain_list_is_minimal() {
    let fx = start();
    let own = toolchain_harness(Os::Linux, Harness::ClaudeCode);
    let base = toolchain_base(Os::Linux);
    assert!(!own.is_empty());
    for e in &own {
        assert!(
            e.starts_with('/') && !e.ends_with('/'),
            "{e}: an absolute path"
        );
        // Not what the base list already gives.
        assert!(
            !base.iter().any(|b| within(e, b)),
            "{e} is already in toolchain-base.txt"
        );
        // Needed by a recorded start: it covers at least one read that start was refused.
        assert!(
            fx.denied_reads.iter().any(|r| within(recorded(r), e)),
            "{e} covers no read Task 0 recorded"
        );
        // Names its evidence.
        let line = LINUX_TEXT
            .lines()
            .find(|l| l.split('#').next().unwrap().trim() == *e)
            .unwrap();
        assert!(
            line.split_once('#')
                .is_some_and(|(_, c)| c.contains("Task 0")),
            "{e}: no comment naming Task 0"
        );
    }
    // Every recorded read is given (base or own) or is named, with its reason, as not given.
    for r in &fx.denied_reads {
        let given = base.iter().chain(&own).any(|g| within(recorded(r), g));
        match fx.tolerated.get(r) {
            Some(why) => {
                assert!(
                    !why.is_empty() && !given,
                    "{r}: tolerated yet given or unexplained"
                );
            }
            None => assert!(given, "{r}: a recorded read nobody gives or tolerates"),
        }
    }
    for r in fx.tolerated.keys() {
        assert!(
            fx.denied_reads.contains(r),
            "{r}: tolerated but not recorded"
        );
    }
}

#[test]
fn only_claude_code_has_a_list_and_other_oses_claim_nothing_unproven() {
    for os in [Os::Linux, Os::Macos, Os::Windows] {
        for h in [
            Harness::Codex,
            Harness::Fake,
            Harness::Builtin,
            Harness::Plugin,
        ] {
            assert!(toolchain_harness(os, h).is_empty(), "{os:?} {h:?}");
        }
    }
    // Task 0 ran on Linux: macOS and Windows list nothing until Task 18 records a start there.
    for (os, text) in [
        (
            Os::Macos,
            include_str!("../data/macos/toolchain-claudeCode.txt"),
        ),
        (
            Os::Windows,
            include_str!("../data/windows/toolchain-claudeCode.txt"),
        ),
    ] {
        assert!(
            toolchain_harness(os, Harness::ClaudeCode).is_empty(),
            "{os:?}"
        );
        assert!(text.contains("TODO(7D Task 18)"), "{os:?}");
    }
}

#[test]
fn the_list_never_names_the_install_tree() {
    // The install moves with the person's install: one file under `<home>/.local/share/claude/
    // versions/` and the link to it. The adapter grants its directory as `extraRead`.
    let fx = start();
    for e in toolchain_harness(Os::Linux, Harness::ClaudeCode) {
        for tree in [&fx.install.link, &fx.install.target] {
            let tree = tree.trim_start_matches("<home>");
            assert!(!e.contains(tree.trim_matches('/')), "{e} names the install");
        }
        assert!(!e.contains(".local") && !e.contains("/home/"), "{e}");
    }
}

/// The fixture's install tree made under `home`: the file and the link to it.
#[cfg(unix)]
fn install(home: &Path, fx: &Install) -> (PathBuf, PathBuf) {
    let place = |s: &str| PathBuf::from(s.replace("<home>", &home.to_string_lossy()));
    let (link, target) = (place(&fx.link), place(&fx.target));
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::fs::write(&target, "ELF").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    (link, target)
}

#[cfg(unix)]
fn input(root: &Path, toolchains: Vec<PathBuf>) -> AgentInput {
    let (project, data) = (root.join("p"), root.join("data"));
    AgentInput {
        session_id: "s1".into(),
        mode: AgentMode::Draft,
        harness: Harness::ClaudeCode,
        workspace_dir: project.join("workspace/s1"),
        shared_dir: Some(project.join("workspace/shared")),
        project_dir: project,
        library_roots: vec![],
        session_dir: data.join("agents/s1"),
        sign_in_home: None,
        sign_in_store: Some(data.join("providers/claudeCode/home/store")),
        toolchains,
        server_data: data,
        home: root.join("home"),
        fin: None,
    }
}

#[cfg(unix)]
#[test]
fn install_directory_comes_from_the_resolved_binary_not_the_list() {
    use fin_sandbox::profile::for_os;
    let fx = start();
    let d = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(d.path()).unwrap();
    let (link, target) = install(&root.join("home"), &fx.install);
    // The directory of the link is `~/.local/bin`; the binary is the link's target's directory.
    let real = std::fs::canonicalize(&link).unwrap();
    assert_eq!(real, target);
    assert_ne!(real.parent(), link.parent());
    // Without the adapter's `extraRead` the install is out of reach, whatever the list holds ...
    let policy = for_os::agent(Os::Linux, &input(&root, vec![])).unwrap();
    assert!(!policy.readable(&real));
    assert!(!policy.readable(&link));
    // ... and with the resolved binary's directory the binary is readable, a sibling version too.
    let dir = real.parent().unwrap().to_path_buf();
    let policy = for_os::agent(Os::Linux, &input(&root, vec![dir.clone()])).unwrap();
    assert!(policy.readable(&real));
    assert!(policy.readable(&dir.join("2.1.322")));
    // The link's own directory is not granted by that (`~/.local/bin` holds other tools).
    assert!(!policy.readable(&link));
}
