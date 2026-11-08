//! Where `finisherd`'s local socket is (plan decisions 5 and 6).
//!
//! `FIN_SERVER` names it directly: a Unix socket path, or `pipe:<name>` for the Windows named
//! pipe `\\.\pipe\<name>`. Otherwise `<server-data>/finisherd.json` (written by the running
//! server) gives its `socket`. Server-data is `FIN_SERVER_DATA`, else the per-OS default.

use std::path::{Path, PathBuf};

use fin_core::CoreError;
use serde_json::{Value, json};

use crate::output::unavailable;
use crate::paths;

pub const SERVER_ENV: &str = "FIN_SERVER";
pub const SERVER_DATA_ENV: &str = "FIN_SERVER_DATA";
const INFO_FILE: &str = "finisherd.json";
const PIPE_PREFIX: &str = r"\\.\pipe\";
/// The `protocolVersion` this client speaks (`@finisher/contracts` `PROTOCOL_VERSION`).
pub const PROTOCOL_VERSION: u64 = 1;

/// A local socket address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// A Unix domain socket file.
    Socket(PathBuf),
    /// A Windows named pipe, as the full `\\.\pipe\<name>` path.
    Pipe(String),
}

impl Endpoint {
    /// `pipe:<name>` or a `\\.\pipe\...` path is a named pipe; anything else is a socket path.
    pub fn parse(spec: &str) -> Endpoint {
        if let Some(name) = spec.strip_prefix("pipe:") {
            return Endpoint::Pipe(format!("{PIPE_PREFIX}{name}"));
        }
        if spec.starts_with(PIPE_PREFIX) {
            return Endpoint::Pipe(spec.to_string());
        }
        Endpoint::Socket(paths::resolve_str(spec))
    }

    pub fn display(&self) -> String {
        match self {
            Endpoint::Socket(p) => p.display().to_string(),
            Endpoint::Pipe(p) => p.clone(),
        }
    }
}

fn os_default(home: Option<PathBuf>) -> Option<PathBuf> {
    if cfg!(windows) {
        let local = std::env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        return local
            .or_else(|| home.map(|h| h.join("AppData").join("Local")))
            .map(|d| d.join("Finisher"));
    }
    if cfg!(target_os = "macos") {
        return home.map(|h| {
            h.join("Library")
                .join("Application Support")
                .join("Finisher")
        });
    }
    let xdg = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    xdg.or_else(|| home.map(|h| h.join(".local").join("share")))
        .map(|d| d.join("finisher"))
}

/// Plan decision 6: `FIN_SERVER_DATA`, else the per-OS application data directory.
pub fn server_data() -> Option<PathBuf> {
    match std::env::var_os(SERVER_DATA_ENV) {
        Some(dir) if !dir.is_empty() => Some(paths::resolve(Path::new(&dir))),
        _ => os_default(paths::home()),
    }
}

/// Reads `socket` (and checks `protocolVersion`) from a `finisherd.json` document.
fn from_info(file: &Path, text: &str) -> Result<Endpoint, CoreError> {
    let bad =
        |why: &str| unavailable(json!({ "infoFile": file.display().to_string(), "reason": why }));
    let info: Value = serde_json::from_str(text).map_err(|e| bad(&format!("not JSON: {e}")))?;
    crate::diag::note_discovery(&info);
    let socket = info
        .get("socket")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("no \"socket\""))?;
    let version = info.get("protocolVersion").and_then(Value::as_u64);
    if version != Some(PROTOCOL_VERSION) {
        let shown = version.map_or_else(
            || "no protocol version".to_string(),
            |v| format!("protocol {v}"),
        );
        return Err(CoreError::new("PROTOCOL_MISMATCH", format!("finisherd speaks {shown}, fin speaks {PROTOCOL_VERSION}"))
            .with_details(json!({ "server": version, "client": PROTOCOL_VERSION, "infoFile": file.display().to_string() }))
            .with_fix("update fin or finisherd so both come from the same Finisher release"));
    }
    Ok(Endpoint::parse(socket))
}

