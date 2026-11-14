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

/// What is wrong with `own` (the harness list, with its file's `text`) against the base list and
/// the recorded start: empty when it is minimal. An entry must **be** a recorded denied read (a
/// `<dir>/*` record is its directory, the narrowest root that names every file below it), so an
/// ancestor of one (`/usr/share` for `/usr/share/ca-certificates`) is too broad; every recorded
/// read must be given or tolerated with a reason.
fn minimality(own: &[&str], text: &str, base: &[&str], fx: &Start) -> Vec<String> {
    let mut bad = vec![];
    if own.is_empty() {
        bad.push("the list is empty".to_owned());
    }
    for e in own {
        if !e.starts_with('/') || e.ends_with('/') {
            bad.push(format!("{e}: not an absolute path"));
        }
        // Not what the base list already gives.
        if base.iter().any(|b| within(e, b)) {
            bad.push(format!("{e} is already in toolchain-base.txt"));
        }
        // Needed by a recorded start, and no broader than that need.
        if !fx.denied_reads.iter().any(|r| recorded(r) == *e) {
            match fx.denied_reads.iter().find(|r| within(recorded(r), e)) {
                Some(r) => bad.push(format!("{e} is broader than the recorded read {r}")),
                None => bad.push(format!("{e} covers no read Task 0 recorded")),
            }
        }
        // Names its evidence.
        let evidence = text
            .lines()
            .find(|l| l.split('#').next().unwrap().trim() == *e)
            .and_then(|l| l.split_once('#'))
            .is_some_and(|(_, c)| c.contains("Task 0"));
        if !evidence {
            bad.push(format!("{e}: no comment naming Task 0"));
        }
    }
    // Every recorded read is given (base or own) or is named, with its reason, as not given.
    for r in &fx.denied_reads {
        let given = base.iter().chain(own).any(|g| within(recorded(r), g));
        match fx.tolerated.get(r) {
            Some(why) if why.is_empty() || given => {
                bad.push(format!("{r}: tolerated yet given or unexplained"));
            }
            Some(_) => {}
            None if !given => bad.push(format!("{r}: a recorded read nobody gives or tolerates")),
            None => {}
        }
    }
    for r in fx.tolerated.keys() {
        if !fx.denied_reads.contains(r) {
            bad.push(format!("{r}: tolerated but not recorded"));
        }
    }
    bad
}

#[test]
fn toolchain_list_is_minimal() {
    let own = toolchain_harness(Os::Linux, Harness::ClaudeCode);
    let bad = minimality(&own, LINUX_TEXT, &toolchain_base(Os::Linux), &start());
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn a_broader_root_than_the_recorded_read_is_rejected() {
    // The shipped list with each entry swapped, in memory, for an ancestor of it: every one of
    // them still covers the recorded read, and every one is refused as broader than it.
    let fx = start();
    let base = toolchain_base(Os::Linux);
    let own = toolchain_harness(Os::Linux, Harness::ClaudeCode);
    for (narrow, broad) in [
        ("/usr/share/ca-certificates", "/usr/share"),
        ("/proc/sys/vm", "/proc/sys"),
        ("/proc/stat", "/proc"),
        ("/sys/devices/system/cpu/online", "/sys/devices/system/cpu"),
        ("/sys/kernel/mm/transparent_hugepage", "/sys"),
    ] {
        assert!(own.contains(&narrow), "{narrow} is not in the shipped list");
        let mutated: Vec<&str> = own
            .iter()
            .map(|e| if *e == narrow { broad } else { e })
            .collect();
        let text = LINUX_TEXT.replace(&format!("{narrow}  #"), &format!("{broad}  #"));
        let bad = minimality(&mutated, &text, &base, &fx);
        // (`/proc` also gives the tolerated `/proc/self/*`, refused on its own line.)
        let want = format!("{broad} is broader than the recorded read ");
        assert!(
            bad.iter().any(|b| b.starts_with(&want)),
            "{broad}: {bad:#?}"
        );
        assert!(
            bad.iter()
                .all(|b| b.starts_with(&want) || b.contains("tolerated yet given")),
            "{broad}: {bad:#?}"
        );
    }
    // An entry no recorded read needs, and one already in the base list, are refused too.
    let mut extra = own.clone();
    extra.push("/opt/elsewhere");
    let text = format!("{LINUX_TEXT}\n/opt/elsewhere  # (Task 0)\n");
    let bad = minimality(&extra, &text, &base, &fx);
    assert_eq!(
        bad,
        vec!["/opt/elsewhere covers no read Task 0 recorded".to_owned()]
    );
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

// The install-layout half of `toolchain_list_is_minimal`: a **layout fixture** (Task 0's recorded
// layout, rebuilt in a temp dir), not drift detection against a real install, which this suite
// may not depend on (the host's install is the person's). Drift of a real install is 7D Task 18's
// recorded start on each OS; the TypeScript side (`installTreeRead`) is tested in
// `apps/finisherd/test/agent-claude-install.test.ts`.
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
