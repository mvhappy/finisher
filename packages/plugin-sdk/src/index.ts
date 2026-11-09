// `@finisher/plugin-sdk`: the TypeScript SDK for Finisher code plugins (sub-project 8 spec §3,
// §7.2; plan 8B Task 13). A plugin process is a worker of 5C's `plugin` family and speaks the
// `fin-jobs` envelope on stdin and stdout (plan 8B Decision 2): `hello` with `v` 1, the job loop,
// `ready` after each job under a trailing `--warm`, and typed host calls over `call`/`reply`.
// Its types are generated into `@finisher/core-types`, never written here. Built in Task 13.

/** The envelope's major version this SDK speaks (5C-1: `hello.v`). */
export const ENVELOPE_VERSION = 1;
