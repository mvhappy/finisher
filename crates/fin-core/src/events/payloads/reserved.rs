//! `ui.action` (sub-project 9) and `review.verdict` (sub-project 10): reserved, version 0,
//! empty until they get an emitter.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::approx::ApproxBytes;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct UiAction {}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct ReviewVerdict {}

impl ApproxBytes for UiAction {
    fn approx_bytes(&self) -> usize {
        2
    }
}

impl ApproxBytes for ReviewVerdict {
    fn approx_bytes(&self) -> usize {
        2
    }
}
