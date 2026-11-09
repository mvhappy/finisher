//! `auth.pair` (5B plan Decision 33, control-plane spec §5.2): one per pairing step. Emitted by
//! `finisherd`. Metadata only: no device name, no address, no offer or pending id, no safety
//! code, never a secret; the platform and the route are closed words (the same lists as
//! `@finisher/contracts` `PLATFORMS` and `ROUTE_KINDS`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::events::approx::approx_struct;

/// `auth.pair` (`kv` 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuthPair {
    pub outcome: PairOutcome,
    /// The device's platform, once a device redeemed the offer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<PairPlatform>,
    /// How the device reached the server, once it redeemed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<PairRoute>,
    /// From the redeem to the owner's answer (`approved`, `rejected`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub micros: Option<u64>,
}
approx_struct!(AuthPair {
    outcome,
    platform,
    route,
    micros
});

/// A pairing step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PairOutcome {
    Offered,
    Redeemed,
    Approved,
    Rejected,
    Expired,
    Revoked,
}

/// What a device runs on (`PLATFORMS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PairPlatform {
    Macos,
    Windows,
    Linux,
    Ios,
    Android,
    Web,
    Other,
}

/// How a device reached the server (`ROUTE_KINDS`): a kind, never an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PairRoute {
    Loopback,
    Lan,
    Tailscale,
    Public,
    Connect,
    Other,
}
