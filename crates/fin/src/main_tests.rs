//! Unit tests of `main.rs`: argument parsing and the registry's samples.

use super::*;

fn args(s: &str) -> Vec<OsString> {
    std::iter::once("fin")
        .chain(s.split_whitespace())
        .map(OsString::from)
        .collect()
}

#[test]
fn every_registry_command_parses() {
    let samples = [
        "project create d --name n",
        "project open d --relocate",
        "project status",
        "project takeover --confirm",
        "project verify",
        "project rebuild",
        "project compact",
        "project recover --create all --import d --dismiss abc123abc123",
        "project acknowledge --rewind 2 --branch main",
        "status",
        "look --at 1s,f30 --size 960x540 --out looks --no-wait",
        "view --track t1 --range 1s.. --budget 200",
        "branch new b --from main@3",
        "branch ls --all",
        "branch rename a b",
        "branch archive b --restore --as c",
        "branch default --clear",
        "checkpoint cp",
        "undo --to cp",
        "redo",
        "merge b --into main --skip-conflicting",
        "cherry-pick tx1 --into b",
        "drop tx1 tx2 --from b --into main",
        "review request --title t --into main --summary s",
        "review ls --state open --kind branchReview",
        "review show r1",
        "review refresh r1",
        "review comment r1 hello --tx tx1 --at 1s",
        "review decide r1 --accept tx1,tx2 --reject tx3 --reason why",
        "review merge r1 --accept-pending",
        "review close r1 --dismissed",
        "revert m1",
        "try t",
        "try keep",
        "try drop",
        "asset add f.mp4",
        "clip insert --track t1 --at end --asset a1 --dur 1s",
        "clip trim c1 --end -1s",
        "clip delete c1 --mode lift",
        "image insert --track t1 --at end --asset a2 --dur 1s",
        "graphic insert --track t1 --at 0 --template a3 --slot title=Hi --slot tint=#fff",
        "item move c1 --to after:c2 --track t3 --mode overwrite",
        "item split c1 --at 2s",
        "item delete c1 --mode lift",
        "item trim c1 --start -1s --mode roll",
        "item slip c1 -0.5s",
        "item slide c1 +f3",
        "item link c1 c2 c3",
        "item unlink lk1",
        "transition set --between c1 c2 --dur 1s --kind dissolve --curve equalPower --no-linked",
        "transition remove tr1",
        "track add --kind audio --index 0 --name Music",
        "track remove t3 --with-items",
        "track move t3 --index 1",
        "track set t1 gain -6dB",
        "timeline add --name Alt --rate 25 --resolution 1920x1080",
        "timeline set tl1 name Main",
        "timeline remove tl2",
        "marker add --at 2s --label drop --dur 1s --color #f00 --timeline tl2",
        "marker set m1 label --reset",
        "marker remove m1",
        "bin add --name Footage --parent b1",
        "bin move b2 --parent b1",
        "bin remove b2",
        "asset move a1 --bin b1",
        "asset remove a1",
        "asset reprobe a1 --deep",
        "param set c1 opacity 50%",
        "keyframe set c1 opacity --at 1s 0.5 --interp easeOut",
        "keyframe remove c1 opacity --at 1s",
        "effect add c1 blur --at 0 --set radius=4",
        "effect remove c1 fx1",
        "effect move c1 fx1 --to 0",
        "caption import --track t3 --srt s.srt --style preset:bold --at 2s",
        "caption set-text cap1 Hello",
        "caption split cap1 --word 2",
        "caption merge cap1 cap2",
        "caption style cap1 st1 --set text.size=72px",
        "style add --name Big --from preset:bold --set text.size=96px",
        "style set st1 color #fff",
        "style remove st1",
        "style ls",
        "eval frame --at 2s",
        "eval audio --range 0..2s",
        "media caps",
        "cache status",
        "cache gc --to 1.5",
        "media probe a1",
        "media proxy a1 --tier 1080 --codec av1 --no-wait",
        "media status a1",
        "media thumbs a1 --at 0,1.5s --max-side 128 --no-wait",
        "media pcm a1 --recipe pcm16m --no-wait",
        "fonts ls --cached --detail",
        "fonts fetch noto-sans",
        "fonts status",
        "fonts pin Inter --weight 700 --italic --fallback 0",
        "fonts repin Inter|700|normal --catalog-version 2026.11.1",
        "fonts unpin Inter|700|normal",
        "diagnostics show",
        "diagnostics export --since 2026-11-01 --project p1 --kind cli.command --limit 9 --as-upload training",
        "diagnostics purge --before 2026-11-01",
        "diagnostics project never",
        "diagnostics consent training off",
        "track ls --offset 2 --limit 3",
        "asset ls --bin b1 --offset 2 --limit 3",
        "bin ls --offset 2 --limit 3",
        "effect ls c1 --offset 2 --limit 3",
        "log --before 9 --limit 5",
        "plugin search fonts --kind content --registry team",
        "plugin info @acme/fonts",
        "plugin ls",
        "plugin suggest @acme/fonts --reason why",
        "plugin install @acme/fonts@1.0.0 --registry team",
        "plugin update @acme/fonts",
        "plugin rm @acme/fonts --purge-data",
        "plugin enable @acme/fonts",
        "plugin disable @acme/fonts",
        "plugin commands @acme/tools",
        "registry ls",
        "registry status team",
        "registry refresh team",
        "registry add team --dir d --root-sha256 ab",
        "registry rm team",
        "registry forget --root ab",
        "job ls --state failed --kind media.look --mine --limit 5",
        "job show 01job",
        "job cancel 01job",
        "job pause --all",
        "job resume 01job",
        "wait 01job 01job2 --any --timeout 5",
        "library ls",
        "library status raw",
        "library scan raw --full --dir clips",
        "library files raw clips --state present --kind video --limit 5 --cursor c1",
        "library show raw clips/a.mov",
        "library search interview --lib raw --min-duration 90s --since 2026-11-01",
        "library set raw --name Footage --include *.mov --poll 300 --pause",
        "search interview --kind video --limit 5",
        "approval wait 01appr --timeout 5",
        "approval ls --status open",
        "agent spawn --role fixer --brief fix --mode draft --budget 2usd --model m",
        "agent wait 01s 01s2 --any --timeout 5",
        "agent ls",
        "agent send 01s hello",
        "agent stop 01s",
        "agent rollback 01s --to-run 1",
        "agent resume 01s go on",
        "agent interrupt 01s",
        "batch",
        "help clip",
        "whoami",
        "schema clip.insert",
        "schema param --scope style --path text.",
    ];
    let commands: Vec<_> = registry::commands().collect();
    assert_eq!(samples.len(), commands.len());
    for (sample, command) in samples.iter().zip(commands) {
        assert!(
            sample.starts_with(&command.name.replace('.', " ")),
            "{sample} vs {}",
            command.name
        );
        // The name `FIN_CALL`'s default keys on is the registry's (`schema param`, a
        // read, is `schema` with an argument).
        let (_, name) = parse_named(args(sample)).unwrap_or_else(|e| panic!("{sample}: {e:?}"));
        let want = command.name.strip_suffix(".param").unwrap_or(command.name);
        assert_eq!(name.as_deref(), Some(want), "{sample}");
    }
}

