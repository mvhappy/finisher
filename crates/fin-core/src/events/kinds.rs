//! The closed kind registry and payload enum (plan D, Decision 3). One table, [`registry!`],
//! lists every kind once with its version, flags, sources and payload type, and generates
//! [`KINDS`], [`AnyPayload`] (one variant per kind, reserved ones included), the typed decodes
//! of [`decode_payload`] (a stored payload) and [`validate_payload`] (a received one, through
//! its wire form when a remote source may send the kind) and the kind-keyed schema of
//! [`EventPayload`].
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
use super::class::{BlobRef, ScrubCtx};
use super::payloads::{self as p, body_blobs};
use super::wire::{self as w, WireForm};
use super::wire_agent as wa;
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

/// A payload type: serialisable, decodable, schema'd, sized, and able to list its blobs,
/// scrub its [`super::class::Scrubbed`] fields again and drop its content.
pub trait Payload: Serialize + DeserializeOwned + JsonSchema + ApproxBytes {
    /// The [`BlobRef`]s in its content fields.
    fn blobs(&self) -> Vec<&BlobRef> {
        Vec::new()
    }

    /// Scrubs every [`super::class::Scrubbed`] field again with `ctx` (a received payload).
    fn rescrub(&mut self, _ctx: &ScrubCtx) {}

