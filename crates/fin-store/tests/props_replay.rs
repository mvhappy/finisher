//! Replay property (spec §4.6 prop 5; plan 4B Task 20, widened by its review): a generated log
//! written through the store, with edits by several authors, branches forked at the head, at a
//! checkpoint or at an earlier seq, archiving and restoring, undos, redos, `undo --to`,
//! checkpoints, merges and cherry-picks in any direction (into the parent, a child or a
//! sibling), reverts, and partial imports by a follower along the way. Replaying each branch
//! from its first snapshot (`apply_committed`) gives its live head; the writer verifies; and the
//! follower, after a last import, has the same heads, branch index, `merged` states and undo
//! stacks, and verifies.
//!
//! The log also carries 4C ops (overwrite inserts, every trim mode, slip, split, ripple and
//! overwrite moves across tracks, slide, links, transitions, params, effects, keyframes,
//! captions, styles) and font pins (catalog and asset, with the same-hash re-pin).
//!
//! 16 cases by default (about 5 s in a debug build); `PROPTEST_CASES=300` for a stress run.

mod integration_support;

use fin_core::{
    AssetKind, CommittedTx, CoreError, Edge, Item, ItemBody, MemIds, Op, ParamValue, Project,
    Rational, SCHEMA_VERSION, Track, TrackKind, Tx, apply_committed,
};
use fin_store::{
    BranchCreate, CheckpointReq, CherryPickReq, HistoryReq, MAIN, MergeReq, Store, StoreError,
};
use integration_support as is;
use proptest::prelude::*;
use rusqlite::{Connection, params};

const AUTHORS: [&str; 3] = ["alice", "bob", "carol"];

/// `PROPTEST_CASES` overrides the per-block default.
fn config(default: u32) -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    ProptestConfig {
        cases,
        ..ProptestConfig::default()
    }
}

fn secs(n: i64) -> Rational {
    Rational::from_int(n)
}

#[derive(Debug, Clone)]
enum Act {
    /// On branch `b` (an index into the branches made so far): an insert before clip `k` (or
    /// at the end), a delete or an end trim of clip `k`.
    Edit {
        b: usize,
        what: u8,
        k: usize,
        k2: usize,
        n: i64,
    },
    /// A branch forked from branch `from`: at its head (`at` 0–1), at checkpoint `k1` (2), or
    /// at `at / 5` of its head seq.
    Branch {
        from: usize,
        at: u8,
    },
    /// Branch `b` archived (`on`) or restored.
    Archive {
        b: usize,
        on: bool,
    },
    /// `undo --to k1` on branch `b`.
    UndoTo {
        b: usize,
    },
    /// A revert of the `k`th merge made so far, on branch `b`.
    Revert {
        b: usize,
        k: usize,
    },
    /// The writer exports and the follower imports what is there.
    Sync,
    Undo {
        b: usize,
    },
    Redo {
        b: usize,
    },
    /// Branch `from` merged into branch `into` (5: its parent).
    Merge {
        from: usize,
        into: usize,
        skip: bool,
    },
    /// The `k`th own record of branch `from` cherry-picked into branch `into` (5: its parent).
    Pick {
        from: usize,
        into: usize,
        k: usize,
    },
    Checkpoint {
        b: usize,
    },
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        9 => (0usize..4, 0u8..WHATS, 0usize..6, 0usize..6, -3i64..4)
            .prop_map(|(b, what, k, k2, n)| Act::Edit { b, what, k, k2, n }),
        2 => (0usize..5, 0u8..6).prop_map(|(from, at)| Act::Branch { from, at }),
        1 => (1usize..5, any::<bool>()).prop_map(|(b, on)| Act::Archive { b, on }),
        1 => (0usize..5).prop_map(|b| Act::UndoTo { b }),
        1 => (0usize..5, 0usize..4).prop_map(|(b, k)| Act::Revert { b, k }),
        1 => Just(Act::Sync),
        3 => (0usize..4).prop_map(|b| Act::Undo { b }),
        2 => (0usize..4).prop_map(|b| Act::Redo { b }),
        3 => (0usize..5, 0usize..6, any::<bool>()).prop_map(|(from, into, skip)| Act::Merge { from, into, skip }),
        2 => (0usize..5, 0usize..6, 0usize..8).prop_map(|(from, into, k)| Act::Pick { from, into, k }),
        1 => (0usize..4).prop_map(|b| Act::Checkpoint { b }),
    ]
}

