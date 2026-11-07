use super::*;

fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |k| {
        pairs
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| (*v).to_string())
    }
}

#[test]
fn the_env_opt_out_follows_the_server_s_rules() {
    assert!(!env_opt_out(env(&[])));
    assert!(env_opt_out(env(&[("DO_NOT_TRACK", "1")])));
    assert!(env_opt_out(env(&[("DO_NOT_TRACK", "yes")])));
    for off in ["", "0", "false", " FALSE "] {
        assert!(!env_opt_out(env(&[("DO_NOT_TRACK", off)])), "{off:?}");
    }
    assert!(env_opt_out(env(&[("FIN_DIAGNOSTICS", "off")])));
    assert!(env_opt_out(env(&[("FIN_DIAGNOSTICS", " OFF")])));
    assert!(!env_opt_out(env(&[("FIN_DIAGNOSTICS", "on")])));
}

#[test]
fn the_actor_is_an_agent_only_with_a_session() {
    let get = |h: &[[String; 2]], k: &str| h.iter().find(|p| p[0] == k).map(|p| p[1].clone());
    let user = actor_headers_from(None, Some("call-1"));
    assert_eq!(get(&user, "fin-actor-kind").as_deref(), Some("user"));
    assert_eq!(user.len(), 1);
    let agent = actor_headers_from(Some("s1"), Some("c1"));
    assert_eq!(get(&agent, "fin-actor-kind").as_deref(), Some("agent"));
    assert_eq!(get(&agent, "fin-actor-session").as_deref(), Some("s1"));
    assert_eq!(get(&agent, "fin-actor-call").as_deref(), Some("c1"));
    let no_call = actor_headers_from(Some("s1"), None);
    assert_eq!(no_call.len(), 2);
    // A bad id (empty, over-long) is dropped; the run is still an agent's.
    let long = "x".repeat(MAX_RAW_ID + 1);
    assert_eq!(actor_headers_from(Some("s1"), Some(&long)).len(), 2);
    for bad in ["", long.as_str()] {
        let h = actor_headers_from(Some(bad), Some("c"));
        assert_eq!(get(&h, "fin-actor-kind").as_deref(), Some("agent"));
        assert_eq!(get(&h, "fin-actor-session"), None);
        assert_eq!(get(&h, "fin-actor-call").as_deref(), Some("c"));
    }
    assert_eq!(actor_headers_from(Some(""), None).len(), 1);
}

#[test]
fn the_discovery_file_gates_only_when_it_says_so() {
    note_discovery(&json!({ "diagnostics": { "enabled": false } }));
    assert_eq!(DISCOVERY.load(Ordering::Relaxed), DISABLED);
    note_discovery(&json!({ "diagnostics": { "enabled": true } }));
    assert_eq!(DISCOVERY.load(Ordering::Relaxed), ENABLED);
    // An older server wrote no `diagnostics`: send, and let the server's gate decide.
    note_discovery(&json!({ "socket": "s" }));
    assert_eq!(DISCOVERY.load(Ordering::Relaxed), UNKNOWN);
}

#[test]
fn a_command_event_holds_names_and_hashes_only() {
    let s = State {
        name: Some("item.trim".into()),
        flags: ["project".to_string(), "typed-by-a-user".to_string()].into(),
        parse_hash: Some(format!("blake3:{}", "ab".repeat(32))),
        tx_ids: vec!["tx-1".into()],
        ..State::default()
    };
    let err = CoreError::new("CONFLICT", "no").with_details(json!({ "txId": "tx-2" }));
    let e = command_event(&s, &Err(err), 4, 1234, true);
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(v["name"], "item.trim");
    assert_eq!(v["flags"], json!(["project", "other"]));
    assert_eq!(v["exit"], 4);
    assert_eq!(v["code"], "CONFLICT");
    assert_eq!(v["txIds"], json!(["tx-1", "tx-2"]));
    let unknown = State {
        name: Some("not.a.command".into()),
        ..State::default()
    };
    let e = command_event(&unknown, &Ok(json!({})), 0, 1, false);
    assert_eq!(e.name.as_str(), "other");
    assert!(e.args_hash.is_none());
}

