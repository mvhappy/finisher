//! Everything the journal needs from `fin-core::events` to size and serialise an entry, in
//! one place (D1's writer path): the caller-side size estimate, and on the writer
//! `serde_json::to_value`, `redact` against the purposes in force, then the envelope.

use fin_core::events::{
    AnyPayload, ConsentStamp, Ctx, ENVELOPE_VERSION, Envelope, Purposes, kinds, redact,
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
    session: &str,
    p: Pending,
    purposes: Purposes,
) -> serde_json::Result<()> {
    let spec = p.payload.kind();
    let mut payload = serde_json::to_value(&p.payload)?;
    redact(spec, &mut payload, purposes);
    let env = Envelope {
        v: ENVELOPE_VERSION,
        id: id.to_string(),
        ts: p.ts,
        session: p.session.as_deref().unwrap_or(session).to_owned(),
        source: p.source,
        kind: spec.name.to_owned(),
        kv: spec.version,
        consent: ConsentStamp {
            rev: p.stamp.rev,
            purposes,
            basis: p.stamp.basis,
        },
        ctx: p.ctx,
        payload,
    };
    let start = out.len();
    if let Err(e) = serde_json::to_writer(&mut *out, &env) {
        out.truncate(start);
        return Err(e);
    }
    out.push(b'\n');
    Ok(())
}
