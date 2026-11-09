//! Mechanism status (plan 7C Task 3): `status` per profile from the mechanism and `READY`,
//! and the `FIN_TEST_SANDBOX_STATUS` stub that the F2 tests elsewhere rely on.

use fin_sandbox::status::{DisabledReason, Mechanism, READY, status_with};
use fin_sandbox::{Os, Profile};

fn enforced() -> Mechanism {
    Mechanism::Enforced {
        detail: "landlock abi 8".into(),
    }
}

fn disabled(reason: DisabledReason) -> Mechanism {
    Mechanism::Disabled { reason }
}

#[test]
fn a_disabled_mechanism_wins_over_readiness() {
    let ready = [(Profile::Worker, Os::current())];
    for p in Profile::ALL {
        assert_eq!(
            status_with(p, disabled(DisabledReason::LandlockDisabled), &ready),
            disabled(DisabledReason::LandlockDisabled),
            "{p:?}"
        );
    }
}

#[test]
fn a_profile_not_ready_runs_as_before_7c() {
    let ready = [(Profile::Worker, Os::current())];
    let unavailable = Mechanism::Unavailable {
        need: "kernel ≥ 6.2".into(),
    };
    // Ready: the mechanism's answer, including a refusal.
    assert_eq!(status_with(Profile::Worker, enforced(), &ready), enforced());
    assert_eq!(
        status_with(Profile::Worker, unavailable.clone(), &ready),
        unavailable
    );
    // Not ready (another profile, or another OS): unconfined with a warning, never refused.
    for m in [enforced(), unavailable] {
        assert_eq!(
            status_with(Profile::Agent, m.clone(), &ready),
            disabled(DisabledReason::ProfileNotReady)
        );
        assert_eq!(
            status_with(Profile::Plugin, m, &[]),
            disabled(DisabledReason::ProfileNotReady)
        );
    }
    // Not ready, `worker` runs as before 7C too, whatever the mechanism (plan 7C Task 7: the
    // job host's spawner is its only requester and passes it through until it is ready).
    let unavailable = Mechanism::Unavailable {
        need: "kernel ≥ 6.2".into(),
    };
    for m in [enforced(), unavailable] {
        assert_eq!(
            status_with(Profile::Worker, m, &[]),
            disabled(DisabledReason::ProfileNotReady)
        );
    }
}

/// `status` and the job host's spawner answer alike for every mechanism and `READY` table,
/// so `server.status`'s warnings say what the spawner does.
#[test]
fn status_and_the_spawner_agree() {
    use fin_jobhost::ProfileState;
    use fin_sandbox::spawner::{Decision, decide};
    let mechanisms = [
        enforced(),
        disabled(DisabledReason::LandlockDisabled),
        Mechanism::Unavailable { need: "k".into() },
    ];
    for profile in [Profile::Worker, Profile::Plugin] {
        for ready in [vec![], vec![(profile, Os::current())]] {
            for m in &mechanisms {
                let st = status_with(profile, m.clone(), &ready);
                let d = decide(profile, m, &ready);
                let want = match st {
                    Mechanism::Enforced { .. } => ProfileState::Enforced,
                    Mechanism::Disabled { .. } => ProfileState::Disabled,
                    Mechanism::Unavailable { .. } => ProfileState::Unavailable,
                };
                let got = match d {
                    Decision::Apply => ProfileState::Enforced,
                    Decision::Pass => ProfileState::Disabled,
                    Decision::Refuse(_) => ProfileState::Unavailable,
                };
                assert_eq!(got, want, "{profile:?} {m:?} ready={}", !ready.is_empty());
            }
        }
    }
}

#[test]
fn ready_is_empty_until_the_suites_pass() {
    // Task 16 adds rows as each OS's escape suite passes on the validation hosts.
    assert!(READY.is_empty(), "{READY:?}");
}

#[test]
fn the_status_crosses_as_tagged_json() {
    let v = serde_json::to_value(disabled(DisabledReason::LandlockDisabled)).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"kind": "disabled", "reason": "landlockDisabled"})
    );
    let v = serde_json::to_value(Mechanism::Unavailable { need: "x".into() }).unwrap();
    assert_eq!(v, serde_json::json!({"kind": "unavailable", "need": "x"}));
}

#[cfg(target_os = "linux")]
mod linux_support;

/// The stub replaces the mechanism, and `self_apply` follows it: `disabled` applies nothing
/// (the attempt succeeds, unconfined), `unavailable` refuses (fail closed).
#[cfg(target_os = "linux")]
#[test]
fn the_stub_drives_self_apply() {
    use fin_jobs::sandbox::SelfSandbox;
    use fin_sandbox::{WorkerInput, WorkerScope};
    use linux_support::{Outcome, json_file, probe_env, s, tempdir, write};

    let (_d, t) = tempdir();
    write(&t.join("srv/auth/local.key"), "k");
    for d in ["srv/jobs/j1/in", "srv/jobs/j1/out", "srv/jobs/j1/tmp"] {
        std::fs::create_dir_all(t.join(d)).unwrap();
    }
    let input = WorkerInput {
        tmp: t.join("srv/jobs/j1/tmp"),
        caches: vec![],
        devices: vec![],
        server_data: t.join("srv"),
        scope: WorkerScope::Cold {
            job_dir: t.join("srv/jobs/j1"),
            sources: vec![],
        },
    };
    let req = SelfSandbox {
        profile: "worker".into(),
        args: serde_json::to_value(&input).unwrap(),
    };
    let flags = ["--self".to_string(), json_file(&t, "self.json", &req)];
    let op = ["read".to_string(), s(&t.join("srv/auth/local.key"))];
    let stub = fin_sandbox::status::TEST_ENV;
    assert_eq!(
        probe_env(&flags, &op, &[(stub, "disabled")]),
        Outcome::Ran(None)
    );
    match probe_env(&flags, &op, &[(stub, "unavailable")]) {
        Outcome::Apply(e) => assert!(e.contains("kernel ≥ 6.2"), "{e}"),
        other => panic!("{other:?}"),
    }
}
