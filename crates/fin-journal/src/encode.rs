//! Everything the journal needs from `fin-core::events` to size and serialise an entry, in
//! one place (D1's writer path): the caller-side size estimate, and on the writer the
//! payload stripped of content the purposes in force do not allow, then the borrowed
//! envelope serialised straight to the line (no `Value`).

use fin_core::events::{
    AnyPayload, Basis, BlobRef, ConsentStamp, Ctx, Envelope, EnvelopeRef, Purpose, Purposes,
    Ulid as EventUlid, blob_refs, kind, kinds, redact,
};
use ulid::Ulid;

use crate::control::Pending;

/// The envelope's own fields, without `ctx` and `payload`: `v`, `id`, `ts`, `session`,
/// `source`, `kind`, `kv` and `consent`, with their keys (about 180 bytes serialised).
const ENVELOPE_BYTES: usize = 192;

/// The size estimate an entry counts against the channel's byte bound: the payload's
/// `approx_bytes` (which adds its own per-variant overhead) plus the envelope's.
pub(crate) fn entry_bytes(ctx: Option<&Ctx>, p: &AnyPayload) -> usize {
    ENVELOPE_BYTES + ctx.map_or(0, ctx_bytes) + p.approx_bytes()
}

fn ctx_bytes(c: &Ctx) -> usize {
    let s = |o: Option<&str>| o.map_or(0, |s| s.len() + 12);
    12 + s(c.project.as_ref().map(|x| x.as_str()))
        + s(c.branch.as_ref().map(|x| x.as_str()))
        + s(c.tx_id.as_ref().map(|x| x.as_str()))
        + s(c.author.as_ref().map(|x| x.as_str()))
        + c.actor.as_ref().map_or(0, |a| {
            24 + s(a.session.as_ref().map(|x| x.as_str())) + s(a.call.as_ref().map(|x| x.as_str()))
        })
}

/// Warms `fin-core`'s per-kind content-path cache, so the first entry does not pay for the
/// schema walk.
pub(crate) fn warm() {
    if let Some(k) = kinds().first() {
        redact(k, &mut serde_json::Value::Null, Purposes::NONE);
    }
}

/// Appends `p` as one JSON line (with its `\n`) to `out`, its content removed unless
/// `purposes` has training, stamped with `purposes`. On error `out` is left as it was.
pub(crate) fn encode(
    out: &mut Vec<u8>,
    id: Ulid,
    session: &EventUlid,
    p: Pending,
    purposes: Purposes,
) -> serde_json::Result<()> {
    let mut payload = p.payload;
    if !purposes.contains(Purpose::Training) {
        payload.strip_content();
    }
    let id = EventUlid::from_u128(id.0);
    let env = EnvelopeRef::new(
        &id,
        p.ts,
        p.session.as_ref().unwrap_or(session),
        p.source,
        ConsentStamp {
            rev: p.stamp.rev,
            purposes,
            basis: p.stamp.basis,
        },
        p.ctx.as_ref(),
        &payload,
    );
    let start = out.len();
    if let Err(e) = serde_json::to_writer(&mut *out, &env) {
        out.truncate(start);
        return Err(e);
    }
    out.push(b'\n');
    Ok(())
}

/// A line already encoded, written again under `gate`: its purposes become the stamp ∩
/// `gate` (with the basis in force now), and its content goes unless training is left. The line is `None` when nothing is
/// left (or it does not parse, which the writer never produces): the entry is dropped. The
/// second value lists the blob references the rewrite removed.
pub(crate) fn restamp(
    line: &[u8],
    gate: Purposes,
    basis: Basis,
) -> (Option<Vec<u8>>, Vec<BlobRef>) {
    let Ok(mut e) = serde_json::from_slice::<Envelope>(line) else {
        return (None, Vec::new());
    };
    let Some(spec) = kind(e.kind.as_str(), e.kv) else {
        return (None, Vec::new());
    };
    let before = blob_refs(spec, &e.payload);
    let purposes = e.consent.purposes.intersect(gate);
    if purposes.is_empty() {
        return (None, before);
    }
    redact(spec, &mut e.payload, purposes);
    let after = blob_refs(spec, &e.payload);
    e.consent.purposes = purposes;
    e.consent.basis = basis;
    let gone = before.into_iter().filter(|b| !after.contains(b)).collect();
    match serde_json::to_vec(&e) {
        Ok(mut out) => {
            out.push(b'\n');
            (Some(out), gone)
        }
        Err(_) => (None, after.into_iter().chain(gone).collect()),
    }
}

/// The blob references of an encoded line (none when it does not parse).
pub(crate) fn line_blobs(line: &[u8]) -> Vec<BlobRef> {
    let Ok(e) = serde_json::from_slice::<Envelope>(line) else {
        return Vec::new();
    };
    kind(e.kind.as_str(), e.kv).map_or_else(Vec::new, |spec| blob_refs(spec, &e.payload))
}