/// A step may be refused (nothing to undo, a conflict, an archived branch, a name taken, a
/// merge of a branch into itself, …) and the script goes on; only codes that say the store
/// itself is broken fail the case.
const BROKEN: [&str; 4] = ["DB_ERROR", "INTERNAL", "INVALID_FORMAT", "CORRUPT"];

fn allowed<T>(r: Result<T, StoreError>) -> Result<(), TestCaseError> {
    if let Err(e) = r {
        let e = CoreError::from(e);
        prop_assert!(!BROKEN.contains(&e.code.as_str()), "{e:?}");
    }
    Ok(())
}

struct Script {
    s: Store,
    branches: Vec<String>,
    n: usize,
    /// The merges asked for so far (revert targets), with their target branch.
    merges: Vec<(String, String)>,
    /// The op kinds the generated edits committed.
    kinds: std::collections::BTreeSet<String>,
}

impl Script {
    fn id(&mut self) -> String {
        self.n += 1;
        format!("t{}", self.n)
    }

    fn author(&self) -> &'static str {
        AUTHORS[self.n % AUTHORS.len()]
    }

    fn branch(&self, b: usize) -> String {
        self.branches[b % self.branches.len()].clone()
    }

    fn parent(&self, b: &str) -> Option<String> {
        self.s
            .branches(true)
            .unwrap()
            .into_iter()
            .find(|v| v.id == b)
            .and_then(|v| v.parent)
    }

    /// Branch `into` (an index), or `from`'s parent for 5 and up.
    fn target(&self, from: &str, into: usize) -> Option<String> {
        if into >= 5 {
            self.parent(from)
        } else {
            Some(self.branch(into))
        }
    }

    fn history(&mut self, b: &str) -> HistoryReq {
        let tx_id = self.id();
        HistoryReq {
            tx_id,
            author: self.author().into(),
            branch: b.into(),
            dry_run: false,
            args_hash: None,
            to: None,
        }
    }

    fn edit(
        &mut self,
        b: &str,
        what: u8,
        k: usize,
        k2: usize,
        n: i64,
    ) -> Result<(), TestCaseError> {
        let head = self.s.snapshot(b).unwrap().1;
        let Some(op) = realise(&head, what, k, k2, n, self.n) else {
            return Ok(());
        };
        let tx_id = self.id();
        let tx = Tx {
            tx_id,
            author: self.author().into(),
            branch: b.into(),
            base_rev: self.s.head(b),
            schema_version: SCHEMA_VERSION,
            args_hash: None,
            ops: vec![op.clone()],
        };
        // Skipped when it would not apply to the head.
        if fin_core::apply_tx(&head, &tx, &mut MemIds::after(&head)).is_err() {
            return Ok(());
        }
        let r = self.s.commit(tx);
        if r.is_ok() {
            let kind = serde_json::to_value(&op).unwrap()["op"]
                .as_str()
                .unwrap()
                .to_string();
            self.kinds.insert(kind);
        }
        allowed(r)
    }

    fn step(&mut self, act: &Act) -> Result<(), TestCaseError> {
        match *act {
            Act::Edit { b, what, k, k2, n } => {
                let b = self.branch(b);
                self.edit(&b, what, k, k2, n)
            }
            Act::Branch { from, at } => {
                let from = self.branch(from);
                let head = self.s.head(&from);
                let at = match at {
                    0 | 1 => None,
                    2 => Some(fin_store::At::Checkpoint("k1".into())),
                    n => Some(fin_store::At::Seq(head * u64::from(n) / 5)),
                };
                let tx_id = self.id();
                let made = self.s.create_branch(BranchCreate {
                    tx_id,
                    author: self.author().into(),
                    name: None,
                    from,
                    at,
                    dry_run: false,
                    args_hash: None,
                });
                match made {
                    Ok(m) => self.branches.push(m.branch.id),
                    Err(e) => allowed::<()>(Err(e))?,
                }
                Ok(())
            }
            Act::Archive { b, on } => {
                let branch = self.branch(b);
                let tx_id = self.id();
                allowed(self.s.set_branch(fin_store::BranchSet {
                    tx_id,
                    author: self.author().into(),
                    branch,
                    name: None,
                    archived: Some(on),
                    dry_run: false,
                    args_hash: None,
                }))
            }
            Act::UndoTo { b } => {
                let mut r = self.history(&self.branch(b));
                r.to = Some("k1".into());
                allowed(self.s.undo(r))
            }
            Act::Revert { b, k } => {
                if self.merges.is_empty() {
                    return Ok(());
                }
                let (m, _) = self.merges[k % self.merges.len()].clone();
                let branch = self.branch(b);
                let tx_id = self.id();
                allowed(self.s.revert(fin_store::RevertReq {
                    tx_id,
                    author: self.author().into(),
                    merge_id: m,
                    branch,
                    dry_run: false,
                    args_hash: None,
                }))
            }
            Act::Sync => Ok(()),
            Act::Undo { b } => {
                let r = self.history(&self.branch(b));
                allowed(self.s.undo(r))
            }
            Act::Redo { b } => {
                let r = self.history(&self.branch(b));
                allowed(self.s.redo(r))
            }
            Act::Merge { from, into, skip } => {
                let from = self.branch(from);
                let Some(into) = self.target(&from, into) else {
                    return Ok(());
                };
                let tx_id = self.id();
                self.merges.push((tx_id.clone(), into.clone()));
                let r = self.s.merge(MergeReq {
                    tx_id,
                    author: self.author().into(),
                    from,
                    into,
                    skip_conflicting: skip,
                    dry_run: false,
                    args_hash: None,
                });
                allowed(r)
            }
            Act::Pick { from, into, k } => {
                let from = self.branch(from);
                let Some(into) = self.target(&from, into) else {
                    return Ok(());
                };
                let fork = self
                    .s
                    .branches(true)
                    .unwrap()
                    .into_iter()
                    .find(|v| v.id == from)
                    .unwrap()
                    .fork_seq;
                let own: Vec<CommittedTx> = self.s.ops_since(&from, fork).unwrap();
                let Some(origin) = own.get(k % own.len().max(1)) else {
                    return Ok(());
                };
                let tx_id = self.id();
                let r = self.s.cherry_pick(CherryPickReq {
                    tx_id,
                    author: self.author().into(),
                    origin: origin.tx.tx_id.clone(),
                    into,
                    dry_run: false,
                    args_hash: None,
                });
                allowed(r)
            }
            Act::Checkpoint { b } => {
                let branch = self.branch(b);
                let tx_id = self.id();
                let r = self.s.checkpoint(CheckpointReq {
                    tx_id,
                    author: self.author().into(),
                    branch,
                    name: "k1".into(),
                    dry_run: false,
                    args_hash: None,
                });
                allowed(r)
            }
        }
    }
}