    /// Sets every content field to `None`: the typed twin of [`super::redact::redact`]
    /// without training (`strip_content_matches_redact` checks them equal).
    fn strip_content(&mut self) {}
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
    (@wire_name $name:literal $wire:ty) => { $name };
    (@recv $ty:ty, $p:ident, $k:ident, $c:ident) => {
        <$ty as serde::Deserialize>::deserialize($p).map(|mut x| {
            Payload::rescrub(&mut x, $c);
            AnyPayload::from(x)
        })
    };
    (@recv $ty:ty, $p:ident, $k:ident, $c:ident, $wire:ty) => {
        <$wire as serde::Deserialize>::deserialize($p)
            .map(|x| AnyPayload::from(WireForm::into_payload(x, $k, $c)))
    };
    ($($variant:ident($ty:ty) = $name:literal v $version:literal $(training_only $t:tt)?
        $(via $wire:ty)? [$($src:ident),*];)*) => {
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

            /// Drops every content field (typed [`super::redact::redact`] without training),
            /// so the writer can serialise an [`super::EnvelopeRef`] without a `Value`.
            pub fn strip_content(&mut self) {
                match self { $(AnyPayload::$variant(x) => Payload::strip_content(&mut **x),)* }
            }

            /// Scrubs every [`super::class::Scrubbed`] field again with `ctx`.
            pub fn rescrub(&mut self, ctx: &ScrubCtx) {
                match self { $(AnyPayload::$variant(x) => Payload::rescrub(&mut **x, ctx),)* }
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

        fn decode_received(
            spec: &KindSpec,
            payload: &Value,
            key: &dyn Fn(&[u8]) -> [u8; 16],
            ctx: &ScrubCtx,
        ) -> Option<Result<AnyPayload, serde_json::Error>> {
            match (spec.name, spec.version) {
                $(($name, $version) => Some(registry!(@recv $ty, payload, key, ctx $(, $wire)?)),)*
                _ => None,
            }
        }

        /// The kinds whose received payload is their wire form ([`super::wire`]).
        pub static WIRE_KINDS: &[&str] = &[$($(registry!(@wire_name $name $wire),)?)*];

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
    CliCommand(p::CliCommand) = "cli.command" v 1 via w::CliCommandIn [Fin, Mcp];
    RpcStats(p::RpcStats) = "rpc.stats" v 1 [Finisherd];
    EvalStats(p::EvalStats) = "eval.stats" v 1 [FinNode];
    FontsFetch(p::FontsFetch) = "fonts.fetch" v 1 [FinNode];
    StorageState(p::StorageState) = "storage.state" v 1 [FinNode];
    PluginLifecycle(p::PluginLifecycle) = "plugin.lifecycle" v 1 [FinNode];
    RegistryRefresh(p::RegistryRefresh) = "registry.refresh" v 1 [FinNode];
    ErrorInternal(p::ErrorInternal) = "error.internal" v 1 [FinNode, Finisherd];
    ErrorPanic(p::ErrorPanic) = "error.panic" v 1 via w::ErrorPanicIn [Fin, Mcp, Finisherd, FinNode];
    JobDone(p::JobDone) = "job.done" v 1 [Finisherd];
    WorkerExit(p::WorkerExit) = "worker.exit" v 1 [Finisherd];
    AgentSession(p::AgentSession) = "agent.session" v 1 via wa::AgentSessionIn [Agent];
    AgentTurn(p::AgentTurn) = "agent.turn" v 1 training_only true via wa::AgentTurnIn [Agent];
    AgentToolCall(p::AgentToolCall) = "agent.toolCall" v 1 training_only true via wa::AgentToolCallIn [Agent];
    AgentToolResult(p::AgentToolResult) = "agent.toolResult" v 1 training_only true via wa::AgentToolResultIn [Agent];
    AgentApproval(p::AgentApproval) = "agent.approval" v 1 [Agent];
    UiAction(p::UiAction) = "ui.action" v 0 [Ui];
    ReviewVerdict(p::ReviewVerdict) = "review.verdict" v 0 [Ui];
}

impl Payload for p::SessionStart {}
impl Payload for p::SessionEnd {}
impl Payload for p::SessionUnclean {}
impl Payload for p::ConsentChanged {}
impl Payload for p::JournalDropped {}
impl Payload for p::LogCommit {
    fn strip_content(&mut self) {
        self.intent = None;
    }
}
impl Payload for p::CliCommand {}
impl Payload for p::RpcStats {}
impl Payload for p::EvalStats {}
impl Payload for p::FontsFetch {}
impl Payload for p::StorageState {}
impl Payload for p::PluginLifecycle {}
impl Payload for p::RegistryRefresh {}
impl Payload for p::ErrorInternal {}
impl Payload for p::JobDone {}
impl Payload for p::WorkerExit {}
impl Payload for p::ErrorPanic {
    fn rescrub(&mut self, ctx: &ScrubCtx) {
        if let Some(m) = &mut self.message {
            m.0 = m.0.rescrub(ctx);
        }
    }

    fn strip_content(&mut self) {
        self.message = None;
    }
}
impl Payload for p::AgentSession {}
impl Payload for p::UiAction {}
impl Payload for p::ReviewVerdict {}

impl Payload for p::AgentTurn {
    fn blobs(&self) -> Vec<&BlobRef> {
        body_blobs(&[
            self.messages_delta.as_ref(),
            self.output.as_ref(),
            self.system.as_ref(),
            self.tools.as_ref(),
        ])
    }

    fn strip_content(&mut self) {
        self.messages_delta = None;
        self.output = None;
        self.system = None;
        self.tools = None;
    }
}

impl Payload for p::AgentApproval {
    fn blobs(&self) -> Vec<&BlobRef> {
        body_blobs(&[self.subject.as_ref()])
    }

    fn strip_content(&mut self) {
        self.subject = None;
    }
}

impl Payload for p::AgentToolCall {
    fn blobs(&self) -> Vec<&BlobRef> {
        body_blobs(&[self.args.as_ref()])
    }

    fn strip_content(&mut self) {
        self.args = None;
    }
}

impl Payload for p::AgentToolResult {
    fn blobs(&self) -> Vec<&BlobRef> {
        body_blobs(&[self.output.as_ref()])
    }

    fn strip_content(&mut self) {
        self.output = None;
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

fn unknown(spec: &KindSpec) -> CoreError {
    require_kind(spec.name, spec.version)
        .err()
        .unwrap_or_else(|| CoreError::new("EVENT_KIND_UNKNOWN", "unknown event kind"))
}

fn invalid(spec: &KindSpec, e: serde_json::Error) -> CoreError {
    CoreError::new(
        "EVENT_INVALID",
        format!("the {} payload does not decode: {e}", spec.name),
    )
    .with_details(json!({ "kind": spec.name, "kv": spec.version }))
}

/// Decodes a stored payload (a line read back from disk) as `spec`'s type. A typed decode (no
/// schema engine): the class types check their patterns, a `Name` reads through its table,
/// unknown fields are dropped. `EVENT_KIND_UNKNOWN` when `spec` is not a registry kind,
/// `EVENT_INVALID` when the payload does not decode.
pub fn decode_payload(spec: &KindSpec, payload: &Value) -> Result<AnyPayload, CoreError> {
    decode(spec, payload)
        .ok_or_else(|| unknown(spec))?
        .map_err(|e| invalid(spec, e))
}

/// Decodes a received payload (`diagnostics.record`): the only way a remote `Value` becomes a
/// payload. A kind a remote source may send ([`WIRE_KINDS`]) is decoded as its wire form and
/// converted ([`super::wire`]: raw ids and `argsHash` keyed with `key`, the message scrubbed
/// with `ctx` and hashed); any other is decoded as stored and its
/// [`super::class::Scrubbed`] fields scrubbed again with `ctx`. Errors as
/// [`decode_payload`]'s.
pub fn validate_payload(
    spec: &KindSpec,
    payload: &Value,
    key: &dyn Fn(&[u8]) -> [u8; 16],
    ctx: &ScrubCtx,
) -> Result<AnyPayload, CoreError> {
    decode_received(spec, payload, key, ctx)
        .ok_or_else(|| unknown(spec))?
        .map_err(|e| invalid(spec, e))
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
