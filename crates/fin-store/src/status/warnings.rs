//! The `warnings` of [`ProjectStatus`] and the helpers that build them (module docs of
//! [`super`]).

use super::{INCOMPLETE_POLLS, QuarantineSummary, Warning};
use crate::lease::SystemClock;
use crate::{Store, StoreError, db};

impl Store {
    /// Every warning of the status, in display order.
    pub(super) fn status_warnings(
        &self,
        quarantine: &QuarantineSummary,
        compaction: &crate::compact::CompactionStatus,
    ) -> Vec<Warning> {
        let f = &self.follow;
        let mut warnings = vec![];
        let mut warn = |code: &str, message: String| {
            warnings.push(Warning {
                code: code.to_string(),
                message,
            });
        };
        if let Some(w) = &self.storage_warning {
            warn("STORAGE_OVERRIDE", w.clone());
        }
        if !SystemClock::counts_sleep() {
            warn("CLOCK_IGNORES_SLEEP", "this machine's monotonic clock does not count time asleep: after a sleep the lease relies on its record check and claims alone".into());
        }
        if quarantine.count > 0 {
            let mut m = format!(
                "{} record(s) set aside ({})",
                quarantine.count,
                quarantine
                    .reasons
                    .iter()
                    .map(|(r, n)| format!("{r}: {n}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if quarantine.reasons.contains_key("DIVERGED_TAIL") {
                m.push_str("; DIVERGED_TAIL records are this server's own edits that lost to the project folder: they are kept and stay listed until recovered");
            }
            warn("QUARANTINE", m);
        }
        for (code, m) in rewind_warnings(&self.conn, &self.server_id).unwrap_or_default() {
            warn(code, m);
        }
        if !f.foreign.is_empty() {
            warn(
                "FOREIGN_FILES",
                format!(
                    "{} file(s) under .finisher are not Finisher's",
                    f.foreign.len()
                ),
            );
        }
        if !f.stuck.is_empty() {
            warn(
                "INCOMPLETE_FILES",
                format!(
                    "incomplete for more than {INCOMPLETE_POLLS} polls on a {} folder, where writes are atomic: {}",
                    self.storage_kind(),
                    f.stuck.join(", ")
                ),
            );
        }
        let now = self.ctl.now();
        let mut locked: Vec<String> = f
            .locked
            .iter()
            .filter(|p| crate::verify::locked_too_long(Some(&self.damage.borrow()), p, now))
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        if !locked.is_empty() {
            locked.sort();
            warn(
                "LOCKED_FILES",
                format!(
                    "unreadable for 60 s or more (held open by another program, pending deletion while another client holds it, or no permission): {}",
                    locked.join(", ")
                ),
            );
        }
        if let Some(why) = &self.identity_note {
            warn(
                "SERVER_ID_RENEWED",
                format!("this server has a new server id: {why}"),
            );
        }
        if let Some(e) = &self.ctl.check_error {
            warn("LEASE_CHECK", e.clone());
        }
        for (copy, errors) in self.unacknowledged_salvages() {
            warn(
                "SALVAGE_INCOMPLETE",
                format!(
                    "a rebuild could not read all of the old database {copy} ({}), so edits that never reached the project folder may be missing from quarantine; it is kept for recovery until acknowledged",
                    errors.first().cloned().unwrap_or_default()
                ),
            );
        }
        if let Some(r) = self.last_rebuild() {
            let mut m = format!(
                "the database was rebuilt from the project folder ({}): {} record(s)",
                r.reason, r.records
            );
            if r.salvaged > 0 {
                m.push_str(&format!(
                    ", {} row(s) salvaged from the old database into quarantine (SALVAGED: edits that never reached the folder)",
                    r.salvaged
                ));
            }
            if let Some(p) = &r.corrupt_copy {
                m.push_str(&format!("; the old database is kept as {}", p.display()));
            }
            if !r.verify.ok {
                m.push_str("; fin project verify reports problems");
            }
            warn("DB_REBUILT", m);
            if r.missing_from_folder > 0 {
                warn(
                    "MISSING_FROM_FOLDER",
                    format!(
                        "{} exported record(s) of the old database are no longer in the project folder (damaged or deleted segment files): set aside in quarantine",
                        r.missing_from_folder
                    ),
                );
            }
        }
        if self.storage_kind().is_sync() && f.arriving() {
            warn(
                "SYNC_ARRIVING",
                format!(
                    "{} segment file(s) are still arriving from the sync client; after taking the lease the project stays CATCHING_UP until they are in (on a sync folder the 10 s wait does not bound how late another machine's edits arrive)",
                    f.arriving_files.len()
                ),
            );
        }
        if !f.project_file_copies.is_empty() {
            warn(
                "PROJECT_FILE_CONFLICT",
                format!(
                    "a sync client made a conflict copy of finisher.project.json: {} (this file is used; compare and delete the copy)",
                    f.project_file_copies.join(", ")
                ),
            );
        }
        for p in compaction.pending.iter().filter(|p| p.state == "abandoned") {
            warn(
                "COMPACTION_ABANDONED",
                format!(
                    "compaction {} was abandoned: {}; every file it names is kept (fin project compact checks it again)",
                    p.manifest,
                    p.reason.clone().unwrap_or_default()
                ),
            );
        }
        warnings
    }
}

pub(super) fn quarantine_summary(
    conn: &rusqlite::Connection,
) -> Result<QuarantineSummary, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT origin, reason, COUNT(*) FROM quarantine WHERE current = 1
         GROUP BY origin, reason ORDER BY origin, reason",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut s = QuarantineSummary::default();
    for row in rows {
        let (origin, reason, n) = row?;
        let n = u32::try_from(n).unwrap_or(u32::MAX);
        s.count = s.count.saturating_add(n);
        if !s.origins.contains(&origin) {
            s.origins.push(origin);
        }
        *s.reasons.entry(reason).or_default() += n;
    }
    Ok(s)
}

/// `REWOUND` / `SPLIT_BRAIN_SUSPECTED`, one per rewind still shown (module docs).
fn rewind_warnings(
    conn: &rusqlite::Connection,
    server_id: &str,
) -> Result<Vec<(&'static str, String)>, StoreError> {
    let now = crate::commit::now_ms();
    let mut out = vec![];
    for branch in db::branches(conn)? {
        for r in crate::history::rewinds(conn, &branch)? {
            if !r.shown(now) {
                continue;
            }
            let list = |v: Vec<String>| v.join(", ");
            let mut m = format!(
                "branch {branch:?} was rewound from rev {} (generation {}): {} record(s) written by {} at epoch {} lost to a line at epoch {} and are set aside (SUPERSEDED)",
                r.from_rev,
                r.generation,
                r.count,
                list(r.writers.clone()),
                list(r.epochs.iter().map(u64::to_string).collect()),
                r.winner_epoch,
            );
            if r.own > 0 {
                m.push_str(&format!(
                    "; {} of them are this server's own edits ({server_id}), which other servers never applied",
                    r.own
                ));
            }
            let code = if r.split_brain() {
                m.push_str(&format!(
                    "; the lost records span {} segment(s) and {} s of commits: two servers may have written at once (shown until acknowledged)",
                    r.segments,
                    r.span_ms / 1000
                ));
                "SPLIT_BRAIN_SUSPECTED"
            } else {
                "REWOUND"
            };
            out.push((code, m));
        }
    }
    Ok(out)
}