/// Number of edit kinds `realise` knows.
const WHATS: u8 = 38;

fn sha(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

fn font_file(c: char) -> fin_core::FontFile {
    fin_core::FontFile {
        sha256: sha(c),
        size: 1234,
        format: fin_core::FontFormat::Ttf,
        faces: vec![fin_core::FontFace {
            index: 0,
            family: "Test Sans".into(),
            weight: 400,
            italic: false,
            scripts: vec!["Latn".into()],
        }],
    }
}

fn font_asset_op(path: &str, c: char) -> Op {
    let mut a = fin_core::AssetAdd::media("$f", path, secs(1), None, None, "fp");
    a.kind = AssetKind::Font;
    a.duration = None;
    a.font = Some(font_file(c));
    a.into()
}

fn asset_pin(asset: &str, c: char) -> fin_core::FontPin {
    fin_core::FontPin {
        family: "Test Sans".into(),
        weight: 400,
        italic: false,
        face: 0,
        sha256: sha(c),
        size: 1234,
        format: fin_core::FontFormat::Ttf,
        scripts: vec!["Latn".into()],
        source: fin_core::FontSource::Asset {
            asset: asset.into(),
        },
    }
}

fn catalog_pin(family: &str, weight: u16, c: char) -> fin_core::FontPin {
    fin_core::FontPin {
        family: family.into(),
        weight,
        italic: false,
        face: 0,
        sha256: sha(c),
        size: 1000,
        format: fin_core::FontFormat::Ttf,
        scripts: vec!["Latn".into(), "Cyrl".into()],
        source: fin_core::FontSource::Catalog {
            entry: family.to_lowercase(),
            style: if weight >= 700 { "bold" } else { "regular" }.into(),
            catalog: "2026.11.1".into(),
        },
    }
}

fn pin_op(p: fin_core::FontPin) -> Op {
    fin_core::FontPinOp {
        key: fin_core::font_key(&p.family, p.weight, p.italic),
        pin: p,
        fallback: None,
    }
    .into()
}

fn caption_item(
    id: &str,
    track: &str,
    at: Rational,
    duration: i64,
    words: &[&str],
    style: &str,
) -> Op {
    fin_core::ItemInsert {
        id: id.into(),
        track: track.into(),
        at,
        duration: secs(duration),
        mode: fin_core::InsertMode::Overwrite,
        body: ItemBody::Caption {
            block: fin_core::CaptionBlock {
                words: words
                    .iter()
                    .enumerate()
                    .map(|(i, t)| fin_core::Word {
                        text: (*t).into(),
                        start: secs(i as i64),
                        end: secs(i as i64 + 1),
                    })
                    .collect(),
                style: style.into(),
                overrides: Default::default(),
                source: None,
            },
        },
    }
    .into()
}

/// The first timeline's tracks of `kind`.
fn tracks(p: &Project, kind: TrackKind) -> Vec<&Track> {
    p.timelines[0]
        .tracks
        .iter()
        .filter(|t| t.kind == kind)
        .collect()
}

/// The track holding `item`, and its index there.
fn place<'a>(p: &'a Project, item: &str) -> Option<(&'a Track, usize)> {
    p.timelines[0]
        .tracks
        .iter()
        .find_map(|t| t.items.iter().position(|i| i.id == item).map(|i| (t, i)))
}

