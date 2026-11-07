//! The closed kind registry and payload enum (plan D, Decision 3). One table, [`registry!`],
//! lists every kind once with its version, flags, sources and payload type, and generates
//! [`KINDS`], [`AnyPayload`] (one variant per kind, reserved ones included), the typed decode
//! of [`validate_payload`] and the kind-keyed schema of [`EventPayload`].
//!
//! Names are `domain.verb`, lower camel case within a segment. Version 0 marks a reserved kind
//! (its payload may change without a bump until it gets an emitter, which sets version 1); a
//! changed payload is `kv + 1`, and there are no upcasters (the journal is not canonical
//! state).

use std::borrow::Cow;

use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::DeserializeOwned;
use serde::{Serialize, Serializer};
use serde_json::{Value, json};

use super::Source;
use super::approx::ApproxBytes;
use super::class::BlobRef;
use super::payloads::{self as p, body_blobs};
use crate::error::CoreError;

/// One kind of event.
#[derive(Debug)]
pub struct KindSpec {
    /// `domain.verb`.
    pub name: &'static str,
    /// The payload version (`kv`); 0 = reserved.
    pub version: u16,
    /// No emitter yet (version 0).
    pub reserved: bool,
    /// The kind exists for training: under diagnostics alone it is written as its metadata
    /// part (its content removed, like any kind's).
    pub training_only: bool,
    /// The sources that may emit it.
    pub sources: &'static [Source],
    /// The payload's JSON Schema, a root document with its own `$defs`.
    pub schema: fn() -> Value,
}

/// A payload type: serialisable, decodable, schema'd, sized, and able to list its blobs.
pub trait Payload: Serialize + DeserializeOwned + JsonSchema + ApproxBytes {
    /// The [`BlobRef`]s in its content fields.
    fn blobs(&self) -> Vec<&BlobRef> {
        Vec::new()
    }
}

fn schema_of<T: JsonSchema>() -> Value {
    let generator = SchemaGenerator::new(SchemaSettings::draft2020_12());
    serde_json::to_value(generator.into_root_schema_for::<T>()).unwrap_or(Value::Bool(false))
}

/// The per-variant overhead [`AnyPayload::approx_bytes`] adds (the envelope's own fields are
/// the journal's to count).
const VARIANT_BYTES: usize = 16;

macro_rules! registry {
    (@flag) => { false };
    (@flag $t:tt) => { $t };
    ($($variant:ident($ty:ty) = $name:literal v $version:literal $(training_only $t:tt)?
        [$($src:ident),*];)*) => {
        #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
        #[repr(usize)]
        enum Idx { $($variant),* }

        /// Every kind, in registry order.
        pub static KINDS: &[KindSpec] = &[$(KindSpec {
            name: $name,
            version: $version,
            reserved: $version == 0,
            training_only: registry!(@flag $($t)?),
            sources: &[$(Source::$src),*],
            schema: schema_of::<$ty>,
        }),*];

        /// A payload in memory: one variant per kind, each boxed (the journal's channel holds
        /// it). [`Serialize`] writes the payload part only; the envelope is the journal's.
        #[derive(Debug, Clone, PartialEq)]
        pub enum AnyPayload { $($variant(Box<$ty>)),* }

        $(impl From<$ty> for AnyPayload {
            fn from(p: $ty) -> AnyPayload {
                AnyPayload::$variant(Box::new(p))
            }
        })*

        impl AnyPayload {
            /// The payload's kind.
            pub fn kind(&self) -> &'static KindSpec {
                match self { $(AnyPayload::$variant(_) => &KINDS[Idx::$variant as usize],)* }
            }

            /// An estimate of the serialised payload size, without serialising.
            pub fn approx_bytes(&self) -> usize {
                VARIANT_BYTES + match self { $(AnyPayload::$variant(x) => x.approx_bytes(),)* }
            }

            /// The [`BlobRef`]s in the payload's content fields.
            pub fn blobs(&self) -> impl Iterator<Item = &BlobRef> {
                match self { $(AnyPayload::$variant(x) => Payload::blobs(&**x),)* }.into_iter()
            }
        }

        impl Serialize for AnyPayload {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                match self { $(AnyPayload::$variant(x) => x.serialize(s),)* }
            }
        }

        fn decode(spec: &KindSpec, payload: &Value) -> Option<Result<AnyPayload, serde_json::Error>> {
            match (spec.name, spec.version) {
                $(($name, $version) => Some(
                    <$ty as serde::Deserialize>::deserialize(payload).map(AnyPayload::from)
                ),)*
                _ => None,
            }
        }

        /// Adds every payload type to `generator` (`export_schema`).
        pub fn register_schemas(generator: &mut SchemaGenerator) {
            $(generator.subschema_for::<$ty>();)*
        }

        fn event_payload_variants(generator: &mut SchemaGenerator) -> Vec<Value> {
            vec![$(json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "const": $name },
                    "kv": { "type": "integer", "const": $version },
                    "payload": generator.subschema_for::<$ty>(),
                },
                "required": ["kind", "kv", "payload"],
                "additionalProperties": false,
            })),*]
        }
    };
}

