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
    let domain = Box::new(Recording {
        inner: ProcessGroups,
        calls: calls.clone(),
    });
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
    assert_eq!(calls.last().map(String::as_str), Some("close_group g1"));
}
