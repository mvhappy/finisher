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
    // Folded (Windows, macOS): the same folder. Not folded (Linux): another one — unless the
    // volume itself is case-insensitive, where resolving the links gives back the stored case
    // (macOS `realpath`), so the two spellings are one path either way.
    assert!(confine_with(&upper, &[&workspace, &project], true).is_ok());
    let not_folded = confine_with(&upper, &[&workspace, &project], false);
    if std::fs::canonicalize(upper.parent().unwrap()).is_err() {
        assert_eq!(not_folded.unwrap_err().code, "PATH_NOT_ALLOWED");
    } else {
        assert!(not_folded.is_ok());
    }
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

/// Review B1: a folder with no readable `finisher.project.json` is not the session's project.
#[test]
fn a_folder_without_a_project_file_is_not_the_sessions() {
    let (_t, project, workspace, _) = tree();
    let state = crate::agent_session::SelfState {
        project_id: "p1".into(),
        root_branch: None,
        default_branch: "b".into(),
        tries: vec![],
    };
    for dir in [&project, &workspace, Path::new("/")] {
        let e = crate::agent_session::check_folder(Some(dir), &state).unwrap_err();
        assert_eq!(e.code, "PROJECT_NOT_FOUND", "{}", dir.display());
    }
    std::fs::write(project.join("finisher.project.json"), r#"{"id":"p1"}"#).unwrap();
    crate::agent_session::check_folder(Some(&project), &state).unwrap();
    std::fs::write(project.join("finisher.project.json"), r#"{"id":"p2"}"#).unwrap();
    assert!(crate::agent_session::check_folder(Some(&project), &state).is_err());
}

/// Review B1/H3: the index the `fin` tool parses argv with gives every flag's value count, the
/// global ones too (they may come between the command words).
#[test]
fn the_index_gives_flag_arity_and_the_globals() {
    let idx = crate::commands::schema::index(&crate::configure(crate::Cli::command()));
    let flag = |list: &serde_json::Value, long: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|f| f["long"] == long)
            .cloned()
            .unwrap_or_else(|| panic!("no {long} in {list}"))
    };
    assert_eq!(flag(&idx["globals"], "--project")["values"], 1);
    assert_eq!(flag(&idx["globals"], "--branch")["values"], 1);
    assert_eq!(flag(&idx["globals"], "--dry-run")["values"], 0);
    let cmd = |name: &str| {
        idx["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .cloned()
            .unwrap()
    };
    let import = cmd("caption.import");
    assert_eq!(flag(&import["flags"], "--words")["values"], 1);
    assert_eq!(flag(&import["flags"], "--project")["values"], 1);
    assert_eq!(cmd("asset.add")["paths"][0]["position"], 0);
    assert_eq!(
        flag(&cmd("transition.set")["flags"], "--between")["values"],
        2
    );
}