fn hash_of(line: &str) -> String {
    let mut cmd = crate::configure(<crate::Cli as clap::CommandFactory>::command());
    let args = std::iter::once("fin").chain(line.split_whitespace());
    let m = cmd.try_get_matches_from_mut(args).unwrap();
    describe(&cmd, &m).2
}

#[test]
fn the_parse_hash_is_canonical() {
    let base = hash_of("marker add --at 1s --label x");
    assert!(Blake3Hex::parse(&base).is_some());
    assert_eq!(base, hash_of("marker add --label x --at 1s"), "flag order");
    assert_eq!(hash_of("--dry-run status"), hash_of("status --dry-run"));
    assert_ne!(base, hash_of("marker add --at 1s --label y"), "a value");
    assert_ne!(base, hash_of("marker add --at 2s --label x"));
    assert_ne!(hash_of("status"), hash_of("status --dry-run"));
    assert_ne!(
        hash_of("branch rename a b"),
        hash_of("branch rename b a"),
        "positionals in order"
    );
    assert_ne!(hash_of("status"), hash_of("view"), "the command");
}

#[test]
fn a_write_hashes_its_input_the_parse_hash_is_the_fallback() {
    let input = json!({ "cmd": "marker.add", "args": { "at": "1s" } });
    let s = State {
        inputs: vec![input.clone()],
        parse_hash: Some(hash_of("marker add --at 1s --label x")),
        ..State::default()
    };
    assert_eq!(args_hash(&s), crate::commands::args_hash(&[input]));
    let read_only = State {
        parse_hash: Some(hash_of("status")),
        ..State::default()
    };
    assert_eq!(args_hash(&read_only), hash_of("status"));
}

fn parsed(line: &str) -> (String, Vec<String>) {
    let mut cmd = crate::configure(<crate::Cli as clap::CommandFactory>::command());
    let args = std::iter::once("fin").chain(line.split_whitespace());
    let m = cmd.try_get_matches_from_mut(args).unwrap();
    let (name, flags, _) = describe(&cmd, &m);
    (name, flags.into_iter().collect())
}

#[test]
fn registry_names_are_in_core_tables() {
    for c in crate::registry::commands() {
        assert!(
            CLI_COMMANDS.get(c.name).is_some(),
            "{} is not in CLI_COMMANDS",
            c.name
        );
    }
    fn walk(c: &Command, out: &mut BTreeSet<String>) {
        out.extend(
            c.get_arguments()
                .filter_map(|a| a.get_long())
                .map(str::to_string),
        );
        c.get_subcommands().for_each(|s| walk(s, out));
    }
    let mut cmd = crate::configure(<crate::Cli as clap::CommandFactory>::command());
    cmd.build();
    let mut longs = BTreeSet::new();
    walk(&cmd, &mut longs);
    for long in longs {
        assert!(
            CLI_FLAGS.get(&long).is_some(),
            "--{long} is not in CLI_FLAGS"
        );
    }
    // Every name the parse yields for a sample of the tree is in the table.
    for line in [
        "status",
        "try keep",
        "cherry-pick tx1 --into b",
        "caption set-text c1 Hello",
    ] {
        let (name, _) = parsed(line);
        assert!(CLI_COMMANDS.get(&name).is_some(), "{line}: {name}");
    }
}

#[test]
fn a_parse_yields_a_name_and_flag_names_never_values() {
    let (name, flags) = parsed("clip insert --track t1 --at end --asset a1 --dur 2s --dry-run");
    assert_eq!(name, "clip.insert");
    assert_eq!(flags, ["asset", "at", "dry-run", "dur", "track"]);
    assert_eq!(
        parsed("--project d --idem-key k status"),
        ("status".into(), vec!["idem-key".into(), "project".into()])
    );
    assert_eq!(parsed("try keep").0, "try.keep");
    assert_eq!(parsed("schema param --scope style").0, "schema.param");
    assert_eq!(parsed("schema clip.insert").0, "schema");
    assert_eq!(parsed("").0, "help");
    assert_eq!(parsed("help clip").0, "help");
    let (name, flags) =
        parsed("diagnostics export --since 2026-11-01 --kind cli.command --as-upload training");
    assert_eq!(name, "diagnostics.export");
    assert_eq!(flags, ["as-upload", "kind", "since"]);
}
