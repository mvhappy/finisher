//! The plugin name tables (8A, sub-project 8 spec §11), re-exported by [`super::names`].

use super::class::Name;
use super::names::NameTable;

/// Bundled plugins (sub-project 8 spec §11): the ids of the plugins the app ships, without
/// the leading `@` (a [`Name`] starts with a letter). `fin-node`'s compiled-in
/// `BUNDLED_PLUGINS` list (8A Task 10) must be a subset; any other plugin is a keyed `Hashed`.
pub static BUNDLED_PLUGINS: NameTable =
    NameTable::new("BUNDLED_PLUGINS", &["finisher/captions-basic"]);

/// `plugin.lifecycle` actions (sub-project 8 spec §11).
pub static PLUGIN_ACTIONS: NameTable = NameTable::new(
    "PLUGIN_ACTIONS",
    &[
        "plan",
        "install",
        "update",
        "downgrade",
        "remove",
        "enable",
        "disable",
        "revoke",
    ],
);

/// The [`BUNDLED_PLUGINS`] name of plugin id `@publisher/name`, if the table lists it.
pub fn bundled_plugin_name(id: &str) -> Option<Name> {
    id.strip_prefix('@')
        .and_then(|rest| BUNDLED_PLUGINS.get(rest))
}
