//! `main!`: a plugin binary's whole `main`.

/// Defines `fn main()` running the plugin type over stdin and stdout
/// ([`crate::runtime::run`]): `hello` (`v` 1, [`crate::MINOR`], no envelope hash), a trailing
/// `--warm` for a warm process, the job loop, cancel, `alive` and `ready`.
///
/// ```ignore
/// fin_plugin_sdk::main!(MyPlugin);
/// ```
#[macro_export]
macro_rules! main {
    ($plugin:ty) => {
        fn main() {
            $crate::runtime::run::<$plugin>()
        }
    };
}
