//! Building a project database from the project folder (sub-project 4 spec §3.8).
//!
//! # When
//!
//! - **Missing:** a server with no database for a project whose file carries its [`Genesis`]
//!   (`Store::open`).
//! - **Corrupt:** the file at our own database path is damaged ([`check::damaged`]: a bad
//!   header, an unreadable schema or `meta` under our `application_id`, or a failing
//!   `PRAGMA quick_check`). A file that is someone else's (a foreign `application_id`,
//!   `NOT_A_PROJECT_DB`) or newer (`DB_TOO_NEW`) is never rebuilt over.
//! - **Manual:** [`Store::rebuild`], or [`rebuild_project`] without an open store
//!   (`fin project rebuild`).
//!
//! # How ([`rebuild`]), holding `<project-data>/db.lock` exclusively
//!
//! Every open store holds that lock shared (`crate::dblock`), so no other store has the file
//! open while it is replaced: a manual rebuild is refused with `DB_IN_USE`, an open's
//! automatic rebuild waits briefly for a racing open first.
//! 1. Re-check under the lock: another open may have rebuilt meanwhile (the database exists,
//!    or is no longer damaged); then its database is used.
//! 2. The rev-0 state: the old database's rev-0 snapshot when it reads (the history was built
//!    on it), else `new_project` of the file's genesis; neither is `DB_CORRUPT`. Every
//!    segment in the folder names the genesis it was written under (`genesisHash`): one that
//!    does not match the rev 0 used is `GENESIS_MISMATCH`, and nothing is built. A file
//!    genesis that does not describe a readable old rev 0 is reported
//!    ([`RebuildReport::genesis_mismatch`]).
//! 3. Build `project.fdb.rebuild-<ulid>` beside the database: the schema with a fresh
//!    incarnation (`crate::history`), rev 0, then one import pass of the whole folder
//!    ([`Store::import`], through a store on the new file that holds no lease), offered the
//!    folder's snapshots as seeds: one seeds only if its `txHash` is on the resolved chain and
//!    its `genesisHash` matches (`import::apply`, step 3; others are ignored). An import error
//!    or a fork stops the branch where the folder does, as on any open.
//! 4. Salvage ([`salvage`]) from the old database, from every kept damaged copy
//!    (`project.fdb.corrupt-*`) that the old database does not list as salvaged already
//!    (`meta` `salvagedFrom`; a crash between the swap's two renames leaves the old database
//!    only there), and from unfinished builds. Every record the new database lacks goes to
//!    quarantine (`SALVAGED`, or `MISSING_FROM_FOLDER` for exported ones). Whatever could not
//!    be read is reported ([`RebuildReport::salvage_complete`], status `SALVAGE_INCOMPLETE`).
//! 5. A manual rebuild of a database the folder no longer fully holds (it exported records
//!    the new database lacks) is refused, `REBUILD_WOULD_LOSE`: nothing is swapped.
//! 6. Verify the new database with the checks that need no replay (`crate::verify`): the
//!    import just replayed everything a seed did not cover. Mismatches are reported, never
//!    fatal.
//! 7. Swap ([`files::swap`]): the old database is renamed to `project.fdb.corrupt-<UTC ts>`
//!    with its `-wal` and `-shm` (so SQLite never replays that WAL onto the new file, and the
//!    kept copy still opens with it); without an old database an orphan `-wal` is renamed to
//!    `project.fdb.orphan-<ts>-wal`. The new file is then linked into place without replacing
//!    anything; if that fails, the old file is moved back. Then unfinished builds are
//!    removed and the newest 3 damaged copies kept.
//!
//! A crash at any step leaves the old database in place, or the new one, or neither with
//! the old one kept as `corrupt-*`: the next open rebuilds again and salvages that copy.
//! Two opens racing share the first one's database.
//!
//! Long-running (it imports the whole folder): `fin-node` runs it, and [`Store::verify`], on
//! the per-engine worker thread (plan Decision 7, Task 14).

mod check;
mod files;
mod salvage;
mod seeds;
mod store;

use std::path::{Path, PathBuf};
use std::time::Duration;

use fin_core::{CoreError, Project, new_project};
use serde::Serialize;
use serde_json::json;

pub(crate) use check::damaged;
#[cfg(any(test, feature = "test-hooks"))]
pub use files::fail_next_rebuild_swap_for_test;
pub use store::rebuild_project;

