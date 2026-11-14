//! The native Windows pipe listener (sub-project 5A Task 11, Decision 38): first instance,
//! remote clients rejected, the descriptor read back, other tokens refused, peers reported,
//! AppContainer SIDs allowed, graceful close and shutdown. The crate is a napi `cdylib`, so this
//! includes the napi-free listener (`pipe_win`, `pipe_loop`, `pipe_sec`) by path.

#![cfg(windows)]
#![allow(dead_code)]

#[path = "../src/pipe_loop.rs"]
mod pipe_loop;
#[path = "../src/pipe_sec.rs"]
mod pipe_sec;
#[path = "../src/pipe_win.rs"]
mod pipe_win;
#[path = "pipe_server/support.rs"]
mod support;

use std::io::{Read, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use pipe_win::{Event, Options, PipeCore, StartError};
use support::*;

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A pipe name of its own per test.
fn name() -> String {
    format!(
        r"\\.\pipe\finisher-pipe-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    )
}

fn start(name: &str, options: Options) -> (PipeCore, mpsc::Receiver<Event>) {
    let core = PipeCore::new(name, options).expect("the pipe server starts");
    let (tx, rx) = mpsc::channel();
    core.set_handler(Box::new(move |e| {
        let _ = tx.send(e);
    }));
    (core, rx)
}

fn connected(rx: &mpsc::Receiver<Event>) -> pipe_win::Peer {
    match rx.recv_timeout(Duration::from_secs(10)).expect("an event") {
        Event::Connected(p) => p,
        Event::Refused(r) => panic!("refused: {r:?}"),
    }
}

/// `core.read(id)`, waited for.
fn read(core: &PipeCore, id: u32) -> Option<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    core.read(id, Box::new(move |r| tx.send(r).unwrap()));
    rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap()
}

fn write(core: &PipeCore, id: u32, data: &[u8]) -> std::io::Result<()> {
    let (tx, rx) = mpsc::channel();
    core.write(id, data.to_vec(), Box::new(move |r| tx.send(r).unwrap()));
    rx.recv_timeout(Duration::from_secs(10)).unwrap()
}

#[test]
fn first_instance_or_pipe_squatted() {
    let n = name();
    // Someone else's pipe of the same name (default descriptor) came first.
    let squatter = raw_instance(&n).expect("the squatter's pipe");
    assert!(matches!(
        PipeCore::new(&n, Options::default()),
        Err(StartError::Squatted(_))
    ));
    drop(squatter);
    // Once it is gone the name can be taken; a second listener of it is refused.
    let (core, _rx) = start(&n, Options::default());
    assert!(matches!(
        PipeCore::new(&n, Options::default()),
        Err(StartError::Squatted(_))
    ));
    core.shutdown();
}

#[test]
fn remote_clients_rejected() {
    let n = name();
    let (core, rx) = start(
        &n,
        Options {
            max_connections: 5,
            ..Options::default()
        },
    );
    let client = open(&n, CLIENT_ACCESS).expect("a local client connects");
    let info = pipe_info(&client).expect("FilePipeLocalInformation");
    assert_ne!(
        info.NamedPipeType & FILE_PIPE_REJECT_REMOTE_CLIENTS,
        0,
        "the pipe rejects remote clients (type {:#x})",
        info.NamedPipeType
    );
    // The five client instances and the anchor.
    assert_eq!(info.MaximumInstances, 6);
    connected(&rx);
    core.shutdown();
}

#[test]
fn dacl_is_user_and_system_only() {
    let n = name();
    let (core, rx) = start(&n, Options::default());
    let client = open(&n, CLIENT_ACCESS | READ_CONTROL).expect("a client with READ_CONTROL");
    let sddl = dacl_sddl(&client).expect("the pipe's DACL");
    let user = core.user_sid().to_string();
    assert!(sddl.starts_with("D:P"), "a protected DACL: {sddl}");
    let mut sids = ace_sids(&sddl);
    sids.sort();
    let mut want = vec![user.clone(), "SY".to_string()];
    want.sort();
    assert_eq!(sids, want, "only this user and SYSTEM: {sddl}");
    assert!(
        sddl.contains(&format!("(A;;FA;;;{user})")),
        "full access for the user: {sddl}"
    );
    connected(&rx);
    core.shutdown();
}

#[test]
fn other_user_cannot_open() {
    let n = name();
    let (core, rx) = start(&n, Options::default());
    // A token whose user SID is deny-only: the DACL grants it nothing (no second account needed).
    let err = with_restricted_token(|| open(&n, CLIENT_ACCESS)).expect_err("refused by the DACL");
    assert_eq!(err.raw_os_error(), Some(5), "ERROR_ACCESS_DENIED: {err}");
    assert!(
        rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "no connection was accepted"
    );
    // The listener is still there for the user.
    open(&n, CLIENT_ACCESS).expect("the user connects");
    connected(&rx);
    core.shutdown();
}

#[test]
fn peer_check_matches_user_and_containers() {
    use pipe_sec::{Identity, peer_allowed};
    let ours = "S-1-5-21-1-2-3-1001";
    let ac = "S-1-15-2-1-2-3-4-5-6-7";
    let id = |user: &str, ac: Option<&str>, integrity: u32| Identity {
        user: user.into(),
        app_container: ac.map(Into::into),
        integrity,
    };
    let (low, medium, high) = (0x1000, 0x2000, 0x3000);
    assert!(peer_allowed(ours, &[], &id(ours, None, medium)));
    assert!(peer_allowed(ours, &[], &id(ours, None, high)));
    assert!(
        !peer_allowed(ours, &[], &id(ours, None, low)),
        "low integrity outside a container"
    );
    assert!(
        !peer_allowed(ours, &[], &id(ours, None, 0)),
        "unknown integrity"
    );
    assert!(!peer_allowed(
        ours,
        &[],
        &id("S-1-5-21-1-2-3-1002", None, medium)
    ));
    assert!(!peer_allowed(ours, &[], &id(ours, Some(ac), low)));
    assert!(peer_allowed(ours, &[ac.into()], &id(ours, Some(ac), low)));
    assert!(!peer_allowed(ours, &[ac.into()], &id(ours, None, low)));
    assert!(!peer_allowed(
        ours,
        &[ac.into()],
        &id("S-1-5-21-9-9-9-1", Some(ac), low)
    ));
    assert_eq!(
        pipe_sec::sddl(ours, &[]),
        format!("D:P(A;;GA;;;{ours})(A;;GA;;;SY)")
    );
    assert_eq!(
        pipe_sec::sddl(ours, &[ac.into()]),
        format!("D:P(A;;GA;;;{ours})(A;;GA;;;SY)(A;;0x12008b;;;{ac})S:(ML;;NW;;;LW)")
    );
}

#[test]
fn peer_pid_and_sid_reported() {
    let n = name();
    let (core, rx) = start(&n, Options::default());
    let _client = open(&n, CLIENT_ACCESS).unwrap();
    let peer = connected(&rx);
    assert_eq!(peer.pid, std::process::id());
    assert_eq!(peer.sid, core.user_sid());
    assert_eq!(peer.app_container, None);
    core.shutdown();
}

#[test]
fn round_trip_and_graceful_close() {
    let n = name();
    let (core, rx) = start(&n, Options::default());
    let mut client = open(&n, CLIENT_ACCESS).unwrap();
    let peer = connected(&rx);
    client.write_all(b"hello\n").unwrap();
    assert_eq!(read(&core, peer.id).as_deref(), Some(&b"hello\n"[..]));
    // A reply far larger than the pipe's buffer, then a close: the client still reads all of it.
    let big: Vec<u8> = (0..(1u32 << 20)).map(|i| (i % 251) as u8).collect();
    let (tx, done) = mpsc::channel();
    core.write(peer.id, big.clone(), Box::new(move |r| tx.send(r).unwrap()));
    core.close(peer.id);
    let mut got = Vec::new();
    client
        .read_to_end(&mut got)
        .or_else(|e| {
            if e.raw_os_error() == Some(109) {
                Ok(0)
            } else {
                Err(e)
            }
        })
        .unwrap();
    assert_eq!(got.len(), big.len());
    assert!(got == big);
    done.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    // Closed: reads end, writes fail.
    assert_eq!(read(&core, peer.id), None);
    assert!(write(&core, peer.id, b"x").is_err());
    core.shutdown();
}

#[test]
fn client_leaving_ends_the_read() {
    let n = name();
    let (core, rx) = start(&n, Options::default());
    let client = open(&n, CLIENT_ACCESS).unwrap();
    let peer = connected(&rx);
    let (tx, got) = mpsc::channel();
    core.read(peer.id, Box::new(move |r| tx.send(r).unwrap()));
    drop(client);
    assert_eq!(
        got.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(),
        None
    );
    let e = write(&core, peer.id, b"late").expect_err("the client is gone");
    assert!(matches!(e.raw_os_error(), Some(109 | 232 | 233)), "{e}");
    core.close(peer.id);
    core.shutdown();
}

#[test]
fn max_connections_caps_instances() {
    let n = name();
    let (core, rx) = start(
        &n,
        Options {
            max_connections: 2,
            ..Options::default()
        },
    );
    // Each client waits for the next instance, as `fin` and libuv do (`ERROR_PIPE_BUSY` until
    // the loop has created it).
    let a = open_waiting(&n, Duration::from_secs(5)).unwrap();
    let a_id = connected(&rx).id;
    let _b = open_waiting(&n, Duration::from_secs(5)).unwrap();
    connected(&rx);
    // Both instances are taken: a third client waits and gives up.
    let busy = open_waiting(&n, Duration::from_millis(300)).expect_err("every instance is busy");
    assert_eq!(busy.raw_os_error(), Some(231), "ERROR_PIPE_BUSY: {busy}");
    core.close(a_id);
    drop(a);
    open_waiting(&n, Duration::from_secs(5)).expect("an instance is free again");
    connected(&rx);
    core.shutdown();
}

#[test]
fn shutdown_closes_all_connections() {
    let n = name();
    let (core, rx) = start(&n, Options::default());
    let mut clients: Vec<_> = (0..3)
        .map(|_| open_waiting(&n, Duration::from_secs(5)).unwrap())
        .collect();
    let ids: Vec<u32> = (0..3).map(|_| connected(&rx).id).collect();
    let (tx, pending) = mpsc::channel();
    let tx2 = tx.clone();
    core.read(ids[0], Box::new(move |r| tx2.send(r).unwrap()));
    core.shutdown();
    // The pending read ends; every client sees the end; the name is free.
    assert_eq!(
        pending
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap(),
        None
    );
    drop(tx);
    for c in &mut clients {
        let mut buf = [0u8; 8];
        let r = c.read(&mut buf);
        assert!(matches!(r, Ok(0)) || r.is_err(), "{r:?}");
    }
    assert_eq!(read(&core, ids[1]), None);
    let (again, _rx) = start(&n, Options::default());
    again.shutdown();
}

#[test]
fn allow_adds_an_appcontainer_sid() {
    let Some(container) = AppContainer::create() else {
        eprintln!("skipped: no AppContainer profile could be created on this host");
        return;
    };
    let n = name();
    // Not allowed: the DACL refuses the container (`ERROR_ACCESS_DENIED`).
    let (core, rx) = start(&n, Options::default());
    assert_eq!(container.probe(&n), CHILD_DENIED);
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    core.shutdown();
    // ALL APPLICATION PACKAGES passes the DACL, but this container's SID is not allowed: the
    // listener refuses it before reading anything.
    let (core, rx) = start(
        &n,
        Options {
            allow: vec!["S-1-15-2-1".into()],
            ..Options::default()
        },
    );
    container.probe(&n);
    match rx.recv_timeout(Duration::from_secs(10)).expect("an event") {
        Event::Refused(r) => {
            assert_eq!(r.reason, "peer");
            assert_eq!(r.app_container.as_deref(), Some(container.sid.as_str()));
        }
        Event::Connected(p) => panic!("accepted {p:?}"),
    }
    // After `allow`, the container connects and is reported as itself.
    core.allow(&container.sid).unwrap();
    assert_eq!(container.probe(&n), CHILD_CONNECTED);
    let peer = connected(&rx);
    assert_eq!(peer.app_container.as_deref(), Some(container.sid.as_str()));
    assert_eq!(peer.sid, core.user_sid());
    core.shutdown();
}

/// Runs inside the AppContainer (`AppContainer::probe`): opens the pipe named by
/// `FIN_PIPE_PROBE` and exits with what happened.
#[test]
#[ignore = "the AppContainer child of allow_adds_an_appcontainer_sid"]
fn appcontainer_child() {
    let Ok(name) = std::env::var("FIN_PIPE_PROBE") else {
        return;
    };
    let code = match open(&name, CLIENT_ACCESS) {
        Ok(mut c) => {
            let _ = c.write_all(b"hi\n");
            CHILD_CONNECTED
        }
        Err(e) if e.raw_os_error() == Some(5) => CHILD_DENIED,
        Err(_) => CHILD_OTHER,
    };
    std::process::exit(code as i32);
}
