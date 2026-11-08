# Bundled plugins

Each folder is one first-party plugin shipped unpacked with the app (8A Decision 24). `crates/fin-node/build.rs` hashes every tree into `BUNDLED_PLUGINS`; editing a file changes the hash of the next build. Files are hashed byte for byte, so they stay LF.
