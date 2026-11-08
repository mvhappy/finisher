use std::path::Path;

use super::*;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn arr(v: &[u8]) -> [u8; 32] {
    v.try_into().unwrap()
}

/// Writes `text` as a private (0600 on Unix) key or credential file.
fn write_private(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn vectors() -> Value {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/auth/handshake.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn handshake_vectors_match_rust() {
    let v = vectors();
    assert_eq!(v["labels"]["hello"], LABEL_HELLO);
    assert_eq!(v["labels"]["prove"], LABEL_PROVE);
    let cases = v["handshakes"].as_array().unwrap();
    assert!(cases.len() >= 5);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let key = decode_key(v["keys"][case["key"].as_str().unwrap()].as_str().unwrap()).unwrap();
        let (env, endpoint) = (
            case["envId"].as_str().unwrap(),
            case["endpoint"].as_str().unwrap(),
        );
        let cn = arr(&hex(case["clientNonce"].as_str().unwrap()));
        let sn = arr(&hex(case["serverNonce"].as_str().unwrap()));
        assert_eq!(
            mac_input(LABEL_HELLO, [env, endpoint], [&cn, &sn]),
            hex(case["helloInput"].as_str().unwrap()),
            "{name}: hello input"
        );
        assert_eq!(
            mac_input(LABEL_PROVE, [env, endpoint], [&sn, &cn]),
            hex(case["proveInput"].as_str().unwrap()),
            "{name}: prove input"
        );
        assert_eq!(
            hello_proof(&key, env, endpoint, &cn, &sn).to_vec(),
            hex(case["proof"].as_str().unwrap()),
            "{name}: proof"
        );
        assert_eq!(
            prove_mac(&key, env, endpoint, &sn, &cn).to_vec(),
            hex(case["mac"].as_str().unwrap()),
            "{name}: mac"
        );
        // The constant-time check accepts the proof and refuses a flipped bit.
        let mut proof = hex(case["proof"].as_str().unwrap());
        assert!(
            hello_mac(&key, env, endpoint, &cn, &sn)
                .verify_slice(&proof)
                .is_ok()
        );
        proof[0] ^= 1;
        assert!(
            hello_mac(&key, env, endpoint, &cn, &sn)
                .verify_slice(&proof)
                .is_err()
        );
    }
}

#[test]
fn key_files_decode_strictly() {
    let key = "RkFLRS1sb2NhbC1rZXktZm9yLXRlc3QtdmVjdG9ycyE";
    assert!(decode_key(key).is_some());
    assert!(decode_key(&format!("{key}\n")).is_some());
    assert!(decode_key(&format!("{key}=")).is_none());
    assert!(decode_key(&key[1..]).is_none());
    assert!(decode_key("").is_none());
    // A non-canonical last character (unused bits set) is refused, as the server refuses it.
    assert!(decode_key("RkFLRS1sb2NhbC1rZXktZm9yLXRlc3QtdmVjdG9ycyF").is_none());
}

#[test]
fn credential_files_need_every_field() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("c.json");
    let key = "RkFLRS1sb2NhbC1rZXktZm9yLXRlc3QtdmVjdG9ycyE";
    write_private(
        &file,
        &format!(
            r#"{{"v":1,"kind":"mcp","id":"01mcp00000000000000000000a","key":"{key}","env":"e"}}"#
        ),
    );
    let c = read_credential_file(&file).unwrap().unwrap();
    assert_eq!(
        (c.kind, c.id.as_deref(), c.env.as_deref()),
        (
            CredentialKind::Mcp,
            Some("01mcp00000000000000000000a"),
            Some("e")
        )
    );
    assert!(!format!("{c:?}").contains(key), "Debug never shows the key");
    for bad in [
        r#"{"v":2,"kind":"mcp","id":"x","key":"K"}"#,
        r#"{"v":1,"kind":"local","id":"x","key":"K"}"#,
        r#"{"v":1,"kind":"agent","key":"K"}"#,
        r#"{"v":1,"kind":"agent","id":"x","key":"short"}"#,
        "not json",
    ] {
        write_private(&file, &bad.replace('K', key));
        assert!(read_credential_file(&file).unwrap().is_none(), "{bad}");
    }
}

