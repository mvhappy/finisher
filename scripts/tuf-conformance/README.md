# TUF conformance for the registry client

The registry client (`fin-registry::tuf`, over `tough`) is run two ways against hostile servers.
Nothing here ships; the client is the `tuf_conformance_client` example built with the test-only
`test-http` feature (`scripts/check-test-hooks.mjs` keeps that feature out of every default build).

| File | What |
|---|---|
| `suite.json` | the upstream suite ([theupdateframework/tuf-conformance](https://github.com/theupdateframework/tuf-conformance)), pinned by commit SHA |
| `client.sh` | the suite's `--entrypoint`: finds the example binary (`CARGO_TARGET_DIR` is honoured) |
| `client.sh.xfails` | the expected failures, each group with the reason; the list is strict (a test that starts to pass fails the run until it is removed) |
| `run.mjs` | runs the pinned suite locally (git-ignored checkout and virtualenv under `archive/tuf-conformance/`; needs Python >= 3.10, `PYTHON=` picks one) |
| `summary.mjs` | the suite's report as a table (the CI job summary) and the verdict |
| `attacks.mjs` | the attack matrix: rollback, freeze, mix-and-match, key rotation (and the replaced key), threshold, expired metadata, endless data, tampered targets, against `fin-plugin`-built repositories served by a hostile loopback server |
| `faketime`, `faketime.mjs` | stand-in for libfaketime where it is not installed (macOS): sets `FIN_TUF_FAKE_NOW_MS`, which the example reads as its clock |

CI (`.github/workflows/tuf-conformance.yml`, Linux only) runs both and writes the tables to the
job summary. Locally:

```sh
cargo build --release -p fin-plugin-cli
node scripts/tuf-conformance/attacks.mjs      # needs the client built below
node scripts/tuf-conformance/run.mjs          # builds the client, then runs the upstream suite
```

## Expected failures (`client.sh.xfails`)

None of them is a case where the client accepts something the suite says it must refuse.

- **Trusted generations are written whole.** After a failed refresh the client keeps exactly the
  files it had (plan 8A review focus 2); python-tuf keeps each role as it verified. The refresh
  fails as required; only the versions kept afterwards differ (10 tests).
- **A timestamp of the trusted version with other content.** `tough` takes it, then fails closed
  fetching the snapshot it names; the specification keeps the trusted copy (1 test).
- **Key types and digests Finisher does not use.** ed25519 (Decision 9) and SHA-256 targets: RSA-PSS
  384/512, ECDSA P-384, ML-DSA, a targets file without SHA-256 (7 tests).
- **Role names with URL-special characters.** Refused; Finisher's roles are publisher names (4 tests).
- **Delegation graphs.** `tough` refuses a delegated role that lists a path it was not delegated, and
  the client loads every delegated role of a snapshot instead of searching per requested path
  (17 tests).
