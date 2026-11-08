//! `fin`'s half of the handshake (spec §4.2; plan 5A Decisions 6, 7, 10 and 11).
//!
//! ```text
//! server.hello {clientNonce, env?, endpoint, credential}
//!   → {envId, endpoint, serverNonce, proof}     fin checks envId, endpoint and the proof
//! auth.prove {mac} → {principal}
//! ```
//!
//! - **Credential discovery:** `FIN_CREDENTIAL` (a credential file
//!   `{"v":1,"kind":"agent"|"mcp","id","key","env"}`), else `<server-data>/auth/local.key`. An
//!   unreadable or malformed one is no credential: fin then fails **before sending anything**
//!   with `UNAUTHENTICATED` (`details.reason: noCredential`), so an unproved server never
//!   receives a request (nor `cli.command`); a server that is not running is still
//!   `SERVER_UNAVAILABLE`. On Unix a key or credential file that other users can read or
//!   that another user owns is refused, as ssh refuses such a key (`reason: credentialUnsafe`);
//!   neither file is read past 4 KiB.
//! - **Nothing an unproved server says is believed:** an error answer to `server.hello` is
//!   `helloFailed` (its code, message and fix are dropped: they would be the squatter's), and a
//!   server from before the handshake is `versionMismatch`. Only fin's own transport and timeout
//!   errors pass through before the proof.
//! - **The retry:** when the server cannot prove itself (`noProof`, `badProof`, `envMismatch`,
//!   `endpointMismatch`, `helloFailed`, `versionMismatch`), fin re-reads `finisherd.json` and the
//!   key **once**, reconnects and
//!   retries: a restart between reading the key and connecting is invisible. A retry that
//!   succeeds through a changed key is noted as `keyStale` on the connection (`fin whoami`
//!   shows it); a second failure is `SERVER_UNTRUSTED` (exit 7), and nothing but the two hellos
//!   reached that server.
//! - MACs: HMAC-SHA-256 over a label and length-prefixed fields (`tests/fixtures/auth/
//!   handshake.json` pins the bytes); the proof is checked in constant time. Keys, nonces and
//!   MACs are never printed.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use fin_core::CoreError;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::client::Client;
use crate::endpoint::{self, Endpoint};

pub const CREDENTIAL_ENV: &str = "FIN_CREDENTIAL";
/// Test hook: `FIN_TEST_AUTH_GATE=<file>` makes the first connect create `<file>.ready.<pid>`
/// after reading the credential, and wait until `<file>` exists (`test-hooks` builds only).
pub const AUTH_GATE_ENV: &str = "FIN_TEST_AUTH_GATE";

pub const LABEL_HELLO: &str = "finisherd/hello/v2";
pub const LABEL_PROVE: &str = "fin/prove/v2";
pub const KEY_BYTES: usize = 32;
/// The most fin reads of a key or credential file; a longer one is malformed.
pub const MAX_CREDENTIAL_BYTES: u64 = 4096;
/// The protocol whose hello carries the handshake. A server below it that answers without the
/// handshake's fields is from an older release (`versionMismatch`), not a squatter. Until 5A
/// Task 6 this branch's own server still says protocol 1, so the missing fields decide.
pub const HANDSHAKE_PROTOCOL: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    /// `<server-data>/auth/local.key`. (`console` is `finisherd <command>`'s, never fin's.)
    Local,
    Agent,
    Mcp,
}

impl CredentialKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CredentialKind::Local => "local",
            CredentialKind::Agent => "agent",
            CredentialKind::Mcp => "mcp",
        }
    }
}

/// A credential fin proves with. `key` never leaves this module except inside a MAC.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    pub kind: CredentialKind,
    pub id: Option<String>,
    pub env: Option<String>,
    key: [u8; KEY_BYTES],
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("kind", &self.kind)
            .field("id", &self.id)
            .field("env", &self.env)
            .finish_non_exhaustive()
    }
}

impl Credential {
    pub fn new(
        kind: CredentialKind,
        id: Option<String>,
        env: Option<String>,
        key: [u8; 32],
    ) -> Self {
        Credential { kind, id, env, key }
    }

    fn reference(&self) -> Value {
        match &self.id {
            Some(id) => json!({ "kind": self.kind.as_str(), "id": id }),
            None => json!({ "kind": self.kind.as_str() }),
        }
    }
}

/// What the server must prove it is: this environment (when known) at this endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expect {
    pub env_id: Option<String>,
    pub endpoint: String,
}

