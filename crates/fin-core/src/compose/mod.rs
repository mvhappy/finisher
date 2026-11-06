//! Multi-op builders (plan 4C Decision 17): pure functions that turn one `fin` command into the
//! primitive ops it sends, read off a project. They are never ops themselves: what they build
//! is committed, applied and undone as the ops it is.

use serde_json::Value;

use crate::ops::Op;

mod overwrite;

pub use overwrite::overwrite;

/// What a builder sends: the ops, in order, and echo data saying what it decided (for `fin`'s
/// output).
#[derive(Debug, Clone, PartialEq)]
pub struct Composed {
    pub ops: Vec<Op>,
    pub echo: Value,
}
