//! Risks (spec §4.3, 8A Decision 5) and permission diffs ("widening").

use fin_plugin_api::{
    FsAccess, FsScope, Origin, Permission, Risk, SecretMode, diff, overall, risks,
};

fn net(origins: &[&str]) -> Permission {
    Permission::Net {
        origins: origins.iter().map(|o| Origin(o.to_string())).collect(),
    }
}

fn observe(hooks: &[&str]) -> Permission {
    Permission::HooksObserve {
        hooks: hooks.iter().map(|h| h.to_string()).collect(),
    }
}

fn secrets(names: &[&str], mode: SecretMode) -> Permission {
    Permission::Secrets {
        names: names.iter().map(|n| n.to_string()).collect(),
        mode,
    }
}

fn fs(access: FsAccess) -> Permission {
    Permission::Fs {
        scope: FsScope::PluginData,
        access,
    }
}

fn risk_in(p: &Permission, all: &[Permission]) -> Risk {
    risks(all).into_iter().find(|(q, _)| q == p).unwrap().1
}

#[test]
fn risk_combinations() {
    let o = observe(&["PostCommand"]);
    assert_eq!(risk_in(&o, std::slice::from_ref(&o)), Risk::Medium);
    let with_net = [o.clone(), net(&["https://api.example.org"])];
    assert_eq!(risk_in(&o, &with_net), Risk::High);
    assert_eq!(overall(std::slice::from_ref(&o), false), Risk::Medium);
    assert_eq!(overall(&with_net, false), Risk::High);
    for (p, want) in [
        (Permission::ProjectRead, Risk::Low),
        (Permission::MediaRead, Risk::Low),
        (fs(FsAccess::Read), Risk::Low),
        (Permission::ProjectWrite, Risk::Medium),
        (Permission::JobsSubmit, Risk::Medium),
        (fs(FsAccess::ReadWrite), Risk::Medium),
        (Permission::Gpu, Risk::Medium),
        (net(&["https://api.example.org"]), Risk::High),
        (secrets(&["k"], SecretMode::Brokered), Risk::High),
        (Permission::MediaFile, Risk::High),
        (
            Permission::HooksModify {
                hooks: vec!["PreCommand".into()],
            },
            Risk::High,
        ),
    ] {
        assert_eq!(risk_in(&p, std::slice::from_ref(&p)), want, "{}", p.key());
    }
    assert_eq!(overall(&[], false), Risk::Low);
    assert_eq!(
        overall(&[], true),
        Risk::Medium,
        "agent-facing content is medium"
    );
    assert_eq!(overall(&[Permission::ProjectRead], true), Risk::Medium);
    assert_eq!(overall(&[Permission::MediaFile], true), Risk::High);
}

#[test]
fn permission_diff_widening_cases() {
    let base = vec![
        Permission::ProjectRead,
        net(&["https://a.example.org", "https://b.example.org"]),
        secrets(&["k1"], SecretMode::Brokered),
        fs(FsAccess::Read),
        observe(&["PostCommand"]),
    ];
    // Reordering, at both levels, is no change.
    let reordered = vec![
        observe(&["PostCommand"]),
        fs(FsAccess::Read),
        net(&["https://b.example.org", "https://a.example.org"]),
        secrets(&["k1"], SecretMode::Brokered),
        Permission::ProjectRead,
    ];
    let d = diff(&base, &reordered);
    assert!(d.added.is_empty() && d.widened.is_empty() && d.removed.is_empty() && !d.risk_up);
    assert!(!d.permissions_widen());

    let widen = |i: usize, p: Permission| {
        let mut new = base.clone();
        new[i] = p;
        let d = diff(&base, &new);
        assert_eq!(d.widened.len(), 1, "{new:?}");
        assert!(d.added.is_empty() && d.removed.is_empty());
        assert_eq!(d.widened[0].from, base[i]);
        assert_eq!(d.widened[0].to, new[i]);
        assert!(d.permissions_widen());
    };
    widen(
        1,
        net(&[
            "https://a.example.org",
            "https://b.example.org",
            "https://c.example.org",
        ]),
    );
    widen(1, net(&["https://a.example.org", "https://c.example.org"]));
    widen(2, secrets(&["k1", "k2"], SecretMode::Brokered));
    widen(2, secrets(&["k1"], SecretMode::Raw));
    widen(3, fs(FsAccess::ReadWrite));
    widen(4, observe(&["PostCommand", "PreCommand"]));

    // An added permission.
    let mut new = base.clone();
    new.push(Permission::Gpu);
    let d = diff(&base, &new);
    assert_eq!(d.added, [Permission::Gpu]);
    assert!(
        d.permissions_widen() && !d.risk_up,
        "gpu is medium; base is already high"
    );

    // Narrowing and removal need no approval.
    let narrowed = vec![
        net(&["https://a.example.org"]),
        secrets(&["k1"], SecretMode::Brokered),
        fs(FsAccess::Read),
    ];
    let d = diff(&base, &narrowed);
    assert!(!d.permissions_widen(), "{d:?}");
    assert_eq!(
        d.removed,
        [observe(&["PostCommand"]), Permission::ProjectRead]
    );
    let d = diff(
        &[secrets(&["k1"], SecretMode::Raw)],
        &[secrets(&["k1"], SecretMode::Brokered)],
    );
    assert!(!d.permissions_widen());

    // A risk increase through a combination: adding net to a plugin that observes hooks.
    let old = [observe(&["PostCommand"])];
    let new = [observe(&["PostCommand"]), net(&["https://a.example.org"])];
    let d = diff(&old, &new);
    assert!(d.risk_up && d.permissions_widen());
    assert_eq!(d.added, [net(&["https://a.example.org"])]);
    // The reverse lowers the risk.
    assert!(!diff(&new, &old).permissions_widen());
}
