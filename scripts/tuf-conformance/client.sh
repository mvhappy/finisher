#!/bin/sh
# The client the tuf-conformance suite drives (its `--entrypoint`): the `tuf_conformance_client`
# example of fin-registry, built with `--features test-http` (README.md). The suite splits the
# entrypoint on spaces, so this wrapper finds the binary itself.
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
exec "${CARGO_TARGET_DIR:-$root/target}/release/examples/tuf_conformance_client" "$@"
