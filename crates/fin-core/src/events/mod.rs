//! Diagnostics and training events (spec §21.1, D34; plan D, Decisions 1–7): the versioned
//! envelope, the closed kind registry and payload enum ([`kinds`]), the field classes
//! ([`class`]), the static name tables ([`names`]), redaction ([`redact`]), scrubbing
//! ([`scrub`]), edit intents ([`intent`]) and the wire forms of received kinds ([`wire`]).
//!
//! Pure, like the rest of `fin-core`: nothing here reads a clock, the environment or a random
//! source, or does I/O. The emitter (`fin-journal`, through `fin-node`, `finisherd` and `fin`)
//! supplies `id` (a ULID) and `ts` (ms since the Unix epoch, UTC).

use schemars::JsonSchema;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

pub mod approx;
pub mod class;
pub mod intent;
pub mod kinds;
pub mod names;
pub mod payloads;
pub mod redact;
mod refs;
pub mod scrub;
pub mod wire;

pub use class::{
    Blake3Hex, BlobHash, BlobRef, BranchId, Code, Content, ContentKind, Hashed, IdRef, KindName,
    MAX_RAW_ID, Name, NeedsHash, PathRef, PathRoot, ProjectId, RawId, ScrubCtx, Scrubbed, SrcLoc,
    Ulid, Version,
};
pub use intent::{IntentTx, MAX_INTENT_ENTRY_BYTES, TxIntent};
pub use kinds::{
    AnyPayload, EventPayload, KindSpec, WIRE_KINDS, decode_payload, kind, kinds, require_kind,
    validate_payload,
};
pub use redact::{ContentPath, Seg, blob_refs, content_paths, redact};
pub use wire::{CliCommandIn, ErrorPanicIn, WireForm};

/// The envelope version (`v`).
pub const ENVELOPE_VERSION: u16 = 1;

/// One journal entry (Decision 2), as read from disk: `payload` is a `Value` (whose schema
/// is [`AnyPayload`]'s). In memory a payload is an [`AnyPayload`], and the journal's writer
/// thread serialises an [`EnvelopeRef`], which has the same JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    /// [`ENVELOPE_VERSION`].
    pub v: u16,
    // The entry's ULID. (Plain comments on `$ref` fields: a doc comment would inline the type
    // in the generated TypeScript.)
    pub id: Ulid,
    /// Milliseconds since the Unix epoch, UTC.
    pub ts: u64,
    // The emitting process run.
    pub session: Ulid,
    pub source: Source,
    // A registry kind (`log.commit`).
    pub kind: KindName,
    /// The kind's payload version.
    pub kv: u16,
    pub consent: ConsentStamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Ctx")]
    pub ctx: Option<Ctx>,
    #[schemars(with = "AnyPayload")]
    pub payload: Value,
}

/// An entry to write, borrowed (Decision 2): the JSON of an [`Envelope`] without building a
/// `Value`. `kind` and `kv` come from the payload ([`EnvelopeRef::new`]); a payload recorded
/// without training has had [`AnyPayload::strip_content`].
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvelopeRef<'a> {
    pub v: u16,
    pub id: &'a Ulid,
    pub ts: u64,
    pub session: &'a Ulid,
    pub source: Source,
    pub kind: &'static str,
    pub kv: u16,
    pub consent: ConsentStamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctx: Option<&'a Ctx>,
    pub payload: &'a AnyPayload,
}

impl<'a> EnvelopeRef<'a> {
    pub fn new(
        id: &'a Ulid,
        ts: u64,
        session: &'a Ulid,
        source: Source,
        consent: ConsentStamp,
        ctx: Option<&'a Ctx>,
        payload: &'a AnyPayload,
    ) -> EnvelopeRef<'a> {
        let spec = payload.kind();
        EnvelopeRef {
            v: ENVELOPE_VERSION,
            id,
            ts,
            session,
            source,
            kind: spec.name,
            kv: spec.version,
            consent,
            ctx,
            payload,
        }
    }
}

/// Which program emitted an entry. The server sets it from the transport (local socket →
/// `fin`, WebSocket → `ui`), never from the payload.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    Fin,
    Finisherd,
    FinNode,
    Ui,
    Agent,
}