fn decode_key(text: &str) -> Option<[u8; KEY_BYTES]> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.len() != 43 {
        return None;
    }
    let bytes = B64.decode(text).ok()?;
    // Canonical encoding only, as the server's `b64u.decode`.
    if B64.encode(&bytes) != text {
        return None;
    }
    bytes.try_into().ok()
}

#[cfg(unix)]
unsafe extern "C" {
    /// POSIX `geteuid(2)`: always succeeds. (`uid_t` is `u32` on every Unix fin builds for.)
    safe fn geteuid() -> u32;
}

/// Why a key or credential file with this `mode` and owner `uid` is refused, if it is.
#[cfg(unix)]
fn private_problem(mode: u32, uid: u32, euid: u32) -> Option<&'static str> {
    if uid != euid {
        Some("ownedByAnotherUser")
    } else if mode & 0o077 != 0 {
        Some("readableByOthers")
    } else {
        None
    }
}

fn unsafe_credential(path: &Path, problem: &str, mode: u32) -> CoreError {
    CoreError::new(
        "UNAUTHENTICATED",
        format!(
            "the credential file {} is not private, so fin will not use it",
            path.display()
        ),
    )
    .with_details(json!({
        "reason": "credentialUnsafe",
        "path": path.display().to_string(),
        "problem": problem,
        "mode": format!("{:03o}", mode & 0o777),
    }))
    .with_fix(
        "A credential file must belong to you and be readable and writable by you alone (mode \
         600), as for an SSH key, so other users cannot copy it. Change its owner or permissions \
         that way, or ask for a new credential, then try again.",
    )
}

/// The text of a key or credential file: `Ok(None)` when it cannot be read or is over
/// [`MAX_CREDENTIAL_BYTES`]; an error when it is not private (Unix: the opened file's owner and
/// mode, so a swap after the check reads nothing else).
fn read_private(path: &Path) -> Result<Option<String>, CoreError> {
    let Ok(file) = std::fs::File::open(path) else {
        return Ok(None);
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let Ok(meta) = file.metadata() else {
            return Ok(None);
        };
        if let Some(problem) = private_problem(meta.mode(), meta.uid(), geteuid()) {
            return Err(unsafe_credential(path, problem, meta.mode()));
        }
    }
    let mut text = String::new();
    match file
        .take(MAX_CREDENTIAL_BYTES + 1)
        .read_to_string(&mut text)
    {
        Ok(n) if n as u64 <= MAX_CREDENTIAL_BYTES => Ok(Some(text)),
        _ => Ok(None),
    }
}

fn read_credential_file(path: &Path) -> Result<Option<Credential>, CoreError> {
    Ok(read_private(path)?.and_then(|text| parse_credential(&text)))
}

fn parse_credential(text: &str) -> Option<Credential> {
    let doc: Value = serde_json::from_str(text).ok()?;
    if doc.get("v").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let kind = match doc.get("kind").and_then(Value::as_str)? {
        "agent" => CredentialKind::Agent,
        "mcp" => CredentialKind::Mcp,
        _ => return None,
    };
    let id = doc
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    let key = decode_key(doc.get("key").and_then(Value::as_str)?)?;
    let env = doc.get("env").and_then(Value::as_str).map(str::to_string);
    Some(Credential::new(kind, Some(id.to_string()), env, key))
}

/// `FIN_CREDENTIAL`, else `<server-data>/auth/local.key` (Decision 11). `Ok(None)`: none found
/// (missing, unreadable or malformed); an error: one was found and is not private.
pub fn discover(server_data: Option<&Path>) -> Result<Option<Credential>, CoreError> {
    if let Some(file) = std::env::var_os(CREDENTIAL_ENV).filter(|v| !v.is_empty()) {
        return read_credential_file(&crate::paths::resolve(Path::new(&file)));
    }
    let Some(sd) = server_data else {
        return Ok(None);
    };
    let text = read_private(&sd.join("auth").join("local.key"))?;
    Ok(text
        .and_then(|t| decode_key(&t))
        .map(|key| Credential::new(CredentialKind::Local, None, None, key)))
}

/// The form both MACs bind (Decision 7): a socket's real path, a pipe's name lower-cased.
pub fn canonical_endpoint(endpoint: &Endpoint) -> String {
    match endpoint {
        Endpoint::Pipe(name) => name.to_lowercase(),
        Endpoint::Socket(path) => std::fs::canonicalize(path)
            .unwrap_or_else(|_| PathBuf::from(path))
            .display()
            .to_string(),
    }
}

