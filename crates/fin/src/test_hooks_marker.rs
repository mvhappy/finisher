//! An empty library target. It exists only so that fin's self dev-dependency
//! (`fin = { path = ".", features = ["test-hooks"] }`) is valid: Cargo ignores a dependency on
//! a package without a library, and that dependency is what turns fin's `test-hooks` feature
//! on for fin's own tests (and never for `cargo build`). See `backend::test_hook`.
