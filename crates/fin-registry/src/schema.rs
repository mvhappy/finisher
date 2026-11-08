//! The types `@finisher/core-types` generates from this crate, registered by
//! `crates/fin-store/examples/export_schema.rs`.

use schemars::SchemaGenerator;

use crate::plan::{Accept, InstallPlan, InstalledPlugin, ValidationReport};
use crate::target::TargetCustom;

pub fn register_schemas(g: &mut SchemaGenerator) {
    g.subschema_for::<InstallPlan>();
    g.subschema_for::<InstalledPlugin>();
    g.subschema_for::<Accept>();
    g.subschema_for::<ValidationReport>();
    g.subschema_for::<TargetCustom>();
}
