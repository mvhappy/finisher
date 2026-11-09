# 4C fixture

A project written by the sub-project 4C store (commit `59e30f4`, the code before plan 5E Task 1),
kept so 5E's format 3 (`drop` records, the `drops` index, database version 4) can prove that a
4C project opens, migrates and rebuilds to identical state, including its branches, checkpoint,
merge, undo/redo, cherry-pick and a 4C op (`item.split`).

Contents: `folder/` (`finisher.project.json`, one `.fseg` each for `main` and `br1`),
`project.fdb` (database version 3, WAL checkpointed, single file), `expected.json`
(`{branches: {main: {rev, project}, br1: {rev, project}}, stacks: {main, br1}, views}`):
`Store::snapshot` per branch, `Store::undo_stacks(branch, "fixture")` and `Store::branches(true)`
after the last commit.

## How it was made

1. A throwaway generator (a binary crate outside the workspace, in a scratch directory, with
   its own `CARGO_TARGET_DIR`, depending on `fin-store` of a temporary worktree at `59e30f4`;
   not part of this repository) created a project with `asset.add`, then on `main` 3 inserts on
   `t1`, a trim and a split; `branch new feature` (`br1`) with 2 inserts; checkpoint `v1` on
   `main`; one more insert on `main`; `merge` of `br1` into `main`; `undo`, `redo`; one more
   insert on `br1` and a cherry-pick of it onto `main`; one last insert on `br1` (so it stays
   active). All authored by `fixture` with deterministic tx ids (`01JC4C…`). It ticked until
   every record was exported, checked `verify`, and closed the store.
2. Scrub: only `finisher.project.json`, the two `.fseg` files and `project.fdb` were copied. On
   the DB: `DELETE FROM meta WHERE key IN ('projectDir','serverIds','createKey'); DELETE FROM
   lease_state; VACUUM;` then a TRUNCATE checkpoint.
3. Leak check: no host name, user name, home path, `/tmp/` path or machine id in any file
   (including `strings project.fdb`). The segment headers hold only random `writerId` ULIDs.

Open it with `OpenOptions { relocate: true, .. }` via `integration_support::fixture_4c`.