registry! {
    SessionStart(p::SessionStart) = "session.start" v 1 [Finisherd, FinNode];
    SessionEnd(p::SessionEnd) = "session.end" v 1 [Finisherd, FinNode];
    SessionUnclean(p::SessionUnclean) = "session.unclean" v 1 [Finisherd, FinNode];
    ConsentChanged(p::ConsentChanged) = "consent.changed" v 1 [FinNode];
    JournalDropped(p::JournalDropped) = "journal.dropped" v 1 [FinNode];
    LogCommit(p::LogCommit) = "log.commit" v 1 [FinNode];
    CliCommand(p::CliCommand) = "cli.command" v 1 [Fin];
    RpcStats(p::RpcStats) = "rpc.stats" v 1 [Finisherd];
    EvalStats(p::EvalStats) = "eval.stats" v 1 [FinNode];
    FontsFetch(p::FontsFetch) = "fonts.fetch" v 1 [FinNode];
    StorageState(p::StorageState) = "storage.state" v 1 [FinNode];
    ErrorInternal(p::ErrorInternal) = "error.internal" v 1 [FinNode, Finisherd];
    ErrorPanic(p::ErrorPanic) = "error.panic" v 1 [Fin, Finisherd, FinNode];
    AgentSession(p::AgentSession) = "agent.session" v 0 [Agent];
    AgentTurn(p::AgentTurn) = "agent.turn" v 0 training_only true [Agent];
    AgentToolCall(p::AgentToolCall) = "agent.toolCall" v 0 training_only true [Agent];
    AgentToolResult(p::AgentToolResult) = "agent.toolResult" v 0 training_only true [Agent];
    AgentEditVerdict(p::AgentEditVerdict) = "agent.editVerdict" v 0 [Agent];
    UiAction(p::UiAction) = "ui.action" v 0 [Ui];
    ReviewVerdict(p::ReviewVerdict) = "review.verdict" v 0 [Ui];
}

impl Payload for p::SessionStart {}
impl Payload for p::SessionEnd {}
impl Payload for p::SessionUnclean {}
impl Payload for p::ConsentChanged {}
impl Payload for p::JournalDropped {}
impl Payload for p::LogCommit {}
impl Payload for p::CliCommand {}
impl Payload for p::RpcStats {}
impl Payload for p::EvalStats {}
impl Payload for p::FontsFetch {}
impl Payload for p::StorageState {}
impl Payload for p::ErrorInternal {}
impl Payload for p::ErrorPanic {}
impl Payload for p::AgentSession {}
impl Payload for p::AgentEditVerdict {}
impl Payload for p::UiAction {}
impl Payload for p::ReviewVerdict {}

impl Payload for p::AgentTurn {
    fn blobs(&self) -> Vec<&BlobRef> {
        body_blobs(&[self.messages.as_ref(), self.output.as_ref()])
    }
}

impl Payload for p::AgentToolCall {
    fn blobs(&self) -> Vec<&BlobRef> {
        body_blobs(&[self.args.as_ref()])
    }
}

impl Payload for p::AgentToolResult {
    fn blobs(&self) -> Vec<&BlobRef> {
        body_blobs(&[self.output.as_ref()])
    }
}

/// Every kind, in registry order.
pub fn kinds() -> &'static [KindSpec] {
    KINDS
}

/// The kind `name` at payload version `version`, if the registry has it.
pub fn kind(name: &str, version: u16) -> Option<&'static KindSpec> {
    KINDS
        .iter()
        .find(|k| k.name == name && k.version == version)
}

/// [`kind`], or `EVENT_KIND_UNKNOWN` (exit 3).
pub fn require_kind(name: &str, version: u16) -> Result<&'static KindSpec, CoreError> {
    kind(name, version).ok_or_else(|| {
        CoreError::new(
            "EVENT_KIND_UNKNOWN",
            format!("no event kind {name:?} at version {version}"),
        )
        .with_details(json!({ "kind": name, "kv": version }))
        .with_fix("send a kind and version from fin-core's event registry")
    })
}

/// Decodes a received payload as `spec`'s type: the only way a `Value` becomes a payload.
/// A typed decode (no schema engine): the class types check their patterns, a `Name` reads
/// through its table, unknown fields are dropped. `EVENT_KIND_UNKNOWN` when `spec` is not a
/// registry kind, `EVENT_INVALID` when the payload does not decode.
pub fn validate_payload(spec: &KindSpec, payload: &Value) -> Result<AnyPayload, CoreError> {
    let decoded = decode(spec, payload).ok_or_else(|| {
        require_kind(spec.name, spec.version)
            .err()
            .unwrap_or_else(|| CoreError::new("EVENT_KIND_UNKNOWN", "unknown event kind"))
    })?;
    decoded.map_err(|e| {
        CoreError::new(
            "EVENT_INVALID",
            format!("the {} payload does not decode: {e}", spec.name),
        )
        .with_details(json!({ "kind": spec.name, "kv": spec.version }))
    })
}

/// The payload union keyed by kind, for TypeScript: `{kind, kv, payload}` with `payload`
/// typed per kind. Schema only (an envelope's `kind`, `kv` and `payload` have this shape); in
/// Rust the payload is an [`AnyPayload`].
pub struct EventPayload;

impl JsonSchema for EventPayload {
    fn schema_name() -> Cow<'static, str> {
        "EventPayload".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let variants = event_payload_variants(generator);
        json_schema!({ "oneOf": variants })
    }
}

/// The payload union as serialised: any one kind's payload.
impl JsonSchema for AnyPayload {
    fn schema_name() -> Cow<'static, str> {
        "AnyPayload".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let variants: Vec<Value> = event_payload_variants(generator)
            .into_iter()
            .map(|v| v["properties"]["payload"].clone())
            .collect();
        json_schema!({ "anyOf": variants })
    }
}
