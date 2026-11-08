//! `fin` over the Linux agent listener (plan 7C Task 6, spec §8.3): `FIN_SERVER=tcp:127.0.0.1:<port>`
//! with an agent credential works, any non-loopback `tcp:` address is `USAGE`, the local key is
//! refused over the listener, and from inside the `agent` profile (the launcher
//! `fin-sandbox-exec`) `fin view` works over the listener.
//!
//! The seccomp filter is deferred (plan Task 4), so "connecting to `fin.sock` fails" from inside
//! the profile is not asserted; `fin_sock_is_a_residual_until_seccomp` asserts the residual instead.

mod support;

use std::ffi::OsStr;

use support::agent_host::AgentHost;
use support::{assert_error, fin_cmd, finish, ok, view_items};

#[test]
fn fin_over_the_listener_with_an_agent_credential() {
    let Some(host) = AgentHost::start("fin_over_the_listener_with_an_agent_credential") else {
        return;
    };
    let (dir, project) = host.project();
    let s = host.open(&project, "draft");
    let ws = host.workspace(&dir, &s);
    let endpoint = host.listener();
    assert!(endpoint.starts_with("tcp:127.0.0.1:"), "{endpoint}");
    let tcp: &[(&str, &OsStr)] = &[("FIN_SERVER", OsStr::new(&endpoint))];
    let who = ok(host.agent_env(&s, &ws, &["whoami"], tcp));
    assert_eq!(who["principal"]["kind"], "agent", "{who}");
    assert!(view_items(&ok(host.agent_env(&s, &ws, &["view"], tcp))).is_empty());

    // Only loopback: anything else is a usage error before any connection.
    let far: &[(&str, &OsStr)] = &[("FIN_SERVER", OsStr::new("tcp:10.0.0.1:1"))];
    assert_error(&host.agent_env(&s, &ws, &["whoami"], far), 2, "USAGE");

    // The local key (a person's `fin`, no credential) is refused by the listener after its proof.
    let mut cmd = fin_cmd(&host.server.sd, &dir, &["whoami"]);
    cmd.env("FIN_SERVER", &endpoint);
    let run = finish(&["whoami"], cmd.output().unwrap());
    assert_ne!(run.code, 0, "{}", run.stdout);
    assert_eq!(
        run.json["error"]["code"], "UNAUTHENTICATED",
        "{}",
        run.stdout
    );
    assert_eq!(
        run.json["error"]["details"]["reason"], "listenerKind",
        "{}",
        run.stdout
    );
}

#[cfg(target_os = "linux")]
mod sandboxed {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use fin_core::events::payloads::agent::{AgentMode, Harness};
    use fin_sandbox::launch::LaunchRequest;
    use fin_sandbox::linux::landlock::{Support, query};
    use fin_sandbox::{AgentInput, FinEndpoint, Os};

    use super::support::agent_host::{AgentHost, Session};
    use super::support::{Run, finish, ok, view_items};

    fn launcher() -> Option<PathBuf> {
        let p = super::support::fontserver::test_native_dir().join("fin-sandbox-exec");
        p.is_file().then_some(p)
    }

    /// Landlock and the launcher, or a printed reason (`FIN_REQUIRE_LANDLOCK=1` makes it fail).
    fn ready(test: &str) -> Option<(PathBuf, u32)> {
        let abi = match query() {
            Support::Abi(a) => Some(a.0),
            other => {
                eprintln!("{test}: skipped, Landlock is not available ({other:?})");
                None
            }
        };
        let l = launcher();
        if l.is_none() {
            eprintln!(
                "{test}: skipped, no fin-sandbox-exec in packages/fin-node/test/native (pnpm build:test)"
            );
        }
        let out = abi.zip(l).map(|(a, l)| (l, a));
        assert!(
            out.is_some() || std::env::var("FIN_REQUIRE_LANDLOCK").as_deref() != Ok("1"),
            "{test}: FIN_REQUIRE_LANDLOCK=1 but the sandboxed case cannot run"
        );
        out
    }

    fn canonical(p: &Path) -> PathBuf {
        std::fs::canonicalize(p).unwrap()
    }

