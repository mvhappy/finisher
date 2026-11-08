use std::collections::BTreeSet;

use clap::CommandFactory;

use super::*;

fn tree() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let project = root.join("film");
    let workspace = project.join("workspace").join("s1");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("words.json"), "[]").unwrap();
    std::fs::write(project.join("clip.mp4"), "x").unwrap();
    let outside = root.join("server-data").join("auth");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("local.key"), "secret").unwrap();
    (tmp, project, workspace, outside)
}

#[test]
fn inside_the_workspace_or_the_project_is_allowed() {
    let (_t, project, workspace, _) = tree();
    for p in [
        workspace.join("words.json"),
        project.join("clip.mp4"),
        workspace.join("not-yet.json"),
        workspace.join("sub/../words.json"),
    ] {
        let r = confine(&p, &workspace, &project).unwrap();
        assert!(r.starts_with(&project), "{}", r.display());
    }
}

#[test]
fn outside_dotdot_and_absolute_are_refused() {
    let (_t, project, workspace, outside) = tree();
    for p in [
        outside.join("local.key"),
        project.join("../server-data/auth/local.key"),
        workspace.join("../../../server-data/auth/local.key"),
        PathBuf::from("/etc/hosts"),
    ] {
        let e = confine(&p, &workspace, &project).unwrap_err();
        assert_eq!(e.code, "PATH_NOT_ALLOWED", "{}", p.display());
        assert_eq!(e.details["reason"], "outside");
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_workspace_is_refused() {
    let (_t, project, workspace, outside) = tree();
    std::os::unix::fs::symlink(outside.join("local.key"), workspace.join("key.json")).unwrap();
    std::os::unix::fs::symlink(&outside, workspace.join("dir")).unwrap();
    for p in [workspace.join("key.json"), workspace.join("dir/local.key")] {
        let e = confine(&p, &workspace, &project).unwrap_err();
        assert_eq!(e.code, "PATH_NOT_ALLOWED", "{}", p.display());
    }
}

#[test]
fn case_variants_fold_where_the_volume_does() {
    let (_t, project, workspace, _) = tree();
    let upper = PathBuf::from(workspace.display().to_string().to_uppercase()).join("w.json");
    // Folded (Windows, macOS): the same folder. Not folded (Linux): another one.
    assert!(confine_with(&upper, &[&workspace, &project], true).is_ok());
    let e = confine_with(&upper, &[&workspace, &project], false).unwrap_err();
    assert_eq!(e.code, "PATH_NOT_ALLOWED");
}

/// Arguments called `path` that are parameter paths (`text.size`), not files.
const PARAMETER_PATHS: &[&str] = &[
    "keyframe.remove",
    "keyframe.set",
    "marker.set",
    "param.set",
    "style.set",
    "timeline.set",
    "track.set",
];

/// The path arguments are exactly the arguments whose value names a file, a folder or a
/// path (`--project` excepted: it must name the session's project, `agent_session`; and the
/// parameter paths of [`PARAMETER_PATHS`]).
#[test]
fn every_path_argument_is_marked() {
    let cli = crate::configure(crate::Cli::command());
    let mut found = BTreeSet::new();
    for c in crate::registry::commands() {
        let mut cmd = Some(&cli);
        for word in c.name.split('.') {
            cmd = cmd.and_then(|x| x.find_subcommand(word));
        }
        let Some(cmd) = cmd else { continue };
        for arg in cmd.get_arguments() {
            if arg.get_id() == "project"
                || (arg.get_id() == "path" && PARAMETER_PATHS.contains(&c.name))
            {
                continue;
            }
            let names = arg.get_value_names().unwrap_or_default();
            let pathy = names.iter().any(|n| {
                let n = n.as_str().to_ascii_lowercase();
                n.contains("path") || n.contains("dir") || n.contains("file")
            });
            if pathy {
                found.insert((c.name.to_string(), arg.get_id().to_string()));
            }
        }
    }
    let marked: BTreeSet<(String, String)> = PATH_ARGS
        .iter()
        .map(|(c, a)| ((*c).to_string(), (*a).to_string()))
        .collect();
    assert_eq!(found, marked, "PATH_ARGS drifted from the command line");
}