fn mac_input(label: &str, strings: [&str; 2], nonces: [&[u8; 32]; 2]) -> Vec<u8> {
    let mut out = label.as_bytes().to_vec();
    for s in strings {
        // Fields above 65,535 bytes cannot be encoded; no envId or endpoint is that long.
        let len = u16::try_from(s.len()).unwrap_or(u16::MAX);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    }
    for n in nonces {
        out.extend_from_slice(n);
    }
    out
}

fn hmac(key: &[u8; 32], input: &[u8]) -> Hmac<Sha256> {
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(input);
    mac
}

fn hello_mac(
    key: &[u8; 32],
    env: &str,
    endpoint: &str,
    cn: &[u8; 32],
    sn: &[u8; 32],
) -> Hmac<Sha256> {
    hmac(key, &mac_input(LABEL_HELLO, [env, endpoint], [cn, sn]))
}

/// `HMAC(key, "finisherd/hello/v2" ‖ envId ‖ endpoint ‖ clientNonce ‖ serverNonce)`: fin only
/// verifies it ([`handshake`], in constant time); the vectors test computes it.
#[cfg(test)]
pub fn hello_proof(
    key: &[u8; 32],
    env: &str,
    endpoint: &str,
    cn: &[u8; 32],
    sn: &[u8; 32],
) -> [u8; 32] {
    hello_mac(key, env, endpoint, cn, sn)
        .finalize()
        .into_bytes()
        .into()
}

/// `HMAC(key, "fin/prove/v2" ‖ envId ‖ endpoint ‖ serverNonce ‖ clientNonce)`.
pub fn prove_mac(
    key: &[u8; 32],
    env: &str,
    endpoint: &str,
    sn: &[u8; 32],
    cn: &[u8; 32],
) -> [u8; 32] {
    hmac(key, &mac_input(LABEL_PROVE, [env, endpoint], [sn, cn]))
        .finalize()
        .into_bytes()
        .into()
}