    /// One session of a host's project, with its workspace.
    struct Ctx<'a> {
        host: &'a AgentHost,
        launcher: &'a Path,
        s: &'a Session,
        dir: &'a Path,
        ws: &'a Path,
    }

    /// `fin args` inside the session's `agent` profile, reaching `server` (with `port` in the
    /// profile's TCP allow-list when it is the listener).
    fn sandboxed_fin(c: &Ctx<'_>, server: &str, port: Option<u16>, args: &[&str]) -> Run {
        let Ctx {
            host,
            launcher,
            s,
            dir,
            ws,
        } = *c;
        let sd = canonical(&host.server.sd);
        let session_dir = sd.join("agents").join(&s.id);
        let mut toolchains = fin_sandbox::data::toolchain_base_resolved(Os::Linux);
        toolchains.push(canonical(Path::new(env!("CARGO_BIN_EXE_fin"))));
        let project = canonical(dir);
        let req = LaunchRequest::Agent(AgentInput {
            session_id: s.id.clone(),
            mode: AgentMode::Draft,
            harness: Harness::Codex,
            workspace_dir: project.join("workspace").join(&s.id),
            shared_dir: None,
            project_dir: project.clone(),
            library_roots: vec![],
            session_dir,
            sign_in_store: None,
            toolchains,
            server_data: sd,
            home: canonical(&host.server.root).join("home"),
            fin: port.map(|port| FinEndpoint::Tcp { port }),
        });
        let req_file = host
            .server
            .root
            .join(format!("req-{}.json", args.join("-")));
        std::fs::write(&req_file, serde_json::to_vec(&req).unwrap()).unwrap();
        // `sh` opens the request on descriptor 3 and replaces itself with the launcher, so the
        // descriptor is the launcher's to read and close.
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(r#"exec "$0" --policy-fd 3 -- "$@" 3<"$REQ""#)
            .arg(launcher)
            .arg(env!("CARGO_BIN_EXE_fin"))
            .args(args)
            .current_dir(ws)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("REQ", &req_file)
            .env("HOME", canonical(&host.server.root).join("home"))
            .env("FIN_CREDENTIAL", &s.credential)
            .env("FIN_SERVER", server)
            .env("FIN_PROJECT_DIR", &project)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        finish(args, out)
    }

    #[test]
    fn fin_view_works_over_the_listener_inside_the_agent_profile() {
        let test = "fin_view_works_over_the_listener_inside_the_agent_profile";
        let Some((launcher, _)) = ready(test) else {
            return;
        };
        let Some(host) = AgentHost::start(test) else {
            return;
        };
        let (dir, project) = host.project();
        let s = host.open(&project, "draft");
        let ws = host.workspace(&dir, &s);
        let endpoint = host.listener();
        let port: u16 = endpoint.rsplit(':').next().unwrap().parse().unwrap();
        let c = Ctx {
            host: &host,
            launcher: &launcher,
            s: &s,
            dir: &dir,
            ws: &ws,
        };
        let run = sandboxed_fin(&c, &endpoint, Some(port), &["view"]);
        assert!(view_items(&ok(run)).is_empty());
    }

    #[test]
    fn fin_sock_is_a_residual_until_seccomp() {
        let test = "fin_sock_is_a_residual_until_seccomp";
        let Some((launcher, abi)) = ready(test) else {
            return;
        };
        let Some(host) = AgentHost::start(test) else {
            return;
        };
        let (dir, project) = host.project();
        let s = host.open(&project, "draft");
        let ws = host.workspace(&dir, &s);
        let socket = host.server.socket.clone();
        let c = Ctx {
            host: &host,
            launcher: &launcher,
            s: &s,
            dir: &dir,
            ws: &ws,
        };
        let run = sandboxed_fin(&c, &socket, None, &["whoami"]);
        let probe = std::process::Command::new(&launcher)
            .arg("--probe")
            .env_clear()
            .output()
            .unwrap();
        let probe: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        if probe["namespaces"]["kind"] == "available" {
            // Plan 7C Decision 26: server data's `run/` is hidden by the launcher's namespace.
            assert_eq!(
                run.json["error"]["code"], "SERVER_UNAVAILABLE",
                "{}",
                run.stdout
            );
        } else if abi >= 9 {
            // Landlock `RESOLVE_UNIX`: the socket is not in the policy.
            assert_eq!(
                run.json["error"]["code"], "SERVER_UNAVAILABLE",
                "{}",
                run.stdout
            );
        } else {
            // Recorded residual (plan Task 4 deferred; `SANDBOX_SOCKETS_EXPOSED` where the
            // namespace is unavailable): below ABI 9 only the seccomp filter's
            // `AF_UNIX` refusal would stop this. It runs as the agent (its own credential), not
            // as `local`: `local.key` stays unreadable. When the filter lands, this must fail.
            assert_eq!(ok(run)["principal"]["kind"], "agent");
        }
    }
}
