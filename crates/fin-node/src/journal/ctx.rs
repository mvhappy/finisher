//! A context as `finisherd` sends it (raw ids from the request headers, Decision 23a), turned
//! into the journal's [`Ctx`]: ids keyed unless they are ULIDs, the author keyed. No napi
//! here: `commit_log` builds its contexts with it too, and the `commit_overhead` example
//! includes both files.

use fin_core::CoreError;
use fin_core::events::{Actor, ActorKind, BranchId, Ctx, MAX_RAW_ID, ProjectId};
use fin_store::WriteGuard;
use serde::Deserialize;
use serde_json::{Value, json};

/// A context as a server sends it: raw ids, which the journal keys (Decision 7).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RawCtx {
    pub(crate) project: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) tx_id: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) actor: Option<RawActor>,
}

/// An actor as `callMeta(headers)` gives it (`{kind, session?, call?}`, raw ids).
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RawActor {
    kind: Option<String>,
    session: Option<String>,
    call: Option<String>,
}

/// The `metaJson` an engine call that writes takes (`{actor?, guard?}`: `callMeta(headers)`
/// and the chain's write guard in finisherd). Only the actor is read here; the guard is read
/// by [`guard_of_meta`], strictly.
#[derive(Debug, Default, Deserialize)]
struct RawMeta {
    #[serde(default)]
    actor: Option<RawActor>,
}

/// The actor of a call's `metaJson`: `{kind: unknown}` when there is none or it does not
/// decode (a commit is never refused for its diagnostics).
pub(crate) fn actor_of_meta(meta_json: Option<&str>) -> RawActor {
    meta_json
        .and_then(|m| serde_json::from_str::<RawMeta>(m).ok())
        .and_then(|m| m.actor)
        .unwrap_or_default()
}

/// The write guard of a call's `metaJson` (`{actor?, guard?: WriteGuard}`; sub-project 5A
/// Decision 17): the default (no limit) without `metaJson` or without `guard`. Unlike the
/// actor, it is never guessed: a `metaJson` that is not a JSON object, or a `guard` that is
/// not a `WriteGuard` (`null` included), is `INVALID_OP` and the call writes nothing.
pub(crate) fn guard_of_meta(meta_json: Option<&str>) -> Result<WriteGuard, CoreError> {
    let Some(m) = meta_json else {
        return Ok(WriteGuard::default());
    };
    let invalid = |reason: String| {
        CoreError::new(
            "INVALID_OP",
            format!("the call's write guard does not decode: {reason}"),
        )
        .with_details(json!({ "field": "metaJson.guard", "reason": reason }))
        .with_fix(
            "send metaJson as {actor?, guard?: {denyBranches?: [branchId], allowBranches?: \
             [branchId]}} (WriteGuard in @finisher/core-types)",
        )
    };
    let meta = serde_json::from_str::<serde_json::Map<String, Value>>(m)
        .map_err(|e| invalid(format!("metaJson is not a JSON object: {e}")))?;
    match meta.get("guard") {
        None => Ok(WriteGuard::default()),
        Some(g) => WriteGuard::deserialize(g).map_err(|e| invalid(e.to_string())),
    }
}

pub(crate) fn event_invalid(m: impl Into<String>) -> CoreError {
    CoreError::new("EVENT_INVALID", m)
}

/// `raw` as the journal stores it: a project id lower-cased, a branch id as it is, txIds and
/// actor ids as `IdRef`s (a ULID kept, else keyed), the author keyed. An id over
/// [`MAX_RAW_ID`] bytes is dropped; an unknown actor kind is `unknown`.
pub(crate) fn ctx_of(j: &fin_journal::Journal, raw: RawCtx) -> Result<Ctx, CoreError> {
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
