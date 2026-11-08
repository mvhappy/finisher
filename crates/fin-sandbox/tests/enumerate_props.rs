//! Root enumeration against the reference model, over random trees (plan 7C Decision 2).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use fin_sandbox::enumerate::{Kind, Tree, enumerate, read_roots, write_roots};
use fin_sandbox::{Limits, Net, Profile, ReadPolicy, SandboxPolicy};
use proptest::prelude::*;

const NAMES: [&str; 3] = ["a", "b", "c"];

fn top() -> PathBuf {
    PathBuf::from(if cfg!(windows) { "C:\\t" } else { "/t" })
}

fn path_of(ix: &[usize]) -> PathBuf {
    let mut p = top();
    for &i in ix {
        p.push(NAMES[i]);
    }
    p
}

/// Every path of depth 0..=3 under `top()`, existing in a tree or not.
fn candidates() -> Vec<PathBuf> {
    let mut out = vec![top()];
    let mut layer: Vec<Vec<usize>> = vec![vec![]];
    for _ in 0..3 {
        layer = layer
            .iter()
            .flat_map(|p| (0..3).map(move |i| [p.clone(), vec![i]].concat()))
            .collect();
        out.extend(layer.iter().map(|ix| path_of(ix)));
    }
    out
}

struct Model {
    nodes: BTreeMap<PathBuf, Kind>,
    reversed: bool,
}

impl Tree for Model {
    fn children(&self, dir: &Path) -> io::Result<Vec<(PathBuf, Kind)>> {
        match self.nodes.get(dir) {
            None | Some(Kind::Link | Kind::Other) => Err(io::ErrorKind::NotFound.into()),
            Some(Kind::File) => Err(io::ErrorKind::NotADirectory.into()),
            Some(Kind::Dir) => {
                let mut kids: Vec<_> = self
                    .nodes
                    .iter()
                    .filter(|(p, _)| p.parent() == Some(dir))
                    .map(|(p, k)| (p.clone(), *k))
                    .collect();
                if self.reversed {
                    kids.reverse();
                }
                Ok(kids)
            }
        }
    }
}

fn kind(n: u8) -> Kind {
    match n % 8 {
        0..=3 => Kind::Dir,
        4 | 5 => Kind::File,
        6 => Kind::Link,
        _ => Kind::Other,
    }
}

/// A tree whose every node's parent is a directory.
fn tree(spec: &[(Vec<usize>, u8)], reversed: bool) -> Model {
    let mut nodes = BTreeMap::from([(top(), Kind::Dir)]);
    let mut spec = spec.to_vec();
    spec.sort_by_key(|(ix, _)| ix.len());
    for (ix, k) in spec {
        let p = path_of(&ix);
        if nodes.get(p.parent().unwrap()) == Some(&Kind::Dir) {
            nodes.entry(p).or_insert(kind(k));
        }
    }
    Model { nodes, reversed }
}

fn covered(roots: &[PathBuf], p: &Path) -> bool {
    roots.iter().any(|r| p.starts_with(r))
}

fn pick(c: &[PathBuf], ix: &[usize]) -> Vec<PathBuf> {
    ix.iter().map(|&i| c[i % c.len()].clone()).collect()
}

