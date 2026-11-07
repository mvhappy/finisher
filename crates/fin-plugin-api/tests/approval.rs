//! `requires_reapproval`: what an update must not change without a person (spec §4.3, §5.3).

mod support;

use fin_plugin_api::{Approval, Manifest, Reapproval, reapproval_reasons, requires_reapproval};
use serde_json::{Value, json};
use support::fixture;

fn manifest(v: Value) -> Manifest {
    serde_json::from_value(v).unwrap()
}

fn reasons(
    old: &Value,
    old_sha: Option<&str>,
    new: &Value,
    new_sha: Option<&str>,
) -> Vec<Reapproval> {
    let (o, n) = (manifest(old.clone()), manifest(new.clone()));
    let r = reapproval_reasons(
        &Approval {
            manifest: &o,
            agent_content_sha256: old_sha,
        },
        &Approval {
            manifest: &n,
            agent_content_sha256: new_sha,
        },
    );
    assert_eq!(
        requires_reapproval(
            &Approval {
                manifest: &o,
                agent_content_sha256: old_sha
            },
            &Approval {
                manifest: &n,
                agent_content_sha256: new_sha
            }
        ),
        !r.is_empty()
    );
    r
}

fn bumped(mut v: Value) -> Value {
    v["version"] = "9.9.9".into();
    v
}

#[test]
fn a_plain_update_needs_no_approval() {
    for f in ["content-pack", "code-node", "code-native"] {
        let old = fixture(f);
        assert_eq!(reasons(&old, None, &bumped(old.clone()), None), [], "{f}");
    }
}

#[test]
fn wider_permissions() {
    let old = fixture("code-node");
    let mut new = bumped(old.clone());
    new["permissions"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "perm": "gpu" }));
    assert_eq!(reasons(&old, None, &new, None), [Reapproval::Permissions]);
    // Narrowing needs none.
    assert_eq!(reasons(&new, None, &old, None), []);
}

#[test]
fn agent_facing_content() {
    let old = fixture("content-pack");
    let mut skills = bumped(old.clone());
    skills["contributes"]["skills"] = json!([{ "name": "cut", "file": "skills/cut.md" }]);
    // Added agent-facing content, then any change of it, needs a person.
    assert_eq!(
        reasons(&old, None, &skills, Some("aa")),
        [Reapproval::AgentContent]
    );
    assert_eq!(
        reasons(&skills, Some("aa"), &skills, Some("bb")),
        [Reapproval::AgentContent]
    );
    assert_eq!(
        reasons(&skills, Some("aa"), &skills, None),
        [Reapproval::AgentContent]
    );
    assert_eq!(reasons(&skills, Some("aa"), &skills, Some("aa")), []);
    // Removing it does not.
    assert_eq!(reasons(&skills, Some("aa"), &bumped(old), None), []);
}

#[test]
fn kind_and_runtime() {
    let node = fixture("code-node");
    let native = fixture("code-native");
    let mut moved = bumped(node.clone());
    moved["runtime"] = native["runtime"].clone();
    moved["platforms"] = native["platforms"].clone();
    assert_eq!(reasons(&node, None, &moved, None), [Reapproval::Runtime]);
    // Another entry file of the same runtime type is no new approval.
    let mut main = bumped(node.clone());
    main["runtime"]["main"] = "dist/other.mjs".into();
    assert_eq!(reasons(&node, None, &main, None), []);
    // content → code: another kind and a runtime where there was none.
    let mut code = fixture("content-pack");
    code["kind"] = "code".into();
    code["runtime"] = node["runtime"].clone();
    code["platforms"] = node["platforms"].clone();
    assert_eq!(
        reasons(&fixture("content-pack"), None, &code, None),
        [Reapproval::Kind, Reapproval::Runtime]
    );
}
