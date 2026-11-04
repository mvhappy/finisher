//! `Store` picks its storage kind at create/open: detected, overridden by
//! `storageOverride` in `finisher.project.json`, or ignoring a bad override.

use fin_core::Rational;
use fin_store::Store;
use fin_store::conflict_names::Provider;
use fin_store::storage_kind::StorageKind;
use serde_json::{Value, json};
use std::path::Path;

fn project_json(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join("finisher.project.json")).unwrap()).unwrap()
}

fn set_override(dir: &Path, value: Option<Value>) {
    let mut v = project_json(dir);
    match value {
        Some(x) => v["storageOverride"] = x,
        None => {
            v.as_object_mut().unwrap().remove("storageOverride");
        }
    }
    std::fs::write(
        dir.join("finisher.project.json"),
        serde_json::to_vec(&v).unwrap(),
    )
    .unwrap();
}

fn make(server: &Path, proj: &Path) {
    Store::create(server, proj, "P", Rational::new(24, 1).unwrap())
        .unwrap()
        .close()
        .unwrap();
}

#[test]
fn detected_then_overridden_then_ignored() {
    let (server, proj) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let created = Store::create(
        server.path(),
        proj.path(),
        "P",
        Rational::new(24, 1).unwrap(),
    )
    .unwrap();
    assert_eq!(created.storage_kind(), StorageKind::Local);
    created.close().unwrap();
    // A new project file has no override and no new fields.
    assert!(project_json(proj.path()).get("storageOverride").is_none());

    let open = || Store::open(server.path(), proj.path()).unwrap();
    assert_eq!(open().storage_kind(), StorageKind::Local);

    set_override(proj.path(), Some(json!("sync:onedrive")));
    let s = open();
    assert_eq!(s.storage_kind(), StorageKind::Sync(Provider::OneDrive));
    assert_eq!(s.storage_warning(), None);

    set_override(proj.path(), Some(json!("network")));
    assert_eq!(open().storage_kind(), StorageKind::Network);

    set_override(proj.path(), Some(json!("floppy")));
    let s = open();
    assert_eq!(s.storage_kind(), StorageKind::Local);
    assert!(s.storage_warning().unwrap().contains("floppy"));

    // A non-string is ignored with a warning too; the project still opens.
    set_override(proj.path(), Some(json!(7)));
    let s = open();
    assert_eq!(s.storage_kind(), StorageKind::Local);
    assert!(s.storage_warning().unwrap().contains("ignored"));

    set_override(proj.path(), None);
    assert_eq!(open().storage_warning(), None);
}

#[test]
fn test_hook_still_replaces_the_detected_kind() {
    let (server, proj) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    make(server.path(), proj.path());
    let mut s = Store::open(server.path(), proj.path()).unwrap();
    s.set_test_storage_kind(StorageKind::Sync(Provider::Dropbox));
    assert_eq!(s.storage_kind(), StorageKind::Sync(Provider::Dropbox));
}

#[test]
fn weakening_a_detected_sync_folder_warns_but_is_honoured() {
    let (server, proj) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    make(server.path(), proj.path());
    std::fs::create_dir(proj.path().join(".stfolder")).unwrap();
    let s = Store::open(server.path(), proj.path()).unwrap();
    assert_eq!(s.storage_kind(), StorageKind::Sync(Provider::Syncthing));
    set_override(proj.path(), Some(json!("local")));
    let s = Store::open(server.path(), proj.path()).unwrap();
    assert_eq!(s.storage_kind(), StorageKind::Local);
    assert!(s.storage_warning().unwrap().contains("sync:syncthing"));
}
