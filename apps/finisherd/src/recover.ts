// `project.recover` in sub-project 4A: the documented manual procedure (spec §3.7). The real
// recovery (a `recovered-*` branch per side) needs branches and lands in 4B.

export const RECOVER_MANUAL = [
  "Automatic recovery comes with branches (sub-project 4B). Until then, by hand:",
  "1. Run `fin project status` on this server. `readOnly.reason` says why the project takes no edits; `foreign`, `pending` and `waiting` list the files involved; `quarantine` counts the records set aside (they are kept, never deleted).",
  "2. LEASE_HELD or LEASE_LOST: wait for the other server, or stop it and run `fin project takeover` (add --confirm on a sync folder). Nothing else is needed.",
  "3. CHAIN_FORK: two histories exist under .finisher/segments/<branch>/. Decide which one to keep. Stop Finisher on every machine that has the project open, then move the other history's segment files (the `pending` ones past the fork) out of the project folder, to a place outside it. Do not delete them: they hold edits.",
  "4. CONFLICT_COPY: a sync client made a conflict copy of a Finisher file (listed in `foreign`): another machine may have written at the same time. Move the copy out of .finisher/ to a place outside the project folder; keep it until you are sure nothing in it is needed.",
  "5. Let sync clients finish, then reopen the project: `fin project status` shows `readOnly: null` once the folder reads as one history. If this server's database no longer matches the folder, `fin project rebuild` rebuilds it from the folder (it refuses with REBUILD_WOULD_LOSE when the folder lacks records the database exported).",
  "6. NEWER_FORMAT: update Finisher; nothing to move.",
].join("\n");
