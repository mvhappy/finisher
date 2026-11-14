# 4A fixture

A project written by the sub-project 4A store (commit `ef6f3ca`), kept so 4B upgrade tests can
prove that a 4A project opens, migrates and rebuilds to identical state.

Contents: `folder/` (`finisher.project.json`, one `.fseg` for `main`, revs 1-6, format 1/1),
`project.fdb` (WAL checkpointed, single file, schema as 4A wrote it), `expected.json`
(`{rev, project}` = `Store::snapshot("main")` after the last commit).

## How it was made

1. A throwaway generator, `crates/fin-store/examples/make_4a_fixture.rs`, lived in a 4A worktree
   (`archive/wt-4a` at `ef6f3ca`, git-ignored; it is not part of 4B). The plan's sketch matched
   the 4A API (`Store::create`, `commit`, `tick`, `close`, `snapshot`); no adaptation was needed.
   It created a project in a temp dir, committed `asset.add`, then inserts, a trim and a delete
   (6 revs), ticked until exported, closed the store, and ran
   `PRAGMA wal_checkpoint(TRUNCATE)`.
2. Scrub: only `finisher.project.json`, the `.fseg` file and `project.fdb` were copied. No
   `lease.json`, `lease.claim-*`, `lease.probe-*`, `server-id*`, backups or locks. On the DB:
   `DELETE FROM meta WHERE key IN ('projectDir','serverIds','createKey'); DELETE FROM
   lease_state; VACUUM;`.
3. Leak check: no host name, user name, home path, `/tmp/` path or machine id in any file
   (including `strings project.fdb`). The segment header holds only the random `writerId` ULID.

Open it with `OpenOptions { relocate: true, .. }` via `integration_support::fixture_4a`.
