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
    std::fs::write(
        &file,
        format!(
            r#"{{"v":1,"kind":"mcp","id":"01mcp00000000000000000000a","key":"{key}","env":"e"}}"#
        ),
    )
    .unwrap();
    let c = read_credential_file(&file).unwrap();
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
        std::fs::write(&file, bad.replace('K', key)).unwrap();
        assert!(read_credential_file(&file).is_none(), "{bad}");
    }
}

#[test]
fn local_key_is_found_in_server_data() {
    let dir = tempfile::tempdir().unwrap();
    assert!(discover(Some(dir.path())).is_none() || std::env::var_os(CREDENTIAL_ENV).is_some());
    std::fs::create_dir_all(dir.path().join("auth")).unwrap();
    std::fs::write(
        dir.path().join("auth/local.key"),
        "RkFLRS1sb2NhbC1rZXktZm9yLXRlc3QtdmVjdG9ycyE\n",
    )
    .unwrap();
    if std::env::var_os(CREDENTIAL_ENV).is_none() {
        let c = discover(Some(dir.path())).unwrap();
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
    let n = no_credential(
        CoreError::new("UNAUTHENTICATED", "x").with_details(json!({ "reason": "unproved" })),
    );
    assert_eq!(n.details["reason"], "noCredential");
    let fix = n.fix.unwrap();
    assert!(
        fix.contains("FIN_CREDENTIAL") && fix.contains("server's data"),
        "{fix}"
    );
    for f in [e.fix.unwrap(), fix] {
        assert!(
            !f.contains('`') && !f.contains("fin ") && !f.contains("finisherd "),
            "{f}"
        );
    }
}
