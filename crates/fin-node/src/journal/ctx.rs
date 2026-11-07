//! A context as `finisherd` sends it (raw ids from the request headers, Decision 23a), turned
//! into the journal's [`Ctx`]: ids keyed unless they are ULIDs, the author keyed.

use fin_core::events::{Actor, ActorKind, BranchId, Ctx, MAX_RAW_ID, ProjectId};
use napi::bindgen_prelude::*;
use serde::Deserialize;
use serde_json::Value;

use fin_core::CoreError;

use crate::reject;

/// A context as a server sends it: raw ids, which the journal keys (Decision 7).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RawCtx {
    project: Option<String>,
    branch: Option<String>,
    tx_id: Option<String>,
    author: Option<String>,
    actor: Option<RawActor>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawActor {
    kind: Option<String>,
    session: Option<String>,
    call: Option<String>,
}

pub(super) fn event_invalid(m: impl Into<String>) -> Error {
    reject(CoreError::new("EVENT_INVALID", m))
}

/// `raw` as the journal stores it: a project id lower-cased, a branch id as it is, txIds and
/// actor ids as `IdRef`s (a ULID kept, else keyed), the author keyed. An id over
/// [`MAX_RAW_ID`] bytes is dropped; an unknown actor kind is `unknown`.
pub(super) fn ctx_of(j: &fin_journal::Journal, raw: RawCtx) -> Result<Ctx> {
    let id = |s: Option<String>| {
        s.filter(|s| !s.is_empty() && s.len() <= MAX_RAW_ID)
            .and_then(|s| j.id_ref(&s))
    };
    let project = match raw.project {
        None => None,
        Some(p) => Some(
            ProjectId::parse(&p.to_ascii_lowercase())
                .ok_or_else(|| event_invalid("ctx.project is not a project id"))?,
        ),
    };
    let branch = match raw.branch {
        None => None,
        Some(b) => Some(
            BranchId::parse(&b).ok_or_else(|| event_invalid("ctx.branch is not a branch id"))?,
        ),
    };
    let actor = raw.actor.map(|a| Actor {
        kind: a
            .kind
            .and_then(|k| serde_json::from_value(Value::String(k)).ok())
            .unwrap_or(ActorKind::Unknown),
        session: id(a.session),
        call: id(a.call),
    });
    Ok(Ctx {
        project,
        branch,
        tx_id: id(raw.tx_id),
        author: raw.author.and_then(|a| j.hash(&a)),
        actor,
    })
}
