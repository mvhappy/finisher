# 4B fixture

A project written by the sub-project 4B store (commit `36fe7e0`, 4B merged with the latest 4A),
kept so 4C upgrade tests can prove that a 4B project opens, migrates and rebuilds to identical
state, including its branches, checkpoint, merge, undo/redo and cherry-pick.

Contents: `folder/` (`finisher.project.json`, one `.fseg` each for `main` and `br1`),
`project.fdb` (WAL checkpointed, single file, schema as 4B wrote it), `expected.json`
(`{branches: {main: {rev, project}, br1: {rev, project}}, stacks: {main, br1}, argsHashes}`):
`Store::snapshot` per branch and `Store::undo_stacks(branch, "fixture")` after the last commit.
`argsHashes` holds the `argsHash` that 4B's `fin` computed (`args_hash(&[input(step)])`) for
three fixed inputs: `clip.insert`, `clip.trim` and `clip.delete` (plan 4C Decision 7).

## How it was made

1. A throwaway generator, `crates/fin-store/examples/make_4b_fixture.rs`, lived in a 4B worktree
   (`archive/wt-4b` at `36fe7e0`, git-ignored; it is not part of 4C). It created a project in a
   temp dir with `asset.add` (video + audio, so `t2` takes clips), then on `main` 4 inserts on
   `t1`/`t2`, 2 trims (one per edge) and 1 delete; `branch new feature` (`br1`) with 2 inserts;
   checkpoint `v1` on `main`; one more insert on `main`; `merge` of `br1` into `main`; `undo`,
   `redo`; one more insert on `br1` and a cherry-pick of it onto `main` (the plan's order picks
   after the merge, which finds the feature edits already present, so the picked record is made
   after the merge). All authored by `fixture` with deterministic tx ids. It ticked until
   exported, closed the store and ran `PRAGMA wal_checkpoint(TRUNCATE)`.
2. Scrub: only `finisher.project.json`, the two `.fseg` files and `project.fdb` were copied. No
   `lease.json`, `lease.claim-*`, `lease.probe-*`, `server-id*`, backups or locks. On the DB:
   `DELETE FROM meta WHERE key IN ('projectDir','serverIds','createKey'); DELETE FROM
   lease_state; VACUUM;` then a TRUNCATE checkpoint.
3. `argsHashes`: a throwaway test in the worktree's `fin` printed `args_hash` of the three
   inputs.
4. Leak check: no host name, user name, home path, `/tmp/` path or machine id in any file
   (including `strings project.fdb`). The segment headers hold only random `writerId` ULIDs.

Open it with `OpenOptions { relocate: true, .. }` via `integration_support::fixture_4b`.