/// A handshake that did not establish the server.
#[derive(Debug)]
pub enum Failure {
    /// The server did not prove itself: worth one retry (Decision 10).
    Untrusted(&'static str),
    /// fin's own transport or timeout error, or the error of a server that has proved itself.
    Other(CoreError),
}

fn nonce() -> [u8; 32] {
    let mut n = [0u8; 32];
    getrandom::fill(&mut n).expect("the OS random source is available");
    n
}

/// Hello, check the server's proof, prove. Returns the principal the server bound.
pub fn handshake(conn: &mut Client, cred: &Credential, expect: &Expect) -> Result<Value, Failure> {
    let cn = nonce();
    let mut hello = json!({
        "clientNonce": B64.encode(cn),
        "endpoint": expect.endpoint,
        "credential": cred.reference(),
    });
    if let Some(env) = &expect.env_id {
        hello["env"] = json!(env);
    }
    // An error answer comes from a server that has not proved anything: its code, message and
    // fix are not shown (a squatter would choose them); only fin's own transport errors are.
    let Ok(answer) = conn
        .call_answer("server.hello", hello)
        .map_err(Failure::Other)?
    else {
        return Err(Failure::Untrusted("helloFailed"));
    };
    let field = |k: &str| answer.get(k).and_then(Value::as_str);
    let version = answer.get("protocolVersion").and_then(Value::as_u64);
    if version.is_none_or(|v| v < HANDSHAKE_PROTOCOL) && answer.get("serverNonce").is_none() {
        return Err(Failure::Untrusted("versionMismatch"));
    }
    let Some(env) = field("envId") else {
        return Err(Failure::Untrusted("noProof"));
    };
    if expect.env_id.as_deref().is_some_and(|e| e != env) {
        return Err(Failure::Untrusted("envMismatch"));
    }
    if field("endpoint") != Some(expect.endpoint.as_str()) {
        return Err(Failure::Untrusted("endpointMismatch"));
    }
    let Some(proof) = field("proof") else {
        return Err(Failure::Untrusted("noProof"));
    };
    let sn: [u8; 32] = field("serverNonce")
        .and_then(|s| B64.decode(s).ok())
        .and_then(|b| b.try_into().ok())
        .ok_or(Failure::Untrusted("badProof"))?;
    let proof = B64
        .decode(proof)
        .map_err(|_| Failure::Untrusted("badProof"))?;
    hello_mac(&cred.key, env, &expect.endpoint, &cn, &sn)
        .verify_slice(&proof)
        .map_err(|_| Failure::Untrusted("badProof"))?;
    let mac = prove_mac(&cred.key, env, &expect.endpoint, &sn, &cn);
    let proved = conn
        .call("auth.prove", json!({ "mac": B64.encode(mac) }))
        .map_err(Failure::Other)?;
    Ok(proved.get("principal").cloned().unwrap_or(Value::Null))
}

/// The credential and the expected server for `endpoint`, read now.
fn target(endpoint: &Endpoint) -> Result<(Option<Credential>, Expect), CoreError> {
    let cred = discover(endpoint::server_data().as_deref())?;
    let env_id = cred
        .as_ref()
        .and_then(|c| c.env.clone())
        .or_else(endpoint::info_env_id);
    let expect = Expect {
        env_id,
        endpoint: canonical_endpoint(endpoint),
    };
    Ok((cred, expect))
}

fn gate() {
    let Some(gate) = crate::backend::test_hook(AUTH_GATE_ENV).map(PathBuf::from) else {
        return;
    };
    let _ = std::fs::write(
        format!("{}.ready.{}", gate.display(), std::process::id()),
        b"",
    );
    let deadline = std::time::Instant::now() + crate::client::TIMEOUT;
    while !gate.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

pub fn untrusted(endpoint: &Endpoint, reason: &str) -> CoreError {
    let at = endpoint.display();
    let details = json!({ "endpoint": at, "reason": reason });
    if reason == "versionMismatch" {
        return CoreError::new(
            "SERVER_UNTRUSTED",
            format!("the server at {at} is from an older Finisher release and cannot prove it is your Finisher server"),
        )
        .with_details(details)
        .with_fix(
            "Your Finisher server is older than this command and does not know how to prove \
             itself. Restart it from the same Finisher release as this command, then try again.",
        );
    }
    CoreError::new(
        "SERVER_UNTRUSTED",
        format!("the server at {at} did not prove it is your Finisher server"),
    )
    .with_details(details)
    .with_fix(
        "Another program may be listening where your Finisher server should be. Stop it, or check \
         that this command uses the same server data as your Finisher server, then try again.",
    )
}

/// The `UNAUTHENTICATED` of a run that found no credential: fin sends the server nothing.
pub fn no_credential(endpoint: &Endpoint) -> CoreError {
    CoreError::new(
        "UNAUTHENTICATED",
        format!(
            "fin found no credential to prove to the server at {}",
            endpoint.display()
        ),
    )
    .with_details(json!({ "reason": "noCredential", "endpoint": endpoint.display() }))
    .with_fix(
        "This command found no credential. It reads the key in your Finisher server's data \
         folder (the FIN_SERVER_DATA variable, or the default folder), or the credential file the \
         FIN_CREDENTIAL variable names. Run it as the user who runs the server, with that \
         server's data, or give it a credential file.",
    )
}

/// Connects with `open` and proves the discovered credential, retrying once (Decision 10).
pub fn connect(
    endpoint: &Endpoint,
    open: impl Fn(&Endpoint) -> Result<Client, CoreError>,
) -> Result<Client, CoreError> {
    let (cred, expect) = target(endpoint)?;
    gate();
    // Connecting first keeps "no server is running" the answer when none is; without a
    // credential nothing can be proved, so nothing is sent (Decision 11).
    let mut client = open(endpoint)?;
    let Some(cred) = cred else {
        return Err(no_credential(endpoint));
    };
    let first = match handshake(&mut client, &cred, &expect) {
        Ok(principal) => {
            client.set_principal(principal, None);
            return Ok(client);
        }
        Err(Failure::Other(e)) => return Err(e),
        Err(Failure::Untrusted(reason)) => reason,
    };
    drop(client);
    // Once: `finisherd.json` (unless FIN_SERVER names the endpoint) and the key, read again.
    let endpoint = if std::env::var_os(endpoint::SERVER_ENV).is_some_and(|v| !v.is_empty()) {
        endpoint.clone()
    } else {
        endpoint::resolve().map_err(|_| untrusted(endpoint, first))?
    };
    let (again, expect) = target(&endpoint)?;
    let Some(again) = again else {
        return Err(untrusted(&endpoint, first));
    };
    let mut client = open(&endpoint)?;
    match handshake(&mut client, &again, &expect) {
        Ok(principal) => {
            let note = if again != cred { "keyStale" } else { first };
            client.set_principal(principal, Some(note));
            Ok(client)
        }
        Err(Failure::Untrusted(reason)) => Err(untrusted(&endpoint, reason)),
        Err(Failure::Other(e)) => Err(e),
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