#[test]
fn local_key_is_found_in_server_data() {
    let dir = tempfile::tempdir().unwrap();
    let set = std::env::var_os(CREDENTIAL_ENV).is_some();
    assert!(set || discover(Some(dir.path())).unwrap().is_none());
    std::fs::create_dir_all(dir.path().join("auth")).unwrap();
    write_private(
        &dir.path().join("auth/local.key"),
        "RkFLRS1sb2NhbC1rZXktZm9yLXRlc3QtdmVjdG9ycyE\n",
    );
    if !set {
        let c = discover(Some(dir.path())).unwrap().unwrap();
        assert_eq!((c.kind, c.id, c.env), (CredentialKind::Local, None, None));
    }
}

#[test]
fn canonical_endpoint_resolves_symlinks_and_lowercases_pipes() {
    assert_eq!(
        canonical_endpoint(&Endpoint::Pipe(r"\\.\pipe\Finisher-Ann-0A1B".into())),
        r"\\.\pipe\finisher-ann-0a1b"
    );
    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("fin.sock"), b"").unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("link")).unwrap();
        let via_link = canonical_endpoint(&Endpoint::Socket(dir.path().join("link/fin.sock")));
        assert_eq!(
            via_link,
            std::fs::canonicalize(real.join("fin.sock"))
                .unwrap()
                .display()
                .to_string()
        );
    }
}

#[test]
fn errors_name_no_secret_and_no_command() {
    let e = untrusted(&Endpoint::Socket("/sd/run/fin.sock".into()), "badProof");
    assert_eq!(e.code, "SERVER_UNTRUSTED");
    assert_eq!(
        e.details,
        json!({ "endpoint": "/sd/run/fin.sock", "reason": "badProof" })
    );
    let n = no_credential(&Endpoint::Socket("/sd/run/fin.sock".into()));
    assert_eq!(
        (n.code.as_str(), &n.details["reason"]),
        ("UNAUTHENTICATED", &json!("noCredential"))
    );
    let fix = n.fix.unwrap();
    assert!(
        fix.contains("FIN_CREDENTIAL") && fix.contains("server's data"),
        "{fix}"
    );
    let old = untrusted(
        &Endpoint::Socket("/sd/run/fin.sock".into()),
        "versionMismatch",
    );
    assert_eq!(old.details["reason"], "versionMismatch");
    assert!(
        old.message.contains("older Finisher release"),
        "{}",
        old.message
    );
    let unsafe_file = unsafe_credential(Path::new("/sd/c.json"), "readableByOthers", 0o100644);
    assert_eq!(unsafe_file.details["mode"], "644");
    for f in [
        e.fix.unwrap(),
        fix,
        old.fix.unwrap(),
        unsafe_file.fix.unwrap(),
    ] {
        assert!(
            !f.contains('`') && !f.contains("fin ") && !f.contains("finisherd "),
            "{f}"
        );
    }
}

#[cfg(unix)]
#[test]
fn credential_files_must_be_private() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("c.json");
    let key = "RkFLRS1sb2NhbC1rZXktZm9yLXRlc3QtdmVjdG9ycyE";
    write_private(
        &file,
        &format!(r#"{{"v":1,"kind":"agent","id":"01agent0000000000000000000","key":"{key}"}}"#),
    );
    assert!(read_credential_file(&file).unwrap().is_some());
    for mode in [0o644, 0o640, 0o604, 0o660, 0o666] {
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        let e = read_credential_file(&file).unwrap_err();
        assert_eq!(e.code, "UNAUTHENTICATED", "{mode:o}");
        assert_eq!(e.details["reason"], "credentialUnsafe");
        assert_eq!(e.details["problem"], "readableByOthers");
        assert_eq!(e.details["mode"], format!("{mode:03o}"));
        assert!(
            !format!("{e:?}").contains(key),
            "the error never shows the key"
        );
    }
    // `local.key` in server data is held to the same rule.
    std::fs::create_dir_all(dir.path().join("auth")).unwrap();
    let local = dir.path().join("auth/local.key");
    std::fs::write(&local, format!("{key}\n")).unwrap();
    std::fs::set_permissions(&local, std::fs::Permissions::from_mode(0o644)).unwrap();
    if std::env::var_os(CREDENTIAL_ENV).is_none() {
        let e = discover(Some(dir.path())).unwrap_err();
        assert_eq!(e.details["reason"], "credentialUnsafe");
    }
}

