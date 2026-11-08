//! `ui.action` (sub-project 9), `review.verdict` (sub-project 10), `preview.stats` (sub-project
//! 9) and `render.sample` (training only; a later sub-project adds its source): reserved,
//! version 0, empty until they get an emitter.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::approx::ApproxBytes;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct UiAction {}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct ReviewVerdict {}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct PreviewStats {}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct RenderSample {}

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

impl ApproxBytes for PreviewStats {
    fn approx_bytes(&self) -> usize {
        2
    }
}

impl ApproxBytes for RenderSample {
    fn approx_bytes(&self) -> usize {
        2
    }
}