fn idx() -> impl Strategy<Value = Vec<usize>> {
    prop::collection::vec(0usize..40, 0..4)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn enumerate_props(
        spec in prop::collection::vec((prop::collection::vec(0usize..3, 1..=3), any::<u8>()), 0..30),
        write in idx(), read in idx(), protect in idx(), deny in idx(),
    ) {
        let c = candidates();
        let (protect, deny) = (pick(&c, &protect), pick(&c, &deny));
        // Only well-formed policies: no write root under a protected root, no root also denied.
        let write: Vec<_> = pick(&c, &write).into_iter().filter(|w| !protect.iter().any(|p| w.starts_with(p)) && !deny.contains(w)).collect();
        let read: Vec<_> = pick(&c, &read).into_iter().filter(|r| !deny.contains(r)).collect();
        let policy = SandboxPolicy {
            profile: Profile::Hook,
            write,
            read: ReadPolicy::Only(read),
            protect,
            deny_read: deny,
            unix_sockets: vec![],
            net: Net::None,
            mach: vec![],
            devices: vec![],
            limits: Limits::default(),
        };
        policy.validate().unwrap();
        let t = tree(&spec, false);
        let e = enumerate(&policy, &t).unwrap();
        let mut excl = policy.protect.clone();
        excl.extend(policy.deny_read.iter().cloned());

        // No enumerated root contains an excluded path.
        for r in &e.write {
            prop_assert!(!excl.iter().any(|x| x.starts_with(r)), "write root {r:?} holds an exclusion");
        }
        for r in &e.read {
            prop_assert!(!policy.deny_read.iter().any(|x| x.starts_with(r)), "read root {r:?} holds a denial");
        }
        // Sound everywhere (an existing path or not), exact on every existing file and directory.
        for p in &c {
            let w = covered(&e.write, p);
            let r = w || covered(&e.read, p);
            prop_assert!(!w || policy.writable(p), "{p:?} granted for write, the model refuses");
            prop_assert!(!r || policy.readable(p), "{p:?} granted for read, the model refuses");
            if matches!(t.nodes.get(p), Some(Kind::Dir | Kind::File)) {
                prop_assert_eq!(w, policy.writable(p), "write {:?}", p);
                prop_assert_eq!(r, policy.readable(p), "read {:?}", p);
            }
        }
        // A link or a special file met while expanding is never granted itself.
        for r in e.write.iter().chain(&e.read) {
            let granted = policy.write.contains(r) || policy.read_roots().contains(r);
            prop_assert!(granted || !matches!(t.nodes.get(r), Some(Kind::Link | Kind::Other)), "{r:?}");
        }
        // Deterministic, sorted and minimal, whatever order directories list in.
        prop_assert_eq!(&e, &enumerate(&policy, &tree(&spec, true)).unwrap());
        for roots in [&e.write, &e.read] {
            prop_assert!(roots.windows(2).all(|w| w[0] < w[1]));
            prop_assert!(!roots.iter().any(|a| roots.iter().any(|b| a != b && a.starts_with(b))));
        }
    }
}

#[test]
fn a_hole_inside_a_denied_root_is_kept_and_expanded_around_its_own_denials() {
    let t = tree(
        &[
            (vec![0], 0),
            (vec![0, 1], 0),
            (vec![0, 1, 2], 4),
            (vec![0, 1, 0], 0),
            (vec![1], 4),
        ],
        false,
    );
    // read /t, deny /t/a, read /t/a/b (a hole), deny /t/a/b/a.
    let roots = read_roots(
        &[top(), path_of(&[0, 1])],
        &[path_of(&[0]), path_of(&[0, 1, 0])],
        &t,
    )
    .unwrap();
    assert_eq!(roots, vec![path_of(&[0, 1, 2]), path_of(&[1])]);
}

#[test]
fn a_missing_protected_path_leaves_its_parent_ungranted() {
    let t = tree(&[(vec![0], 0), (vec![0, 1], 4)], false);
    let roots = write_roots(&[top()], &[path_of(&[0, 2])], &[], &t).unwrap();
    // `/t/a` is expanded (so `/t/a/c` cannot be created), its file `b` is granted.
    assert_eq!(roots, vec![path_of(&[0, 1])]);
}

#[test]
fn a_listing_failure_refuses_the_policy() {
    struct Broken;
    impl Tree for Broken {
        fn children(&self, _: &Path) -> io::Result<Vec<(PathBuf, Kind)>> {
            Err(io::ErrorKind::PermissionDenied.into())
        }
    }
    let err = write_roots(&[top()], &[path_of(&[0])], &[], &Broken).unwrap_err();
    assert!(
        matches!(err, fin_sandbox::PolicyError::Enumerate { .. }),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn the_filesystem_lister_never_grants_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("keep")).unwrap();
    std::fs::create_dir(root.join(".finisher")).unwrap();
    std::fs::write(root.join("file"), b"x").unwrap();
    std::os::unix::fs::symlink("/", root.join("link")).unwrap();
    let roots = write_roots(
        std::slice::from_ref(&root),
        &[root.join(".finisher")],
        &[],
        &fin_sandbox::enumerate::Fs,
    )
    .unwrap();
    assert_eq!(roots, vec![root.join("file"), root.join("keep")]);
}
