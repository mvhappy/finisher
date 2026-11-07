//! The handshake for tests that speak raw NDJSON to a real finisherd (spec §4.2): since 5A
//! Task 4 an unproved connection may only say hello. The MACs are computed here again (fin is a
//! binary crate); `tests/fixtures/auth/handshake.json` pins both implementations.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

/// `label ‖ u16be(len) ‖ envId ‖ u16be(len) ‖ endpoint ‖ nonce ‖ nonce`, HMAC-SHA-256.
pub fn mac(key: &[u8], label: &str, env: &str, endpoint: &str, a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(key).unwrap();
    m.update(label.as_bytes());
    for s in [env, endpoint] {
        m.update(&(s.len() as u16).to_be_bytes());
        m.update(s.as_bytes());
    }
    m.update(a);
    m.update(b);
    m.finalize().into_bytes().to_vec()
}

/// A server-data key file's 32 bytes (`auth/local.key`, `auth/console.key`).
pub fn key(sd: &Path, name: &str) -> Vec<u8> {
    let text = std::fs::read_to_string(sd.join("auth").join(format!("{name}.key"))).unwrap();
    B64.decode(text.trim_end()).unwrap()
}

/// The canonical endpoint of a socket path (its real path) or a pipe name (lower-cased).
pub fn canonical(socket: &str) -> String {
    if socket.starts_with(r"\\") {
        return socket.to_lowercase();
    }
    std::fs::canonicalize(socket)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| socket.to_string())
}

fn exchange<S: Read + Write>(io: &mut BufReader<S>, id: &str, tag: &str, payload: Value) -> Value {
    let frame =
        json!({ "_tag": "Request", "id": id, "tag": tag, "payload": payload, "headers": [] });
    io.get_mut()
        .write_all(format!("{frame}\n").as_bytes())
        .unwrap();
    io.get_mut().flush().unwrap();
    loop {
        let mut line = String::new();
        assert!(
            io.read_line(&mut line).unwrap() > 0,
            "server closed the connection"
        );
        let f: Value = serde_json::from_str(&line).unwrap();
        if f["_tag"] == "Exit" && f["requestId"] == id {
            assert_eq!(f["exit"]["_tag"], "Success", "{tag}: {f}");
            return f["exit"]["value"].clone();
        }
    }
}

/// Hello, check the server's proof, prove `name`'s key (`local` or `console`). Returns the
/// principal. Request ids `auth-1`/`auth-2` never collide with a fixture's.
pub fn prove<S: Read + Write>(io: &mut BufReader<S>, sd: &Path, socket: &str, name: &str) -> Value {
    let key = key(sd, name);
    let endpoint = canonical(socket);
    let cn: [u8; 32] = std::array::from_fn(|i| (i as u8) ^ 0x5a ^ (std::process::id() as u8));
    let hello = exchange(
        io,
        "auth-1",
        "server.hello",
        json!({ "clientNonce": B64.encode(cn), "endpoint": endpoint, "credential": { "kind": name } }),
    );
    let env = hello["envId"].as_str().unwrap();
    assert_eq!(hello["endpoint"], endpoint);
    let sn = B64.decode(hello["serverNonce"].as_str().unwrap()).unwrap();
    let proof = B64
        .decode(hello["proof"].as_str().expect("a proof"))
        .unwrap();
    assert_eq!(
        proof,
        mac(&key, "finisherd/hello/v2", env, &endpoint, &cn, &sn),
        "server proof"
    );
    let m = mac(&key, "fin/prove/v2", env, &endpoint, &sn, &cn);
    exchange(io, "auth-2", "auth.prove", json!({ "mac": B64.encode(m) }))["principal"].clone()
}

/// A fake server that answers every request with `FinError {code}` (and the hello normally).
pub struct FailingServer {
    _dir: tempfile::TempDir,
    pub socket: std::path::PathBuf,
}

pub fn fake_failing_server(code: &str) -> FailingServer {
    use interprocess::local_socket::prelude::*;
    use interprocess::local_socket::{GenericFilePath, ListenerOptions};
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().canonicalize().unwrap().join("failing.sock");
    let name = socket.as_path().to_fs_name::<GenericFilePath>().unwrap();
    let listener = ListenerOptions::new().name(name).create_sync().unwrap();
    let code = code.to_string();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let code = code.clone();
            std::thread::spawn(move || {
                let mut io = BufReader::new(conn);
                let mut line = String::new();
                while io.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
                    let frame: Value = serde_json::from_str(line.trim_end()).unwrap_or_default();
                    line.clear();
                    if frame["_tag"] != "Request" {
                        continue;
                    }
                    let error = json!({ "_tag": "FinError", "code": code, "message": "denied", "details": {}, "fix": "ask the owner" });
                    let exit = json!({ "_tag": "Exit", "requestId": frame["id"], "exit": { "_tag": "Failure", "cause": [{ "_tag": "Fail", "error": error }] } });
                    if io
                        .get_mut()
                        .write_all(format!("{exit}\n").as_bytes())
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    FailingServer { _dir: dir, socket }
}
