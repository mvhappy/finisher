//! `consent.changed` (Decision 12): journaled under the *old* consent, just before the new one
//! takes effect on disk.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::approx::approx_struct;
use crate::events::{Basis, Purposes};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConsentChanged {
    pub from: Purposes,
    pub to: Purposes,
    // The new consent's basis. Not a doc comment: it would inline `Basis` in the generated
    // TypeScript.
    pub basis: Basis,
    /// The new consent revision.
    pub rev: u64,
}
approx_struct!(ConsentChanged {
    from,
    to,
    basis,
    rev
});
