#!/bin/sh
# Cargo runner on Linux (.cargo/config.toml): test binaries (target/*/deps/*) run with TMPDIR
# on tmpfs (/dev/shm), so their temp dirs live in memory.
#
# The store tests write through the real durability path: every segment, backup, server id and
# SQLite commit (synchronous=FULL) is fsynced. On a disk each fsync costs milliseconds and the
# suite waits on thousands of them; on tmpfs the same calls return at once. The code under
# test is unchanged; only the filesystem behind the temp dir differs, as on distributions
# whose /tmp is already tmpfs. Tests that need a disk filesystem use CARGO_TARGET_TMPDIR (see
# `read_uncached_reads_every_size` in crates/fin-store/tests/codec.rs).
#
# - FIN_TEST_TMPDIR=<dir> runs the tests with TMPDIR=<dir> (FIN_TEST_TMPDIR=/tmp: on disk).
# - A TMPDIR already set is kept.
# - Without a writable tmpfs /dev/shm with 512 MiB free, nothing changes (the suite peaks
#   near 70 MiB).
# - `cargo run` binaries (not under deps/) run as they are.
set -eu
case "$1" in
*/deps/*)
    if [ -n "${FIN_TEST_TMPDIR:-}" ]; then
        TMPDIR=$FIN_TEST_TMPDIR
        export TMPDIR
    elif [ -z "${TMPDIR:-}" ] && [ -d /dev/shm ] && [ -w /dev/shm ] &&
        [ "$(stat -f -c %T /dev/shm 2>/dev/null)" = tmpfs ]; then
        free_kib=$(df -Pk /dev/shm 2>/dev/null | awk 'NR == 2 { print $4 }')
        if [ "${free_kib:-0}" -ge 524288 ]; then
            TMPDIR=/dev/shm
            export TMPDIR
        fi
    fi
    ;;
esac
exec "$@"