/// Decision 7 from argv: the 4B command lines, parsed by this `fin` (new flags absent, no
/// `--timeline`), hash to the values the 4B fixture pinned.
#[test]
fn args_hash_of_4b_inputs_is_unchanged_from_argv() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fin-store/tests/fixtures/4b/expected.json");
    let expected: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let pinned = expected["argsHashes"].as_array().unwrap();
    let lines = [
        "clip insert --track t1 --at end --asset a1 --dur 2s",
        "clip trim c3 --start +0.5s",
        "clip delete c2",
    ];
    assert_eq!(pinned.len(), lines.len());
    for (line, case) in lines.iter().zip(pinned) {
        let step = match parse(args(line)).unwrap().command {
            Some(Cmd::Clip(ClipCmd::Insert(a))) => Write::ClipInsert(a),
            Some(Cmd::Clip(ClipCmd::Trim(a))) => Write::ClipTrim(a),
            Some(Cmd::Clip(ClipCmd::Delete(a))) => Write::ClipDelete(a),
            other => panic!("{line}: {other:?}"),
        };
        let input = commands::input(&step);
        assert_eq!(
            input,
            json!({ "cmd": case["cmd"], "args": case["args"] }),
            "{line}"
        );
        assert_eq!(json!(commands::args_hash(&[input])), case["hash"], "{line}");
    }
}

#[test]
fn positional_branch_names_are_not_the_global_flag() {
    for (s, branch) in [
        ("merge side", None),
        ("branch archive side", None),
        ("--branch x merge side", Some("x")),
        ("merge side --branch x", Some("x")),
    ] {
        let cli = parse(args(s)).unwrap();
        assert_eq!(cli.globals.branch.as_deref(), branch, "{s}");
    }
}

#[test]
fn try_takes_a_name_or_keep_or_drop() {
    for (s, name, sub) in [
        ("try", None, false),
        ("try scratch", Some("scratch"), false),
        ("try keep", None, true),
        ("try drop", None, true),
    ] {
        match parse(args(s)).unwrap().command {
            Some(Cmd::Try(a)) => {
                assert_eq!((a.name.as_deref(), a.sub.is_some()), (name, sub), "{s}")
            }
            other => panic!("{s}: {other:?}"),
        }
    }
    assert_eq!(parse(args("try a keep")).unwrap_err().code, "USAGE");
}

#[test]
fn colour_is_never_used() {
    assert_eq!(
        configure(Cli::command()).get_color(),
        clap::ColorChoice::Never
    );
}

#[test]
fn help_flags_are_usage_errors() {
    for s in ["--help", "clip insert --help", "-h", "--version", "clip"] {
        assert_eq!(parse(args(s)).unwrap_err().code, "USAGE", "{s}");
    }
}
