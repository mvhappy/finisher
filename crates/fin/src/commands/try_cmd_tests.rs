//! `fin try keep|drop` against a fake server that lets another fin select a default branch in
//! the middle of the command (review #50).

use std::path::{Path, PathBuf};

use fin_core::{CommittedTx, Rational, Tx};

use super::*;
use crate::backend::{Probed, Snapshot, TxLookup};

/// Branches `main` and the try `br1` (made from `main`); `br2` exists too. When `tag` is
/// called, another fin makes `br2` the default first.
struct Interleaved {
    sd: PathBuf,
    tag: &'static str,
}

impl Interleaved {
    fn other_fin_selects_br2(&self) {
        config::set_default_branch(&self.sd, "p", Some("br2")).unwrap();
    }
}

fn view(id: &str, parent: Option<&str>) -> Value {
    json!({ "id": id, "name": id, "display": id, "state": "active", "parent": parent })
}

impl Backend for Interleaved {
    fn config_scope(&self) -> Result<Option<(PathBuf, String)>, CoreError> {
        Ok(Some((self.sd.clone(), "p".into())))
    }
    fn project_call(&self, tag: &str, _payload: Value) -> Result<Value, CoreError> {
        if tag == self.tag {
            self.other_fin_selects_br2();
        }
        Ok(match tag {
            "branch.list" => json!({ "branches": [
                view("main", None), view("br1", Some("main")), view("br2", Some("main")),
            ] }),
            "branch.set" => json!({ "rev": 3, "tx": { "txId": "t" } }),
            other => unreachable!("{other}"),
        })
    }
    fn snapshot(&self, _branch: &str) -> Result<Snapshot, CoreError> {
        unreachable!()
    }
    fn commit(&self, _tx: &Tx, _generation: Option<&Value>) -> Result<CommittedTx, CoreError> {
        unreachable!()
    }
    fn tx_get(&self, _tx_id: &str) -> Result<TxLookup, CoreError> {
        unreachable!()
    }
    fn rebuild(&self) -> Result<(Value, bool), CoreError> {
        unreachable!()
    }
    fn probe(&self, _path: &Path) -> Result<Probed, CoreError> {
        unreachable!()
    }
    fn create_project(
        &self,
        _dir: &Path,
        _name: &str,
        _rate: Rational,
        _idem: Option<&str>,
    ) -> Result<Value, CoreError> {
        unreachable!()
    }
    fn open_project(&self, _dir: &Path, _relocate: bool) -> Result<Value, CoreError> {
        unreachable!()
    }
}

fn globals() -> Globals {
    Globals::default()
}

#[test]
fn a_drop_keeps_a_default_selected_while_it_ran() {
    let dir = tempfile::tempdir().unwrap();
    let sd = dir.path().to_path_buf();
    config::start_try(&sd, "p", "br1", "main").unwrap();
    let backend = Interleaved {
        sd: sd.clone(),
        tag: "branch.set",
    };
    let doc = finish(&backend, &globals(), false).unwrap();
    assert_eq!(doc["branch"]["state"], "archived", "{doc}");
    // The answer says where the default is, not where it would have gone.
    assert_eq!(doc["switchedTo"], "br2", "{doc}");
    assert_eq!(doc["defaultKept"], true, "{doc}");
    assert_eq!(
        config::default_branch(&sd, "p").unwrap().as_deref(),
        Some("br2"),
        "the other fin's choice stands"
    );
    assert_eq!(config::try_parent(&sd, "p", "br1").unwrap(), None);
}

#[test]
fn a_drop_without_interleaving_switches_back() {
    let dir = tempfile::tempdir().unwrap();
    let sd = dir.path().to_path_buf();
    config::start_try(&sd, "p", "br1", "main").unwrap();
    let backend = Interleaved {
        sd: sd.clone(),
        tag: "none",
    };
    let doc = finish(&backend, &globals(), false).unwrap();
    assert_eq!(doc["switchedTo"], "main", "{doc}");
    assert!(doc.get("defaultKept").is_none(), "{doc}");
    assert_eq!(config::default_branch(&sd, "p").unwrap(), None);
    assert_eq!(config::try_parent(&sd, "p", "br1").unwrap(), None);
}

#[test]
fn clearing_a_stale_default_keeps_one_selected_meanwhile() {
    let dir = tempfile::tempdir().unwrap();
    let sd = dir.path().to_path_buf();
    config::start_try(&sd, "p", "gone", "main").unwrap();
    let backend = Interleaved {
        sd: sd.clone(),
        tag: "branch.list",
    };
    let doc = finish(&backend, &globals(), false).unwrap();
    assert_eq!(doc["cleared"], "gone", "{doc}");
    assert_eq!(doc["switchedTo"], "br2", "{doc}");
    assert_eq!(doc["defaultKept"], true, "{doc}");
    assert_eq!(
        config::default_branch(&sd, "p").unwrap().as_deref(),
        Some("br2")
    );
    assert_eq!(config::try_parent(&sd, "p", "gone").unwrap(), None);
}