/// Windows (5A Task 9): a `FIN_CREDENTIAL` file whose DACL lets anyone else in is refused,
/// read from the opened handle. The DACL is set explicitly: the temp folder's own may admit
/// more (the host's `%TEMP%` can).
#[cfg(windows)]
#[test]
fn credential_files_must_be_private_windows() {
    let icacls = |file: &Path, args: &[&str]| {
        let out = std::process::Command::new("icacls")
            .arg(file)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "icacls {args:?}: {out:?}");
    };
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("c.json");
    let key = "RkFLRS1sb2NhbC1rZXktZm9yLXRlc3QtdmVjdG9ycyE";
    write_private(
        &file,
        &format!(r#"{{"v":1,"kind":"agent","id":"01agent0000000000000000000","key":"{key}"}}"#),
    );
    let me = format!("{}:F", std::env::var("USERNAME").unwrap());
    icacls(&file, &["/inheritance:r", "/grant:r", &me]);
    assert!(read_credential_file(&file).unwrap().is_some());
    // The local Users group may read it: refused, and the ACL entry is named.
    icacls(&file, &["/grant", "*S-1-5-32-545:R"]);
    let e = read_credential_file(&file).unwrap_err();
    assert_eq!(e.code, "UNAUTHENTICATED");
    assert_eq!(e.details["reason"], "credentialUnsafe");
    assert_eq!(e.details["problem"], "notPrivate");
    assert!(
        e.details["acl"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "allows S-1-5-32-545"),
        "{:?}",
        e.details
    );
    assert!(
        !format!("{e:?}").contains(key),
        "the error never shows the key"
    );
    // Everyone, too.
    icacls(
        &file,
        &["/remove:g", "*S-1-5-32-545", "/grant", "*S-1-1-0:R"],
    );
    assert_eq!(
        read_credential_file(&file).unwrap_err().details["problem"],
        "notPrivate"
    );
}

#[cfg(unix)]
#[test]
fn private_problem_checks_owner_then_mode() {
    assert_eq!(private_problem(0o100600, 501, 501), None);
    assert_eq!(private_problem(0o100400, 501, 501), None);
    assert_eq!(private_problem(0o100700, 501, 501), None);
    assert_eq!(
        private_problem(0o100600, 0, 501),
        Some("ownedByAnotherUser")
    );
    assert_eq!(
        private_problem(0o100644, 501, 501),
        Some("readableByOthers")
    );
    assert_eq!(
        private_problem(0o100602, 501, 501),
        Some("readableByOthers")
    );
    assert_eq!(
        private_problem(0o100610, 501, 501),
        Some("readableByOthers")
    );
}

#[test]
fn credential_files_are_read_up_to_4_kib() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("c.json");
    let key = "RkFLRS1sb2NhbC1rZXktZm9yLXRlc3QtdmVjdG9ycyE";
    let doc =
        format!(r#"{{"v":1,"kind":"agent","id":"01agent0000000000000000000","key":"{key}"}}"#);
    let pad = |to: usize| format!("{doc}{}", " ".repeat(to - doc.len()));
    write_private(&file, &pad(MAX_CREDENTIAL_BYTES as usize));
    assert!(read_credential_file(&file).unwrap().is_some());
    write_private(&file, &pad(MAX_CREDENTIAL_BYTES as usize + 1));
    assert!(read_credential_file(&file).unwrap().is_none());
}

#[test]
fn only_regular_files_are_read() {
    let dir = tempfile::tempdir().unwrap();
    // A directory where a credential file should be: nothing (review of Task 5, L7).
    let sub = dir.path().join("c.json");
    std::fs::create_dir(&sub).unwrap();
    assert!(read_credential_file(&sub).unwrap().is_none());
    // A FIFO is neither read nor waited on.
    #[cfg(unix)]
    {
        let fifo = dir.path().join("fifo.json");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|s| s.success());
        if made {
            assert!(read_credential_file(&fifo).unwrap().is_none());
        }
    }
}