use crate::open::OpenOptions;
use crate::project_file::{Genesis, ProjectFile, genesis_hash, genesis_of};
use crate::storage_kind::StorageKind;
use crate::verify::VerifyReport;
use crate::{Head, MAIN, ServerIdentity, Store, StoreError, copies, db, dblock, export, history};

/// How long an open's automatic rebuild waits for another store to let go of the database.
const OPEN_WAITS: Duration = Duration::from_secs(30);

/// What a rebuild did.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RebuildReport {
    /// `missing`, `corrupt` or `manual`.
    pub reason: String,
    /// What made the old database count as damaged (`corrupt`).
    pub problem: Option<String>,
    /// Records in the new database.
    pub records: u64,
    /// Rows salvaged into quarantine: records the new database lacks (`SALVAGED`,
    /// `MISSING_FROM_FOLDER`) and the old databases' own quarantine rows.
    pub salvaged: u32,
    /// Of those, exported records the folder no longer yields (`MISSING_FROM_FOLDER`).
    pub missing_from_folder: u32,
    /// The head of `main` in the new database.
    pub rebuilt_through: u64,
    /// Everything in the old databases could be read.
    pub salvage_complete: bool,
    /// What could not be read.
    pub salvage_errors: Vec<String>,
    /// The seq of the folder snapshot the rebuild was seeded from.
    pub seeded_from_snapshot: Option<u64>,
    /// Where the old database was kept (`project.fdb.corrupt-<ts>`).
    pub corrupt_copy: Option<PathBuf>,
    /// Earlier damaged copies salvaged too.
    pub salvaged_copies: Vec<String>,
    /// The project file's genesis does not describe the old database's rev 0 (that rev 0
    /// was used).
    pub genesis_mismatch: bool,
    /// Why the import stopped early (it stopped where any open would).
    pub import_error: Option<CoreError>,
    /// The checks of the new database that need no replay.
    pub verify: VerifyReport,
}

/// Why a rebuild runs.
#[derive(Debug, Clone)]
pub(crate) enum Why {
    Missing,
    Corrupt(String),
    Manual,
}

impl Why {
    fn name(&self) -> &'static str {
        match self {
            Why::Missing => "missing",
            Why::Corrupt(_) => "corrupt",
            Why::Manual => "manual",
        }
    }
}

/// What a rebuild works from.
pub(crate) struct Plan<'a> {
    pub(crate) db_path: &'a Path,
    pub(crate) project_dir: &'a Path,
    pub(crate) file: &'a ProjectFile,
    pub(crate) identity: &'a ServerIdentity,
    pub(crate) kind: StorageKind,
    pub(crate) opts: &'a OpenOptions,
}

/// The genesis of `file`, or `PROJECT_DB_MISSING` when the file has none (a sub-project 2
/// project whose database was never opened by sub-project 4).
pub(crate) fn genesis(file: &ProjectFile, db_path: &Path) -> Result<Genesis, StoreError> {
    file.genesis.clone().ok_or_else(|| missing(file, db_path))
}

fn missing(file: &ProjectFile, db_path: &Path) -> StoreError {
    CoreError::new(
        "PROJECT_DB_MISSING",
        format!("the database for project {} is missing", file.id),
    )
    .with_details(json!({ "projectId": file.id, "path": db_path.to_string_lossy() }))
    .with_fix("restore project.fdb into the server data dir: this project's folder does not record how to rebuild it (open it once with the server that has its database)")
    .into()
}

fn corrupt_unrecoverable(plan: &Plan, why: &Why) -> StoreError {
    let problem = match why {
        Why::Corrupt(p) => p.clone(),
        _ => String::new(),
    };
    CoreError::new(
        "DB_CORRUPT",
        format!(
            "the database of project {} is damaged and cannot be rebuilt: neither it nor the project file records the project's rev 0",
            plan.file.id
        ),
    )
    .with_details(json!({ "path": plan.db_path.to_string_lossy(), "problem": problem }))
    .with_fix("restore project.fdb from <server-data>/projects/<id>/backups (daily-*.fdb), or open the project once with a server whose database is intact")
    .into()
}