/// What an entry is about: the project, branch and transaction it joins the op log on, the
/// author (keyed-hashed: `FIN_AUTHOR` is free text) and the actor.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Ctx {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ProjectId")]
    pub project: Option<ProjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "BranchId")]
    pub branch: Option<BranchId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "IdRef")]
    pub tx_id: Option<IdRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Hashed")]
    pub author: Option<Hashed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Actor")]
    pub actor: Option<Actor>,
}

/// Who caused an entry: a person, an agent (with its session and the tool call that ran
/// `fin`), a plugin or the system. `session` is the agent or UI session (`FIN_SESSION`), not
/// the envelope's process run; `call` is the harness's tool-call id (`FIN_CALL`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Actor {
    pub kind: ActorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "IdRef")]
    pub session: Option<IdRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "IdRef")]
    pub call: Option<IdRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ActorKind {
    User,
    Agent,
    Plugin,
    System,
    Unknown,
}

/// The consent an entry was written under (Decisions 12 and 14): the consent revision, the
/// purposes in force (recorded ∩ current), and the weaker basis of the two purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConsentStamp {
    pub rev: u64,
    pub purposes: Purposes,
    pub basis: Basis,
}

/// A purpose events are recorded for.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum Purpose {
    // Metadata only: codes, timings, counts, ids, keyed hashes, edit outcomes. Not a doc
    // comment: a described variant splits the generated TypeScript union.
    Diagnostics,
    // Adds content (Decision 10).
    Training,
}

impl Purpose {
    const fn bit(self) -> u8 {
        match self {
            Purpose::Diagnostics => 1,
            Purpose::Training => 2,
        }
    }
}

/// A set of [`Purpose`]s, as bits (the journal's gate is one `AtomicU8` of them). Serialised
/// as a sorted array (`["diagnostics", "training"]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Purposes(u8);

impl Purposes {
    pub const NONE: Purposes = Purposes(0);
    pub const DIAGNOSTICS: Purposes = Purposes(1);
    pub const TRAINING: Purposes = Purposes(2);
    pub const ALL: Purposes = Purposes(3);

    /// The set from its bits; unknown bits are dropped.
    pub const fn from_bits(bits: u8) -> Purposes {
        Purposes(bits & Purposes::ALL.0)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, p: Purpose) -> bool {
        self.0 & p.bit() != 0
    }

    pub const fn has_training(self) -> bool {
        self.contains(Purpose::Training)
    }

    pub const fn intersect(self, other: Purposes) -> Purposes {
        Purposes(self.0 & other.0)
    }

    pub const fn union(self, other: Purposes) -> Purposes {
        Purposes(self.0 | other.0)
    }

    pub const fn with(self, p: Purpose) -> Purposes {
        Purposes(self.0 | p.bit())
    }

    pub fn iter(self) -> impl Iterator<Item = Purpose> {
        [Purpose::Diagnostics, Purpose::Training]
            .into_iter()
            .filter(move |p| self.contains(*p))
    }
}

impl From<Purpose> for Purposes {
    fn from(p: Purpose) -> Purposes {
        Purposes(p.bit())
    }
}

impl Serialize for Purposes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.iter())
    }
}

impl<'de> Deserialize<'de> for Purposes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let list = Vec::<Purpose>::deserialize(d)?;
        let set = list.iter().fold(Purposes::NONE, |s, p| s.with(*p));
        if set.iter().count() != list.len() {
            return Err(D::Error::custom("purposes repeat"));
        }
        Ok(set)
    }
}

impl JsonSchema for Purposes {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Purposes".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let item = generator.subschema_for::<Purpose>();
        schemars::json_schema!({ "type": "array", "items": item, "uniqueItems": true })
    }
}

/// What decided a purpose (Decision 8): the build default, the environment, or the user's
/// settings. Ordered weakest first; a stamp with two bases carries the weaker
/// ([`Basis::weaker`]).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum Basis {
    Default,
    Env,
    User,
}

impl Basis {
    pub fn weaker(self, other: Basis) -> Basis {
        self.min(other)
    }
}

/// A project's collection policy (Decision 13): `finisher.project.json` `collect`, read by
/// `fin-store` and fed to the journal. `never` binds every collaborator's server: no entry of
/// the project is recorded, and setting it purges the project's entries. A project whose
/// policy is not known (not open here) is neither: the journal drops its new entries and
/// purges nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CollectPolicy {
    /// Follow each user's privacy settings (the file holds no `collect` key).
    #[default]
    Default,
    Never,
}