/// One edit drawn from `what` (0..`WHATS`), realised against the branch head `p`; `None` when
/// the head has nothing to apply it to. The caller skips what the engine refuses. `tag` makes
/// names unique.
fn realise(p: &Project, what: u8, k: usize, k2: usize, n: i64, tag: usize) -> Option<Op> {
    use fin_core::{DeleteMode, InsertMode, TrimMode};
    let vt = tracks(p, TrackKind::Video);
    let ct = tracks(p, TrackKind::Caption);
    let clips: Vec<&Item> = vt
        .iter()
        .flat_map(|t| &t.items)
        .filter(|i| matches!(i.body, ItemBody::Clip { .. }))
        .collect();
    let caps: Vec<&Item> = ct.iter().flat_map(|t| &t.items).collect();
    let c = (!clips.is_empty()).then(|| clips[k % clips.len()]);
    let cap = (!caps.is_empty()).then(|| caps[k % caps.len()]);
    let tr = vt.get(k2 % vt.len().max(1)).copied();
    let delta = secs(n);
    let insert = |mode: InsertMode, at: Option<Rational>| -> Option<Op> {
        let tr = tr?;
        let at = at.unwrap_or_else(|| {
            tr.items.get(k % tr.items.len().max(1)).map_or_else(
                || {
                    tr.items
                        .last()
                        .map_or(secs(0), |c| (c.start + c.duration).unwrap())
                },
                |c| c.start,
            )
        });
        Some(
            fin_core::ItemInsert {
                id: "$c".into(),
                track: tr.id.clone(),
                at,
                duration: secs(n.abs() + 1),
                mode,
                body: ItemBody::Clip {
                    asset: "a1".into(),
                    source_in: secs(3),
                    speed: Rational::from_int(1),
                },
            }
            .into(),
        )
    };
    let trim = |mode: TrimMode, edge: Edge| -> Option<Op> {
        (n != 0).then_some(())?;
        Some(
            fin_core::ItemTrim {
                item: c?.id.clone(),
                edge,
                delta,
                mode,
                alone: false,
            }
            .into(),
        )
    };
    let edge = if k2.is_multiple_of(2) {
        Edge::End
    } else {
        Edge::Start
    };
    Some(match what {
        0 | 1 => insert(InsertMode::Ripple, None)?,
        2 => insert(InsertMode::Overwrite, None)?,
        3 => insert(InsertMode::Overwrite, Some(secs(n.abs() * 3)))?,
        4 => fin_core::ItemDelete {
            item: c?.id.clone(),
            mode: DeleteMode::Ripple,
        }
        .into(),
        5 => trim(TrimMode::Ripple, Edge::End)?,
        6 => trim(TrimMode::Ripple, Edge::Start)?,
        7 | 8 => trim(TrimMode::Roll, edge)?,
        9 | 10 => trim(TrimMode::Overwrite, edge)?,
        11 => fin_core::ItemSlip {
            item: c?.id.clone(),
            delta: (n != 0).then_some(delta)?,
        }
        .into(),
        12 => fin_core::ItemSplit {
            item: c?.id.clone(),
            offset: secs(1 + (k2 as i64) % 3),
            id: "$s".into(),
        }
        .into(),
        13..=15 => {
            // No clip means no moving; no video track means no destination.
            c?;
            let to = *vt.get(k2 % vt.len().max(1))?;
            let at = to
                .items
                .get(k % to.items.len().max(1))
                .map_or(secs(0), |i| i.start);
            fin_core::ItemMove {
                item: c?.id.clone(),
                track: to.id.clone(),
                at,
                mode: if what == 15 {
                    InsertMode::Overwrite
                } else {
                    InsertMode::Ripple
                },
            }
            .into()
        }
        16 => fin_core::ItemSlide {
            item: c?.id.clone(),
            delta: (n != 0).then_some(delta)?,
            sides: None,
        }
        .into(),
        17 => {
            let other = clips.get(k2 % clips.len().max(1))?;
            fin_core::ItemLink {
                items: vec![c?.id.clone(), other.id.clone()],
                id: "$lk".into(),
            }
            .into()
        }
        18 => fin_core::ItemUnlink {
            group: c?.common.link.clone()?,
            items: None,
        }
        .into(),
        19 | 20 => {
            let t = vt.iter().find(|t| t.items.len() >= 2)?;
            let i = k % (t.items.len() - 1);
            fin_core::TransitionSet {
                id: "$tr".into(),
                track: t.id.clone(),
                between: [t.items[i].id.clone(), t.items[i + 1].id.clone()],
                kind: fin_core::TransitionKind::Dissolve,
                duration: ["1/2", "1", "3/2"][k2 % 3].parse().unwrap(),
                curve: fin_core::Curve::Linear,
            }
            .into()
        }
        21 => {
            let ts: Vec<_> = vt.iter().flat_map(|t| &t.transitions).collect();
            fin_core::TransitionRemove {
                transition: ts.get(k % ts.len().max(1))?.id.clone(),
            }
            .into()
        }
        22 => fin_core::ParamSet {
            item: c?.id.clone(),
            path: "opacity".into(),
            value: (!k2.is_multiple_of(4))
                .then(|| ParamValue::Number(f64::from((n + 3) as i32) / 7.0)),
        }
        .into(),
        23 => fin_core::EffectAdd {
            item: c?.id.clone(),
            id: "$fx".into(),
            kind: "blur".into(),
            at: None,
            params: Default::default(),
        }
        .into(),
        24..=26 => {
            let withfx: Vec<&&Item> = clips
                .iter()
                .filter(|i| !i.common.effects.is_empty())
                .collect();
            let it = withfx.get(k % withfx.len().max(1))?;
            let fx = &it.common.effects[k2 % it.common.effects.len()];
            if what == 24 {
                fin_core::EffectMove {
                    item: it.id.clone(),
                    effect: fx.id.clone(),
                    to: (k % it.common.effects.len()) as u32,
                }
                .into()
            } else {
                fin_core::EffectRemove {
                    item: it.id.clone(),
                    effect: fx.id.clone(),
                }
                .into()
            }
        }
        27 | 28 => fin_core::KeyframeSet {
            item: c?.id.clone(),
            path: "opacity".into(),
            at: ["0", "1/2", "1"][k2 % 3].parse().unwrap(),
            value: ParamValue::Number(f64::from((n + 3) as i32) / 7.0),
            interp: if n % 2 == 0 {
                fin_core::Interp::Linear
            } else {
                fin_core::Interp::Hold
            },
        }
        .into(),
        29 => {
            let withkf: Vec<&&Item> = clips
                .iter()
                .filter(|i| !i.common.keyframes.is_empty())
                .collect();
            let it = withkf.get(k % withkf.len().max(1))?;
            let (path, list) = it.common.keyframes.iter().next()?;
            fin_core::KeyframeRemove {
                item: it.id.clone(),
                path: path.clone(),
                at: list.get(k2 % list.len().max(1))?.at,
            }
            .into()
        }
        30 => {
            let t = *ct.get(k2 % ct.len().max(1))?;
            let at = t
                .items
                .last()
                .map_or(secs(0), |c| (c.start + c.duration).unwrap());
            let style = p
                .styles
                .keys()
                .nth(k % (p.styles.len() + 1))
                .map_or("preset:bold".to_string(), Clone::clone);
            caption_item(
                "$cap",
                &t.id,
                secs(at.to_f64() as i64 + (n.abs() % 2)),
                3,
                &["x", "y", "z"],
                &style,
            )
        }
        31 => {
            let words = ["alpha beta", "one two three four", "solo", "a b c"];
            fin_core::CaptionSetText {
                item: cap?.id.clone(),
                text: words[(n + 3) as usize % words.len()].into(),
            }
            .into()
        }
        32 => fin_core::CaptionSplit {
            item: cap?.id.clone(),
            word: 1 + (k2 as u32) % 2,
            id: "$cs".into(),
        }
        .into(),
        33 => {
            let cap = cap?;
            let (t, i) = place(p, &cap.id)?;
            fin_core::CaptionMerge {
                item: cap.id.clone(),
                with: t.items.get(i + 1)?.id.clone(),
            }
            .into()
        }
        34 => {
            let style = p
                .styles
                .keys()
                .nth(k2 % (p.styles.len() + 1))
                .map_or("preset:bold".to_string(), Clone::clone);
            fin_core::CaptionSetStyle {
                item: cap?.id.clone(),
                style,
                overrides: None,
            }
            .into()
        }
        // Styles, font pins and font assets.
        35 => fin_core::StyleAdd {
            id: "$st".into(),
            name: format!("Style {tag}"),
            from: None,
            set: Default::default(),
        }
        .into(),
        36 => fin_core::StyleSet {
            style: p.styles.keys().nth(k % p.styles.len().max(1))?.clone(),
            path: "stroke.width".into(),
            value: Some(ParamValue::Number(f64::from((n + 4) as i32))),
        }
        .into(),
        _ => {
            // 37: pins of every flavour, picked by `k`.
            let fonts: Vec<(&String, &fin_core::Asset)> = p
                .assets
                .iter()
                .filter(|(_, a)| a.kind == AssetKind::Font)
                .collect();
            match k % 4 {
                0 => pin_op(catalog_pin(
                    ["Inter", "Lora"][k2 % 2],
                    [400, 700][(n.unsigned_abs() as usize) % 2],
                    ['1', '2'][k2 % 2],
                )),
                1 => font_asset_op(&format!("/fonts/f{tag}.ttf"), 'c'),
                2 => {
                    let (id, _) = fonts.first()?;
                    pin_op(asset_pin(id, 'c'))
                }
                _ => {
                    // The same pin again: a no-op.
                    let pin = p.fonts.pins.values().nth(k2 % p.fonts.pins.len().max(1))?;
                    pin_op(pin.clone())
                }
            }
        }
    })
}

