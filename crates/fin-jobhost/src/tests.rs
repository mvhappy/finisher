//! The host's calls into its kill domain, in order, over the recording double (Unix:
//! `/bin/sh` stands in for a worker; it never says `hello`).

use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::domain::double::Recording;
use crate::unix::pgroup::ProcessGroups;
use crate::*;

fn sh(script: &str) -> SpawnRequest {
    SpawnRequest {
        role: Role::Worker,
        profile: Profile::Plugin,
        family: "sh".into(),
        program: Program::Path("/bin/sh".into()),
        args: vec!["-c".into(), script.into()],
        env: Vec::new(),
        cwd: std::env::temp_dir(),
        grants: Vec::new(),
        qos: Qos::Normal,
        label: "sh".into(),
    }
}

fn wait_for(rx: &mpsc::Receiver<HostEvent>, mut pred: impl FnMut(&HostEvent) -> bool) -> HostEvent {
    loop {
        let e = rx.recv_timeout(Duration::from_secs(10)).expect("an event");
        if pred(&e) {
            return e;
        }
    }
}

#[test]
fn domain_is_driven_in_order() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let opts = HostOptions::new(std::env::temp_dir());
    let domain = Box::new(Recording::new(ProcessGroups::default(), calls.clone()));
    let (sink, rx) = EventSink::channel(64);
    let host = Host::start_with(opts, sink, domain).unwrap();
    let g = host.open_group(GroupSpec {
        label: "t".into(),
        memory_bytes: None,
        qos: Qos::Normal,
    });
    assert!(
        calls.lock().unwrap().is_empty(),
        "open_group only allocates"
    );
    let (tx, prx) = mpsc::channel();
    for _ in 0..2 {
        let tx = tx.clone();
        host.spawn(
            g,
            sh("exit 3"),
            Box::new(move |r| {
                let _ = tx.send(r.unwrap());
            }),
        );
    }
    let p1 = prx.recv().unwrap();
    let p2 = prx.recv().unwrap();
    // The two exits may come in either order; a GroupEmpty follows the last.
    let mut left = vec![p1, p2];
    while !left.is_empty() {
        if let HostEvent::Exit { proc, info } =
            wait_for(&rx, |e| matches!(e, HostEvent::Exit { .. }))
        {
            assert_eq!((info.reason, info.code), (ExitReason::Exited, Some(3)));
            left.retain(|p| *p != proc);
        }
    }
    wait_for(
        &rx,
        |e| matches!(e, HostEvent::GroupEmpty { group } if *group == g),
    );
    host.close_group(g);
    let (dtx, drx) = mpsc::channel();
    // A kill after the close is a no-op that still answers.
    host.kill_group(
        g,
        KillReason::Test,
        Box::new(move || {
            let _ = dtx.send(());
        }),
    );
    drx.recv_timeout(Duration::from_secs(5)).unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while !calls.lock().unwrap().iter().any(|c| c == "close_group g1") {
        assert!(std::time::Instant::now() < until, "the group is released");
        std::thread::sleep(Duration::from_millis(10));
    }
    let calls = calls.lock().unwrap().clone();
    let first: Vec<&str> = calls.iter().take(4).map(String::as_str).collect();
    assert_eq!(
        first,
        ["open_group g1", "open_leaf g1", "place g1", "placed g1"]
    );
    assert_eq!(calls[4..6], ["open_leaf g1", "place g1"]);
    assert_eq!(
        calls.iter().filter(|c| c.starts_with("exit_facts")).count(),
        2
    );
    // Each process's leaf is released, then the group.
    assert_eq!(
        calls[calls.len() - 3..],
        ["close_leaf g1", "close_leaf g1", "close_group g1"]
    );
}

fn test_host(domain: Recording<ProcessGroups>) -> (Host, mpsc::Receiver<HostEvent>) {
    let (sink, rx) = EventSink::channel(64);
    let host = Host::start_with(
        HostOptions::new(std::env::temp_dir()),
        sink,
        Box::new(domain),
    );
    (host.unwrap(), rx)
}

fn group(host: &Host) -> GroupId {
    host.open_group(GroupSpec {
        label: "t".into(),
        memory_bytes: None,
        qos: Qos::Normal,
    })
}

fn spawn(host: &Host, g: GroupId, req: SpawnRequest) -> Result<ProcId, HostError> {
    let (tx, rx) = mpsc::channel();
    host.spawn(
        g,
        req,
        Box::new(move |r| {
            let _ = tx.send(r);
        }),
    );
    rx.recv_timeout(Duration::from_secs(5)).unwrap()
}

/// A failed first registration kills the process's own group and leaves no pgid behind: the
/// next spawn makes a fresh group.
#[test]
fn failed_first_registration_leaves_no_stale_pgid() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let d = Recording::new(ProcessGroups::default(), calls);
    d.fail_placed.store(1, std::sync::atomic::Ordering::Release);
    let (host, _rx) = test_host(d);
    let g = group(&host);
    assert!(matches!(
        spawn(&host, g, sh("sleep 5")),
        Err(HostError::Spawn(_))
    ));
    let p = spawn(&host, g, sh("sleep 5")).expect("the second spawn works");
    let pid = host.inner.proc(p).unwrap().pid as i32;
    // SAFETY: getpgid only reads.
    assert_eq!(unsafe { libc::getpgid(pid) }, pid, "a group of its own");
}

/// A failed spawn settles its group like an exit: `GroupEmpty`, then the release.
#[test]
fn failed_spawn_settles_the_group() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (host, rx) = test_host(Recording::new(ProcessGroups::default(), calls.clone()));
    let g = group(&host);
    let mut req = sh("exit 0");
    req.program = Program::Path("/nonexistent/fin-test".into());
    assert!(spawn(&host, g, req).is_err());
    wait_for(
        &rx,
        |e| matches!(e, HostEvent::GroupEmpty { group } if *group == g),
    );
}

/// A panicking `kill_group` callback skips no release: the group is still closed and the
/// shutdown still ends.
#[test]
fn panicking_callback_skips_nothing() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (host, rx) = test_host(Recording::new(ProcessGroups::default(), calls.clone()));
    let g = group(&host);
    spawn(&host, g, sh("sleep 30")).unwrap();
    host.kill_group(g, KillReason::Test, Box::new(|| panic!("a caller's bug")));
    wait_for(
        &rx,
        |e| matches!(e, HostEvent::GroupEmpty { group } if *group == g),
    );
    host.close_group(g);
    let (tx, drx) = mpsc::channel();
    host.shutdown(
        0,
        Box::new(move || {
            let _ = tx.send(());
        }),
    );
    drx.recv_timeout(Duration::from_secs(5))
        .expect("the shutdown ends");
    assert!(calls.lock().unwrap().iter().any(|c| c == "close_group g1"));
}
