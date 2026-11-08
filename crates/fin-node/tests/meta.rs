//! A call's `metaJson` (`{actor?, guard?}`): the write guard is read strictly, the actor as
//! before. The crate is a napi `cdylib` with nothing to link natively, so this includes the
//! napi-free `src/journal/ctx.rs`, as the `commit_overhead` example does.

#![allow(dead_code)]

#[path = "../src/journal/ctx.rs"]
mod ctx;

use ctx::{actor_of_meta, guard_of_meta};
use fin_store::WriteGuard;

#[test]
fn no_meta_or_no_guard_is_the_default_guard() {
    assert_eq!(guard_of_meta(None).unwrap(), WriteGuard::default());
    let meta = r#"{"actor":{"kind":"agent"}}"#;
    assert_eq!(guard_of_meta(Some(meta)).unwrap(), WriteGuard::default());
}

#[test]
fn guard_decodes_beside_the_actor() {
    let meta = r#"{"actor":{"kind":"agent","session":"s1"},"guard":{"denyBranches":["main"],"allowBranches":["br2"]}}"#;
    assert_eq!(
        guard_of_meta(Some(meta)).unwrap(),
        WriteGuard {
            deny_branches: vec!["main".into()],
            allow_branches: Some(vec!["br2".into()]),
        }
    );
    // The actor is read as before.
    let with = format!("{:?}", actor_of_meta(Some(meta)));
    let without = format!(
        "{:?}",
        actor_of_meta(Some(r#"{"actor":{"kind":"agent","session":"s1"}}"#))
    );
    assert_eq!(with, without);
    assert!(with.contains("agent"), "{with}");
}

#[test]
fn malformed_guard_is_refused() {
    for meta in [
        r#"{"guard":{"allowBranches":"br2"}}"#,
        r#"{"guard":{"deny":["main"]}}"#,
        r#"{"guard":null}"#,
        r#"{"guard":["main"]}"#,
        r#"["guard"]"#,
        "not json",
    ] {
        let e = guard_of_meta(Some(meta)).expect_err(meta);
        assert_eq!(e.code, "INVALID_OP", "{meta}");
        assert_eq!(e.details["field"], "metaJson.guard", "{meta}");
        assert!(e.fix.is_some(), "{meta}");
    }
}