/// A store with the genesis the property needs: a media asset, three clips on `t1`, a second
/// video track, a caption track with two captions, a style, a font asset and font pins.
fn setup(e: &is::Env) -> Store {
    let mut s = e.create("a");
    is::commit(&mut s, "s1", vec![is::asset("/media/a.mov")]);
    let mut clips: Vec<Op> = (0..3)
        .map(|i| {
            fin_core::ItemInsert {
                id: format!("$c{i}"),
                track: "t1".into(),
                at: secs(10 * i),
                duration: secs(10),
                mode: fin_core::InsertMode::Ripple,
                body: ItemBody::Clip {
                    asset: "a1".into(),
                    source_in: secs(5),
                    speed: Rational::from_int(1),
                },
            }
            .into()
        })
        .collect();
    clips.push(
        fin_core::ItemLink {
            items: vec!["$c0".into(), "$c1".into()],
            id: "$lk".into(),
        }
        .into(),
    );
    is::commit(&mut s, "s2", clips);
    let add = |id: &str, kind| -> Op {
        fin_core::TrackAdd {
            id: id.into(),
            timeline: "tl1".into(),
            kind,
            index: None,
            name: None,
        }
        .into()
    };
    is::commit(
        &mut s,
        "s3",
        vec![add("$v2", TrackKind::Video), add("$cp", TrackKind::Caption)],
    );
    let head = s.snapshot(MAIN).unwrap().1;
    let cap = tracks(&head, TrackKind::Caption)[0].id.clone();
    let first = tracks(&head, TrackKind::Video)[0].items[2].id.clone();
    is::commit(
        &mut s,
        "s4",
        vec![
            caption_item(
                "$k1",
                &cap,
                secs(0),
                4,
                &["one", "two", "three"],
                "preset:bold",
            ),
            caption_item("$k2", &cap, secs(5), 3, &["four", "five"], "preset:bold"),
            font_asset_op("/fonts/base.ttf", 'c'),
            pin_op(catalog_pin("Inter", 700, '1')),
            fin_core::StyleAdd {
                id: "$st".into(),
                name: "Base".into(),
                from: None,
                set: Default::default(),
            }
            .into(),
            fin_core::EffectAdd {
                item: first.clone(),
                id: "$fx1".into(),
                kind: "blur".into(),
                at: None,
                params: Default::default(),
            }
            .into(),
            fin_core::EffectAdd {
                item: first.clone(),
                id: "$fx2".into(),
                kind: "blur".into(),
                at: None,
                params: Default::default(),
            }
            .into(),
            fin_core::KeyframeSet {
                item: first,
                path: "opacity".into(),
                at: secs(0),
                value: ParamValue::Number(0.5),
                interp: fin_core::Interp::Linear,
            }
            .into(),
        ],
    );
    let head = s.snapshot(MAIN).unwrap().1;
    let font = head
        .assets
        .iter()
        .find(|(_, a)| a.kind == AssetKind::Font)
        .unwrap()
        .0
        .clone();
    is::commit(&mut s, "s5", vec![pin_op(asset_pin(&font, 'c'))]);
    s
}

