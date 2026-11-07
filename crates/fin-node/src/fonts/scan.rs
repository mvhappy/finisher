//! What a project's branches pin (4C Decisions 29 and 37): the files whose bytes its
//! captions measure with, the font asset files the provider may read, and what its captions
//! need that nothing pins (Decision 35's `FONT_UNPINNED`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use fin_core::layout::faces_for;
use fin_core::{AssetKind, CommittedTx, EntityKind, FontSource, ItemBody, Op, Project};
use fin_store::Store;

/// One pinned file: its key, its SHA-256, and for an asset pin the asset's file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Need {
    pub(crate) key: String,
    pub(crate) sha256: String,
    pub(crate) asset: Option<PathBuf>,
}

/// What one branch head pins, the font assets it holds (SHA-256 → file), and the keys its
/// captions need with no pin (a face key or `script:<code>`, [`faces_for`]) → those captions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BranchFonts {
    pub(crate) needs: BTreeSet<Need>,
    pub(crate) assets: BTreeSet<(String, PathBuf)>,
    pub(crate) unpinned: BTreeMap<String, BTreeSet<String>>,
}

/// The keys `project`'s captions need that no pin provides, each with its captions.
fn unpinned(project: &Project) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for tl in &project.timelines {
        for it in tl.tracks.iter().flat_map(|t| t.items.iter()) {
            let ItemBody::Caption { block } = &it.body else {
                continue;
            };
            let Ok(style) = fin_core::captions::resolve_style(project, block) else {
                continue;
            };
            if let Err(keys) = faces_for(project, &style, &block.words) {
                for k in keys {
                    out.entry(k).or_default().insert(it.id.clone());
                }
            }
        }
    }
    out
}

/// An asset path as stored (absolute), or relative to the project folder.
fn resolve(project_dir: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        project_dir.join(p)
    }
}

pub(crate) fn of_project(project: &Project, project_dir: &Path) -> BranchFonts {
    let file_of = |asset: &str| {
        project
            .assets
            .get(asset)
            .map(|a| resolve(project_dir, &a.path))
    };
    let needs = project
        .fonts
        .pins
        .iter()
        .map(|(key, pin)| Need {
            key: key.clone(),
            sha256: pin.sha256.clone(),
            asset: match &pin.source {
                FontSource::Asset { asset } => file_of(asset),
                FontSource::Catalog { .. } => None,
            },
        })
        .collect();
    let assets = project
        .assets
        .values()
        .filter(|a| a.kind == AssetKind::Font)
        .filter_map(|a| {
            Some((
                a.font.as_ref()?.sha256.clone(),
                resolve(project_dir, &a.path),
            ))
        })
        .collect();
    BranchFonts {
        needs,
        assets,
        unpinned: unpinned(project),
    }
}

/// Every unarchived branch's head (a branch that cannot be read is left out).
pub(crate) fn store(store: &Store) -> BTreeMap<String, BranchFonts> {
    let Ok(branches) = store.branches(false) else {
        return BTreeMap::new();
    };
    branches
        .into_iter()
        .filter_map(|b| {
            let (_, project) = store.snapshot(&b.id).ok()?;
            Some((b.id, of_project(&project, store.project_dir())))
        })
        .collect()
}

/// One branch's head now (`None` when it cannot be read).
pub(crate) fn branch(store: &Store, branch: &str) -> Option<BranchFonts> {
    let (_, project) = store.snapshot(branch).ok()?;
    Some(of_project(&project, store.project_dir()))
}

/// Whether a record may change what its branch pins or what its captions need: a font op
/// (`font.pin|unpin|repin`), an asset, style or caption op, a caption insert or restore, or
/// a diff that changes a pin (`font/<key>`), an asset or a style.
pub(crate) fn touches(c: &CommittedTx) -> bool {
    let caption = |op: &Op| match op {
        Op::ItemInsert(i) => matches!(i.body, ItemBody::Caption { .. }),
        Op::ItemRestore(r) => matches!(r.item.body, ItemBody::Caption { .. }),
        _ => false,
    };
    c.tx.ops.iter().any(|op| {
        let name = op.name();
        ["font.", "asset.", "style.", "caption."]
            .iter()
            .any(|p| name.starts_with(p))
            || caption(op)
    }) || c.diff.changes.iter().any(|ch| {
        matches!(
            ch.kind,
            EntityKind::Asset | EntityKind::Font | EntityKind::Style
        )
    })
}
