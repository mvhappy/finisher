//! `SandboxPolicy::validate` and the reference model (plan 7C Task 1).

use std::path::PathBuf;

use fin_sandbox::{
    DeviceAccess, DeviceRule, Limits, Net, PolicyError, Profile, ReadPolicy, SandboxPolicy,
};

fn abs(rel: &str) -> PathBuf {
    let mut p = PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" });
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(part);
    }
    p
}

fn base(profile: Profile) -> SandboxPolicy {
    SandboxPolicy {
        profile,
        write: vec![abs("w")],
        read: ReadPolicy::Only(vec![abs("r")]),
        protect: vec![abs("w/locked")],
        deny_read: vec![abs("r/secret")],
        unix_sockets: vec![],
        net: if profile == Profile::Agent {
            Net::Open { loopback: vec![] }
        } else {
            Net::None
        },
        mach: vec![],
        devices: vec![],
        limits: if profile == Profile::Worker {
            Limits {
                processes: Some(1),
                ..Limits::default()
            }
        } else {
            Limits::default()
        },
    }
}

#[test]
fn validate_refuses_relative_unnormalised_and_overlapping() {
    for profile in Profile::ALL {
        base(profile).validate().unwrap();
    }

    let mut p = base(Profile::Hook);
    p.write.push(PathBuf::from("relative/dir"));
    assert!(
        matches!(p.validate(), Err(PolicyError::NotAbsolute { field, .. }) if field == "write")
    );

    if !cfg!(windows) {
        for bad in ["/r/../etc", "/r/./x", "/r/x/", "/r//x"] {
            let mut p = base(Profile::Hook);
            p.read = ReadPolicy::Only(vec![PathBuf::from(bad)]);
            assert!(
                matches!(p.validate(), Err(PolicyError::NotNormalised { field, .. }) if field == "read"),
                "{bad}"
            );
        }
    }

    // A write root under, or equal to, a protected root.
    for w in ["w/locked/inner", "w/locked"] {
        let mut p = base(Profile::Hook);
        p.write.push(abs(w));
        assert_eq!(
            p.validate(),
            Err(PolicyError::WriteUnderProtect {
                write: abs(w),
                protect: abs("w/locked")
            }),
            "{w}"
        );
    }

    // A path both granted and denied.
    let mut p = base(Profile::Hook);
    p.read = ReadPolicy::Only(vec![abs("r"), abs("r/secret")]);
    assert!(
        matches!(p.validate(), Err(PolicyError::GrantedAndDenied { field, .. }) if field == "read")
    );
    let mut p = base(Profile::Hook);
    p.deny_read.push(abs("w"));
    assert!(
        matches!(p.validate(), Err(PolicyError::GrantedAndDenied { field, .. }) if field == "write")
    );
}

#[test]
fn validate_holds_each_profile_to_its_rules() {
    let refused = |p: SandboxPolicy| {
        assert!(
            matches!(p.validate(), Err(PolicyError::Profile { .. })),
            "{p:?}"
        )
    };

    for profile in [Profile::Hook, Profile::Worker, Profile::Plugin] {
        let mut p = base(profile);
        p.net = Net::Open { loopback: vec![] };
        refused(p);
    }
    for profile in [Profile::Agent, Profile::Worker, Profile::Plugin] {
        let mut p = base(profile);
        p.net = Net::Loopback { ports: vec![4000] };
        refused(p);
    }
    let mut p = base(Profile::Hook);
    p.net = Net::Loopback { ports: vec![] };
    refused(p);
    let mut p = base(Profile::Agent);
    p.net = Net::Open { loopback: vec![0] };
    refused(p);
    for profile in [Profile::Worker, Profile::Plugin] {
        let mut p = base(profile);
        p.unix_sockets = vec![abs("run/fin.sock")];
        refused(p);
    }
    for profile in [Profile::Agent, Profile::Hook] {
        let mut p = base(profile);
        p.devices = vec![DeviceRule {
            path: abs("dev/dri/renderD128"),
            access: DeviceAccess::ReadWrite,
        }];
        refused(p);
    }
    let mut p = base(Profile::Worker);
    p.limits.processes = Some(2);
    refused(p);
    let mut p = base(Profile::Plugin);
    p.limits.open_files = Some(0);
    refused(p);
    let mut p = base(Profile::Agent);
    p.mach = vec!["com.apple.trustd\") (allow default".into()];
    refused(p);

    let mut p = base(Profile::Hook);
    p.net = Net::Loopback { ports: vec![4000] };
    p.unix_sockets = vec![abs("run/fin.sock")];
    p.validate().unwrap();
}

#[test]
fn the_most_specific_root_wins_and_a_tie_denies() {
    let p = SandboxPolicy {
        profile: Profile::Hook,
        write: vec![abs("data/session/tmp"), abs("w")],
        read: ReadPolicy::Only(vec![abs("data/session/credential.json"), abs("p")]),
        protect: vec![abs("w/locked")],
        deny_read: vec![abs("data"), abs("p/hidden")],
        ..base(Profile::Hook)
    };
    p.validate().unwrap();
    // Holes inside a denied root.
    assert!(p.readable(&abs("data/session/credential.json")));
    assert!(!p.writable(&abs("data/session/credential.json")));
    assert!(p.writable(&abs("data/session/tmp/x")));
    assert!(!p.readable(&abs("data/auth/local.key")));
    assert!(!p.readable(&abs("data/session/blobs/x")));
    // A denial inside a read root.
    assert!(p.readable(&abs("p/a")));
    assert!(!p.readable(&abs("p/hidden/a")));
    // A protected root inside a write root: neither writable nor (outside the read roots) readable.
    assert!(p.writable(&abs("w/x")));
    assert!(!p.writable(&abs("w/locked/x")));
    assert!(!p.readable(&abs("w/locked/x")));
    // Nothing else.
    assert!(!p.readable(&abs("")));
    assert!(!p.readable(&abs("etc/passwd")));
}

#[test]
fn everything_is_for_tests_and_still_denies() {
    let p = SandboxPolicy {
        read: ReadPolicy::Everything {
            except: vec![abs("home/.ssh")],
        },
        ..base(Profile::Hook)
    };
    p.validate().unwrap();
    assert!(p.readable(&abs("etc/passwd")));
    assert!(!p.readable(&abs("home/.ssh/id")));
    assert!(!p.readable(&abs("r/secret/x")));
}

#[test]
fn the_policy_round_trips_as_json_and_refuses_unknown_fields() {
    let mut p = base(Profile::Agent);
    p.net = Net::Open {
        loopback: vec![41234],
    };
    p.mach = vec!["com.apple.trustd".into()];
    let json = serde_json::to_value(&p).unwrap();
    assert_eq!(
        json["net"],
        serde_json::json!({"kind": "open", "loopback": [41234]})
    );
    assert_eq!(json["read"]["only"][0], serde_json::json!(abs("r")));
    assert!(json.get("denyRead").is_some() && json.get("unixSockets").is_some());
    let back: SandboxPolicy = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(back, p);

    let mut extra = json;
    extra["readEverything"] = serde_json::json!(true);
    assert!(serde_json::from_value::<SandboxPolicy>(extra).is_err());
}