/// Every branch with its index row and its head (rev and project).
fn heads(s: &Store) -> Vec<(fin_store::BranchView, u64, Project)> {
    s.branches(true)
        .unwrap()
        .into_iter()
        .map(|b| {
            let (rev, p) = s.snapshot(&b.id).unwrap();
            (b, rev, p)
        })
        .collect()
}

/// Each branch replayed from its first snapshot equals its head.
fn assert_replays(s: &Store) -> Result<(), TestCaseError> {
    let conn = Connection::open(s.db_path()).unwrap();
    for b in s.branches(true).unwrap() {
        let (seq, json): (i64, String) = conn
            .query_row(
                "SELECT seq, json FROM snapshots WHERE branch = ?1 ORDER BY seq LIMIT 1",
                params![b.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let mut p: Project = serde_json::from_str(&json).unwrap();
        for c in s.ops_since(&b.id, u64::try_from(seq).unwrap()).unwrap() {
            p = apply_committed(&p, &c.tx, &c.bindings)
                .unwrap_or_else(|e| panic!("{} {}: {e:?}", b.id, c.tx.tx_id))
                .project;
        }
        prop_assert_eq!(&p, &s.snapshot(&b.id).unwrap().1, "replay of {}", b.id);
    }
    Ok(())
}

proptest! {
    #![proptest_config(config(16))]
    #[test]
    fn replay_and_export_import_roundtrip(
        mut acts in prop::collection::vec(act(), 4..26),
        at in any::<prop::sample::Index>(),
        cp in 0usize..4,
    ) {
        // Every log has a branch, a checkpoint somewhere and a merge of the first branch into
        // its parent at the end.
        acts.insert(0, Act::Branch { from: 0, at: 0 });
        acts.insert(1 + at.index(acts.len()), Act::Checkpoint { b: cp });
        acts.push(Act::Merge { from: 1, into: 5, skip: true });
        let e = is::env();
        let s = setup(&e);
        let mut run = Script {
            s,
            branches: vec![MAIN.to_string()],
            n: 0,
            merges: vec![],
            kinds: Default::default(),
        };
        // The follower, opened at the first partial import.
        let mut f0: Option<Store> = None;
        for act in &acts {
            run.step(act)?;
            if matches!(act, Act::Sync) {
                e.flush(&mut run.s);
                let f = f0.get_or_insert_with(|| e.open("f"));
                e.poll(f);
            }
        }
        let mut s = run.s;
        assert_replays(&s)?;
        let v = s.verify();
        prop_assert!(v.ok, "{v:?}");

        e.flush(&mut s);
        let mut f = f0.unwrap_or_else(|| e.open("f"));
        e.poll(&mut f);
        prop_assert_eq!(heads(&f), heads(&s));
        assert_replays(&f)?;
        for b in s.branches(true).unwrap() {
            for a in AUTHORS {
                prop_assert_eq!(
                    f.undo_stacks(&b.id, a).unwrap(),
                    s.undo_stacks(&b.id, a).unwrap(),
                    "stacks of {} on {}", a, b.id
                );
            }
        }
        let v = f.verify();
        prop_assert!(v.ok, "{v:?}");
    }
}

/// Op kinds the generated edits must reach (checked over a fixed-seed run).
const KINDS: &[&str] = &[
    "item.insert",
    "item.delete",
    "item.trim",
    "item.slip",
    "item.split",
    "item.move",
    "item.slide",
    "item.link",
    "item.unlink",
    "transition.set",
    "transition.remove",
    "param.set",
    "effect.add",
    "effect.move",
    "effect.remove",
    "keyframe.set",
    "keyframe.remove",
    "caption.setText",
    "caption.split",
    "caption.merge",
    "caption.setStyle",
    "style.add",
    "style.set",
    "font.pin",
    "asset.add",
];

/// The generator reaches the 4C ops, font pins and a font asset: a fixed-seed run of 32
/// scripts of edits through the store commits every listed kind, and the genesis itself pins
/// fonts from a catalog and from an asset.
#[test]
fn generator_reaches_4c_ops_font_pins_and_font_asset() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::{Config, TestRunner};
    let mut runner = TestRunner::new_with_rng(
        Config::default(),
        proptest::test_runner::TestRng::from_seed(
            proptest::test_runner::RngAlgorithm::ChaCha,
            &[7; 32],
        ),
    );
    let strat = prop::collection::vec(act(), 24..40);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..32 {
        let acts = strat.new_tree(&mut runner).unwrap().current();
        let e = is::env();
        let s = setup(&e);
        let head = s.snapshot(MAIN).unwrap().1;
        assert!(head.fonts.pins.len() >= 2, "{:?}", head.fonts);
        assert!(head.assets.values().any(|a| a.kind == AssetKind::Font));
        let mut run = Script {
            s,
            branches: vec![MAIN.to_string()],
            n: 0,
            merges: vec![],
            kinds: Default::default(),
        };
        for act in &acts {
            if matches!(act, Act::Edit { .. }) {
                run.step(act).unwrap();
            }
        }
        seen.extend(run.kinds);
    }
    for kind in KINDS {
        assert!(seen.contains(*kind), "never generated {kind}; saw {seen:?}");
    }
}

/// Regression: with no clips (or no video track) the move and link arms indexed an empty
/// collection (`k2 % 0`) instead of yielding no op.
#[test]
fn realise_without_clips_or_video_tracks_yields_no_op() {
    let e = is::env();
    let s = setup(&e);
    let mut p = s.snapshot(MAIN).unwrap().1;
    p.timelines[0]
        .tracks
        .iter_mut()
        .for_each(|t| t.items.clear());
    for what in [13, 14, 15, 17] {
        assert!(
            realise(&p, what, 3, 5, 1, 0).is_none(),
            "empty items, arm {what}"
        );
    }
    p.timelines[0].tracks.clear();
    for what in [13, 14, 15, 17] {
        assert!(
            realise(&p, what, 3, 5, 1, 0).is_none(),
            "no tracks, arm {what}"
        );
    }
}