/// The `genesisHash`es of the folder's segments that are not `expected` (step 2).
fn foreign_genesis(project_dir: &Path, expected: &str) -> Vec<String> {
    let mut found: Vec<String> = crate::scan::scan(project_dir)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| match e {
            crate::scan::Entry::Segment { seg, .. } if seg.header.genesis_hash != expected => {
                Some(seg.header.genesis_hash.clone())
            }
            _ => None,
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Rebuilds the database at `plan.db_path` (module docs). `held`: the exclusive `db.lock`,
/// when the caller already took it. `None`: another open rebuilt it meanwhile, and that
/// database is used.
pub(crate) fn rebuild(
    plan: &Plan,
    why: Why,
    held: Option<std::fs::File>,
) -> Result<Option<RebuildReport>, StoreError> {
    let dir = plan.db_path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(StoreError::io(dir))?;
    let _lock = match held {
        Some(l) => l,
        None => match why {
            Why::Manual => dblock::exclusive_now(dir)?,
            Why::Missing => {
                match dblock::exclusive_within(dir, OPEN_WAITS, || plan.db_path.exists())? {
                    Some(l) => l,
                    None => return Ok(None),
                }
            }
            Why::Corrupt(_) => dblock::exclusive_within(dir, OPEN_WAITS, || false)?
                .expect("only `done` gives None"),
        },
    };
    let old = match &why {
        Why::Missing if plan.db_path.exists() => return Ok(None),
        Why::Missing => None,
        Why::Corrupt(_) if damaged(plan.db_path)?.is_none() => return Ok(None),
        Why::Corrupt(_) | Why::Manual => Some(plan.db_path),
    };
    let old_rev0 = old.and_then(|p| salvage::rev0(p, &plan.file.id));
    let rev0 = match (&old_rev0, &plan.file.genesis) {
        (Some(p), _) => p.clone(),
        (None, Some(g)) => new_project(plan.file.id.clone(), g.name.clone(), g.rate)?,
        (None, None) if old.is_none() => return Err(missing(plan.file, plan.db_path)),
        (None, None) => return Err(corrupt_unrecoverable(plan, &why)),
    };
    let expected = genesis_hash(&genesis_of(&rev0));
    let foreign = foreign_genesis(plan.project_dir, &expected);
    if !foreign.is_empty() {
        let g = genesis_of(&rev0);
        return Err(CoreError::new(
            "GENESIS_MISMATCH",
            "the project folder's history was written under another genesis than the one this rebuild would start from",
        )
        .with_details(json!({
            "expected": expected, "found": foreign,
            "genesis": { "name": g.name, "rate": g.rate.to_string() },
            "from": if old_rev0.is_some() { "database" } else { "projectFile" },
        }))
        .with_fix("restore the genesis in finisher.project.json (name and rate at creation) to the one the project was created with, then retry")
        .into());
    }
    let genesis_mismatch = match (&old_rev0, &plan.file.genesis) {
        (Some(p), Some(g)) => genesis_of(p) != *g,
        _ => false,
    };
    let ulid = ulid::Ulid::new().to_string().to_ascii_lowercase();
    let tmp = dir.join(format!("project.fdb.rebuild-{ulid}"));
    let kept = old.map(|_| files::corrupt_name(dir));
    // Salvage sources: the old database, kept copies it has not salvaged, unfinished builds.
    let listed = old.map(salvage::salvaged_from).unwrap_or_default();
    let copies: Vec<PathBuf> = files::corrupt_copies(dir)
        .into_iter()
        .filter(|c| {
            let name = c.file_name().map(|n| n.to_string_lossy().into_owned());
            !name.is_some_and(|n| listed.contains(&n))
        })
        .collect();
    let stale = files::stale_builds(dir, &tmp);
    let result = (|| {
        let (mut report, exported_through) = build(
            plan,
            &tmp,
            rev0,
            old,
            &copies,
            &stale,
            kept.as_deref(),
            &listed,
        )?;
        report.reason = why.name().to_string();
        report.problem = match &why {
            Why::Corrupt(p) => Some(p.clone()),
            _ => None,
        };
        report.genesis_mismatch = genesis_mismatch;
        if matches!(why, Why::Manual) && report.missing_from_folder > 0 {
            return Err(would_lose(&report, exported_through));
        }
        let moved = files::swap(plan.db_path, &tmp, kept.as_deref())?;
        report.corrupt_copy = kept.filter(|_| moved);
        Ok(report)
    })();
    match result {
        Ok(r) => {
            for s in &stale {
                files::remove_with_sides(s);
            }
            files::prune_corrupt(dir, r.corrupt_copy.as_deref());
            Ok(Some(r))
        }
        Err(e) => {
            files::remove_with_sides(&tmp);
            Err(e)
        }
    }
}

fn would_lose(r: &RebuildReport, exported_through: u64) -> StoreError {
    CoreError::new(
        "REBUILD_WOULD_LOSE",
        format!(
            "the project folder no longer holds {} record(s) this database exported: a rebuild from it would lose them",
            r.missing_from_folder
        ),
    )
    .with_details(json!({
        "missing": r.missing_from_folder,
        "exportedThrough": exported_through,
        "rebuiltThrough": r.rebuilt_through,
    }))
    .with_fix("restore the missing segment files under .finisher/segments (fin project verify lists them), or keep this database: it is intact")
    .into()
}

/// Steps 3, 4 and 6 of the module docs, into `tmp`.
#[allow(clippy::too_many_arguments)]
fn build(
    plan: &Plan,
    tmp: &Path,
    rev0: Project,
    old: Option<&Path>,
    copies: &[PathBuf],
    stale: &[PathBuf],
    kept: Option<&Path>,
    listed: &[String],
) -> Result<(RebuildReport, u64), StoreError> {
    let mut conn = db::connect(tmp, true)?;
    db::init(&mut conn, &rev0, MAIN)?;
    copies::record(&conn, plan.project_dir)?;
    history::note_server_id(&conn, &plan.identity.id)?;
    let heads = std::collections::HashMap::from([(
        MAIN.to_string(),
        Head {
            rev: 0,
            tip: None,
            project: rev0,
        },
    )]);
    let mut store = Store::new(
        conn,
        plan.file.id.clone(),
        tmp.to_path_buf(),
        plan.project_dir,
        plan.identity.clone(),
        heads,
        (plan.kind, None),
        plan.opts,
    );
    store.export.lease = export::StoreLease::NotHeld;
    let expected = db::meta(&store.conn, "genesisHash")?.unwrap_or_default();
    store.import.seeds = seeds::read(plan.project_dir, &plan.file.id, &expected);
    let imported = store.import()?;
    let now = crate::commit::now_ms();
    let mut report = RebuildReport {
        reason: String::new(),
        problem: None,
        records: 0,
        salvaged: 0,
        missing_from_folder: 0,
        rebuilt_through: 0,
        salvage_complete: true,
        salvage_errors: vec![],
        seeded_from_snapshot: store.import.seeded,
        corrupt_copy: None,
        salvaged_copies: vec![],
        genesis_mismatch: false,
        import_error: imported.error,
        verify: VerifyReport {
            ok: true,
            checks: vec![],
        },
    };
    // Kept copies the new database will list as salvaged: those listed before that still
    // exist, those salvaged now, and the old database once it is kept.
    let mut salvaged_from: Vec<String> = listed.to_vec();
    let mut exported_through = 0;
    let sources = old
        .into_iter()
        .map(|p| (p.to_path_buf(), true))
        .chain(copies.iter().map(|p| (p.clone(), true)))
        .chain(stale.iter().map(|p| (p.clone(), false)));
    for (src, counted) in sources {
        let out = salvage::run(&src, &store.conn, now)?;
        report.salvaged += out.added;
        if Some(src.as_path()) == old {
            report.missing_from_folder += out.lost_exported;
            exported_through = out.exported_through;
        }
        if counted {
            report.salvage_errors.extend(out.errors);
        }
        if copies.contains(&src)
            && let Some(n) = src.file_name()
        {
            let n = n.to_string_lossy().into_owned();
            report.salvaged_copies.push(n.clone());
            salvaged_from.push(n);
        }
    }
    report.salvage_complete = report.salvage_errors.is_empty();
    if let Some(k) = kept.and_then(Path::file_name) {
        salvaged_from.push(k.to_string_lossy().into_owned());
    }
    salvaged_from.sort();
    salvaged_from.dedup();
    db::set_meta(
        &store.conn,
        "salvagedFrom",
        &serde_json::to_string(&salvaged_from)?,
    )?;
    let records: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM txs", [], |r| r.get(0))?;
    report.records = db::to_u64(records)?;
    report.rebuilt_through = db::latest_rev(&store.conn, MAIN)?.unwrap_or(0);
    report.verify = crate::verify::run(&crate::verify::Subject {
        conn: &store.conn,
        project_dir: plan.project_dir,
        project_id: &plan.file.id,
        kind: plan.kind,
        live: None,
        damage: None,
    });
    store
        .conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    drop(store);
    Ok((report, exported_through))
}
