//! Root enumeration (plan 7C Decision 2): turns a policy's roots and exclusions into roots
//! that contain no exclusion, for mechanisms that can only add access (Landlock, AppContainer
//! ACLs).
//!
//! A granted root with an excluded path strictly inside it is replaced by its children minus
//! the excluded ones, recursively, down to the excluded path. A root inside an excluded root
//! is kept (a hole: the most specific root wins, [`crate::policy`]). The result is the
//! [`SandboxPolicy::readable`] and [`SandboxPolicy::writable`] model exactly, for every path
//! that exists when the enumeration runs (`tests/enumerate_props.rs`), with three rules that
//! keep it closed:
//! - a symbolic link, junction, socket, FIFO or device met while expanding is never granted
//!   (granting it would grant whatever it points at; following it is checked at its target);
//! - an expanded directory is not granted itself (the model's "a directory holding an
//!   exclusion is not readable or writable as a whole"), so nothing can be created, removed or
//!   renamed directly in it, and an excluded path that does not exist yet cannot be created;
//! - an entry created after the enumeration inside an expanded directory is not granted.
//!
//! The output is sorted, without duplicates and without a root inside another.

use std::io;
use std::path::{Path, PathBuf};

use crate::error::PolicyError;
use crate::policy::SandboxPolicy;

/// What a directory entry is, without following links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    /// A symbolic link or (Windows) a junction or other name-surrogate reparse point.
    Link,
    /// A socket, FIFO or device node.
    Other,
}

/// Lists directories. [`Fs`] reads the real filesystem; tests pass a model tree.
pub trait Tree {
    /// The direct children of `dir` (full paths) and their kinds, in any order.
    fn children(&self, dir: &Path) -> io::Result<Vec<(PathBuf, Kind)>>;
}

/// The real filesystem (links are not followed when classifying entries).
pub struct Fs;

impl Tree for Fs {
    fn children(&self, dir: &Path) -> io::Result<Vec<(PathBuf, Kind)>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let t = entry.file_type()?;
            let kind = if t.is_symlink() {
                Kind::Link
            } else if t.is_dir() {
                Kind::Dir
            } else if t.is_file() {
                Kind::File
            } else {
                Kind::Other
            };
            out.push((entry.path(), kind));
        }
        Ok(out)
    }
}

/// The enumerated roots of a policy: grant read on `read`, read and write on `write`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enumerated {
    pub read: Vec<PathBuf>,
    pub write: Vec<PathBuf>,
}

/// Validates `policy` and enumerates its roots.
pub fn enumerate(policy: &SandboxPolicy, tree: &dyn Tree) -> Result<Enumerated, PolicyError> {
    policy.validate()?;
    let denied = policy.denied();
    Ok(Enumerated {
        read: read_roots(&policy.read_roots(), &denied, tree)?,
        write: write_roots(&policy.write, &policy.protect, &denied, tree)?,
    })
}

/// Write roots minus protected and denied paths.
pub fn write_roots(
    write: &[PathBuf],
    protect: &[PathBuf],
    deny: &[PathBuf],
    tree: &dyn Tree,
) -> Result<Vec<PathBuf>, PolicyError> {
    let mut excl = protect.to_vec();
    excl.extend_from_slice(deny);
    roots(write, &excl, tree)
}

/// Read roots minus denied paths.
pub fn read_roots(
    read: &[PathBuf],
    deny: &[PathBuf],
    tree: &dyn Tree,
) -> Result<Vec<PathBuf>, PolicyError> {
    roots(read, deny, tree)
}

fn roots(
    grants: &[PathBuf],
    excl: &[PathBuf],
    tree: &dyn Tree,
) -> Result<Vec<PathBuf>, PolicyError> {
    let mut out = Vec::new();
    for g in grants {
        // On a tie the exclusion wins; an exclusion strictly above `g` makes `g` a hole.
        if excl.contains(g) {
            continue;
        }
        let inside: Vec<&Path> = excl
            .iter()
            .filter(|e| e.starts_with(g) && *e != g)
            .map(PathBuf::as_path)
            .collect();
        expand(g, &inside, tree, &mut out)?;
    }
    Ok(minimal(out))
}

fn expand(
    dir: &Path,
    inside: &[&Path],
    tree: &dyn Tree,
    out: &mut Vec<PathBuf>,
) -> Result<(), PolicyError> {
    if inside.is_empty() {
        out.push(dir.to_path_buf());
        return Ok(());
    }
    let mut kids = match tree.children(dir) {
        Ok(k) => k,
        // Nothing under a missing directory can be granted, nor created (its parent is not). A
        // file with an excluded path "inside" it is not granted either, as the model says.
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(());
        }
        Err(e) => {
            return Err(PolicyError::Enumerate {
                path: dir.to_path_buf(),
                error: e.to_string(),
            });
        }
    };
    kids.sort_by(|a, b| a.0.cmp(&b.0));
    for (child, kind) in kids {
        if child.parent() != Some(dir) || inside.contains(&child.as_path()) {
            continue;
        }
        let deeper: Vec<&Path> = inside
            .iter()
            .copied()
            .filter(|e| e.starts_with(&child))
            .collect();
        match kind {
            Kind::Dir => expand(&child, &deeper, tree, out)?,
            Kind::File if deeper.is_empty() => out.push(child),
            Kind::File | Kind::Link | Kind::Other => {}
        }
    }
    Ok(())
}

/// Sorted, deduplicated, and without a root inside another (an emitted root holds no
/// exclusion, so a root inside it adds nothing).
fn minimal(mut roots: Vec<PathBuf>) -> Vec<PathBuf> {
    roots.sort();
    roots.dedup();
    let mut out: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for r in roots {
        if !out.iter().any(|o| r.starts_with(o)) {
            out.push(r);
        }
    }
    out
}
