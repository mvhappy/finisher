//! The types `@finisher/core-types` generates from this crate, registered by
//! `crates/fin-store/examples/export_schema.rs`.

use schemars::SchemaGenerator;

use crate::contribution::ContributionKind;
use crate::licence::LicenceClass;
use crate::manifest::Manifest;
use crate::permission::{Permission, PermissionDiff};
use crate::risk::Risk;
use crate::validate::Invalid;

pub fn register_schemas(g: &mut SchemaGenerator) {
    g.subschema_for::<Manifest>();
    g.subschema_for::<Permission>();
    g.subschema_for::<PermissionDiff>();
    g.subschema_for::<Risk>();
    g.subschema_for::<LicenceClass>();
    g.subschema_for::<ContributionKind>();
    g.subschema_for::<Invalid>();
}