/// The `envId` the running server published in `finisherd.json` (5A Task 2), unless `FIN_SERVER`
/// names the endpoint: the handshake expects it (5A Decision 10).
///
/// Why `FIN_SERVER` drops the binding: it names a server directly (a test server, one run with
/// other server data), and `finisherd.json` in this fin's server data then describes some other
/// server, whose `envId` would refuse the right one. The proof still binds what matters: only a
/// holder of the key can make it, and the canonical endpoint is inside both MACs. A credential
/// file's own `env` is still expected under `FIN_SERVER` (`auth::target`).
pub fn info_env_id() -> Option<String> {
    if std::env::var_os(SERVER_ENV).is_some_and(|v| !v.is_empty()) {
        return None;
    }
    let text = std::fs::read_to_string(server_data()?.join(INFO_FILE)).ok()?;
    let info: Value = serde_json::from_str(&text).ok()?;
    info.get("envId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The endpoint to connect to; `SERVER_UNAVAILABLE` (fix "start finisherd") when no server
/// has published one.
pub fn resolve() -> Result<Endpoint, CoreError> {
    if let Some(spec) = std::env::var_os(SERVER_ENV).filter(|v| !v.is_empty()) {
        return Ok(Endpoint::parse(&spec.to_string_lossy()));
    }
    let Some(sd) = server_data() else {
        return Err(unavailable(
            json!({ "reason": "no home directory to find server data in", "env": SERVER_DATA_ENV }),
        ));
    };
    let file = sd.join(INFO_FILE);
    match std::fs::read_to_string(&file) {
        Ok(text) => from_info(&file, &text),
        Err(e) => Err(unavailable(json!({
            "serverData": sd.display().to_string(),
            "infoFile": file.display().to_string(),
            "reason": format!("{:?}", e.kind()),
        }))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_parse_to_sockets_and_pipes() {
        assert_eq!(
            Endpoint::parse("pipe:finisher-ann"),
            Endpoint::Pipe(r"\\.\pipe\finisher-ann".into())
        );
        assert_eq!(
            Endpoint::parse(r"\\.\pipe\finisher-ann"),
            Endpoint::Pipe(r"\\.\pipe\finisher-ann".into())
        );
        #[cfg(unix)]
        assert_eq!(
            Endpoint::parse("/tmp/sd/fin.sock"),
            Endpoint::Socket("/tmp/sd/fin.sock".into())
        );
        assert!(matches!(Endpoint::parse("sd/fin.sock"), Endpoint::Socket(p) if p.is_absolute()));
    }

    #[test]
    fn info_file_gives_the_socket_and_checks_the_protocol() {
        let file = Path::new("/sd/finisherd.json");
        let ok = r#"{"pid":1,"wsPort":2,"socket":"/sd/fin.sock","token":"t","protocolVersion":1}"#;
        assert_eq!(
            from_info(file, ok).unwrap().display(),
            Endpoint::parse("/sd/fin.sock").display()
        );
        assert_eq!(from_info(file, "{").unwrap_err().code, "SERVER_UNAVAILABLE");
        assert_eq!(
            from_info(file, r#"{"pid":1}"#).unwrap_err().code,
            "SERVER_UNAVAILABLE"
        );
        assert_eq!(
            from_info(file, r#"{"socket":"/sd/fin.sock"}"#)
                .unwrap_err()
                .code,
            "PROTOCOL_MISMATCH"
        );
        let newer = r#"{"socket":"/sd/fin.sock","protocolVersion":2}"#;
        assert_eq!(
            from_info(file, newer).unwrap_err().code,
            "PROTOCOL_MISMATCH"
        );
    }

    #[test]
    fn os_default_follows_decision_6() {
        let d = os_default(Some(PathBuf::from("/home/ann"))).unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(
                d,
                PathBuf::from("/home/ann/Library/Application Support/Finisher")
            );
        } else if cfg!(windows) {
            assert!(d.ends_with("Finisher"));
        } else {
            assert!(d.ends_with("finisher"));
        }
    }
}
