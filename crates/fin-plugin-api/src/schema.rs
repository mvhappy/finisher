//! The types `@finisher/core-types` generates from this crate, registered by
//! `crates/fin-store/examples/export_schema.rs`.

use schemars::SchemaGenerator;

use crate::contribution::ContributionKind;
use crate::licence::LicenceClass;
use crate::manifest::{CommandContribution, HookContribution, Manifest};
use crate::permission::{Permission, PermissionDiff};
use crate::protocol::{self, calls};
use crate::risk::Risk;
use crate::schema_subset::{ArgProblem, SchemaError, SchemaSubset};
use crate::validate::Invalid;

pub fn register_schemas(g: &mut SchemaGenerator) {
    g.subschema_for::<Manifest>();
    g.subschema_for::<Permission>();
    g.subschema_for::<PermissionDiff>();
    g.subschema_for::<Risk>();
    g.subschema_for::<LicenceClass>();
    g.subschema_for::<ContributionKind>();
    g.subschema_for::<Invalid>();
    // 8B: contributions, the argument schema subset and the protocol.
    g.subschema_for::<CommandContribution>();
    g.subschema_for::<HookContribution>();
    g.subschema_for::<SchemaSubset>();
    g.subschema_for::<SchemaError>();
    g.subschema_for::<ArgProblem>();
    g.subschema_for::<protocol::PluginContext>();
    g.subschema_for::<protocol::PluginRefFull>();
    g.subschema_for::<protocol::PluginDirs>();
    g.subschema_for::<protocol::CallLimits>();
    g.subschema_for::<protocol::CommandRun>();
    g.subschema_for::<protocol::CommandOut>();
    g.subschema_for::<protocol::PluginError>();
    g.subschema_for::<calls::HostMethod>();
    g.subschema_for::<calls::ProjectSnapshot>();
    g.subschema_for::<calls::ProjectSnapshotPage>();
    g.subschema_for::<calls::ProjectView>();
    g.subschema_for::<calls::ProjectViewText>();
    g.subschema_for::<calls::HttpMethod>();
    g.subschema_for::<calls::Header>();
    g.subschema_for::<calls::HttpRequest>();
    g.subschema_for::<calls::NetFetch>();
    g.subschema_for::<calls::NetFetchReply>();
    g.subschema_for::<calls::NetRead>();
    g.subschema_for::<calls::NetReadPage>();
    g.subschema_for::<calls::LogLevel>();
    g.subschema_for::<calls::Log>();
}
