// Paired devices, pairing, the environment descriptor and its routes (spec §5; plan 5B
// Decisions 1–3, 17–20, 25–27, 30, 33). Devices are principals of kind `device`: the owner, on
// another machine or in the app, holding a P-256 key whose RFC 7638 thumbprint (`jkt`) names
// it. Nothing here ever carries a pairing secret, a token, a proof or a private key but the
// offer's own answer (`link`, `qr`) and the HTTP answers that hand a token or ticket over.
import { Schema } from "effect";

/**
 * The console's typed code (`auth.ts` `ChallengeField`), declared here too so this module stays
 * a leaf (`audit.ts` reads its closed words, and `auth.ts` reads `audit.ts`).
 */
const ChallengeField = Schema.optional(Schema.String);

/** What a device runs on (`Name(PLATFORMS)` in the journal, `fin_core::events::names`). */
export const PLATFORMS = ["macos", "windows", "linux", "ios", "android", "web", "other"] as const;
export const PlatformSchema = Schema.Literals(PLATFORMS);
export type Platform = typeof PlatformSchema.Type;

/**
 * How a device reached the server (Decision 10): a kind, never an address (owner decision 4).
 * `connect` is the `ConnectRelay` extension point (none in v1).
 */
export const ROUTE_KINDS = ["loopback", "lan", "tailscale", "public", "connect", "other"] as const;
export const RouteKindSchema = Schema.Literals(ROUTE_KINDS);
export type RouteKind = typeof RouteKindSchema.Type;

/**
 * One way to reach the server: `pin: identity` routes serve the self-signed certificate made
 * from the identity key (clients pin its SPKI); `webpki` routes serve the owner's trusted
 * certificate files (clients verify it the normal way, then check the `envId`).
 */
export const RouteSchema = Schema.Struct({
  kind: RouteKindSchema,
  url: Schema.String,
  pin: Schema.Literals(["identity", "webpki"]),
});
export type Route = typeof RouteSchema.Type;

export const ENV_STATES = ["ok", "moved"] as const;
export const EnvStateSchema = Schema.Literals(ENV_STATES);
export type EnvState = typeof EnvStateSchema.Type;

export const ProtocolRangeSchema = Schema.Struct({ min: Schema.Number, max: Schema.Number });

/**
 * `env.describe` (Decision 30): the environment a device pairs with. `spki` is
 * `base64url(SHA-256(SubjectPublicKeyInfo))` of the identity key, 43 characters; `serverIds`
 * the server ids that ran this environment's data; `state: moved` while copied or moved data
 * waits for the owner (`env.adopt` or `env.reset`, Decision 31).
 */
export const EnvironmentSchema = Schema.Struct({
  envId: Schema.String,
  name: Schema.String,
  spki: Schema.String,
  serverIds: Schema.Array(Schema.String),
  protocol: ProtocolRangeSchema,
  routes: Schema.Array(RouteSchema),
  capabilities: Schema.Array(Schema.String),
  state: EnvStateSchema,
});
export type Environment = typeof EnvironmentSchema.Type;

/** `GET /.well-known/finisher` (anonymous, spec §5.5): the descriptor's public subset. */
export const WellKnownSchema = Schema.Struct({ envId: Schema.String, protocol: ProtocolRangeSchema, spki: Schema.String });
export type WellKnown = typeof WellKnownSchema.Type;

/**
 * Device and environment names: a person's text, 1–64 UTF-16 units (the audit's `Name` limit),
 * NFC, without control or format characters (no bidi overrides or zero-width characters that
 * would make one name look like another when the owner compares it).
 */
export const DEVICE_NAME_MAX = 64;
/** Live devices at most (Decision 2): past it a pairing approval is `DEVICE_LIMIT`. */
export const MAX_LIVE_DEVICES = 64;

const CONTROL = /[\p{Cc}\p{Cf}]/u;

/** Why `name` is not a valid device or environment name, or `undefined` when it is. */
export function nameProblem(name: string): "empty" | "tooLong" | "notNfc" | "control" | undefined {
  if (name.trim().length === 0) return "empty";
  if (name.length > DEVICE_NAME_MAX) return "tooLong";
  if (name.normalize("NFC") !== name) return "notNfc";
  if (CONTROL.test(name)) return "control";
  return undefined;
}

/** A device's public key as it pairs: an EC P-256 JWK with no private member. */
export const DeviceJwkSchema = Schema.Struct({
  kty: Schema.Literal("EC"),
  crv: Schema.Literal("P-256"),
  x: Schema.String,
  y: Schema.String,
});
export type DeviceJwk = typeof DeviceJwkSchema.Type;

/**
 * A device as the owner sees it (`auth.device.list`): never its key. Times in ms since the
 * epoch; `approvedBy` is `<kind>:<id>` of the principal that approved it; `lastRoute` a kind.
 */
export const DeviceSchema = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
  platform: PlatformSchema,
  jkt: Schema.String,
  createdAt: Schema.Number,
  approvedBy: Schema.String,
  lastSeenAt: Schema.optional(Schema.Number),
  lastRoute: Schema.optional(RouteKindSchema),
  revokedAt: Schema.optional(Schema.Number),
});
export type Device = typeof DeviceSchema.Type;

/** How a device ended: by the owner, by itself, by `env.reset`, or by an identity rotation. */
export const DEVICE_END_REASONS = ["owner", "self", "reset", "rotation"] as const;
export type DeviceEndReason = (typeof DEVICE_END_REASONS)[number];

// ---- Pairing (Decisions 17–21) ----

/** The longest and the default life of an offer, in seconds; open offers at most. */
export const PAIR_OFFER_MAX_TTL_S = 600;
export const PAIR_OFFER_DEFAULT_TTL_S = 300;
export const MAX_OPEN_OFFERS = 8;
/** Pending requests at most, and how long each waits for the owner after its redeem. */
export const MAX_PENDING = 16;
export const PENDING_TTL_S = 600;

/**
 * `auth.pair.offer` (`auth.admin`): `self` only from a `console` principal, for the desktop
 * shell's own renderer (loopback only, approved at redeem, no typed code: Decision 21).
 */
export const PairOfferPayload = Schema.Struct({
  label: Schema.optional(Schema.String),
  ttlS: Schema.optional(Schema.Number),
  self: Schema.optional(Schema.Boolean),
  challenge: ChallengeField,
});
/** The link (`finisher://pair?…#<offerId>.<secret>`), its QR code as SVG, a browser link on a `webpki` route. */
export const PairOfferSuccess = Schema.Struct({
  offerId: Schema.String,
  link: Schema.String,
  browserLink: Schema.optional(Schema.String),
  qr: Schema.String,
  expiresAt: Schema.Number,
});

export const PairPendingIdPayload = Schema.Struct({ pendingId: Schema.String, challenge: ChallengeField });
export const PairApproveSuccess = Schema.Struct({ device: DeviceSchema });
export const PairRejectSuccess = Schema.Struct({ rejected: Schema.Literal(true) });

/** A pending request as the owner compares it: never the remote address (only the audit holds it). */
export const PendingViewSchema = Schema.Struct({
  pendingId: Schema.String,
  deviceName: Schema.String,
  platform: PlatformSchema,
  safetyCode: Schema.String,
  redeemedAt: Schema.Number,
  expiresAt: Schema.Number,
});
export type PendingView = typeof PendingViewSchema.Type;

export const PENDING_END_STATES = ["approved", "rejected", "expired"] as const;
/** `auth.pair.pending` (stream): the snapshot first, then each request added or ended. */
export const PairPendingItem = Schema.Union([
  Schema.Struct({ kind: Schema.Literal("snapshot"), pending: Schema.Array(PendingViewSchema) }),
  Schema.Struct({ kind: Schema.Literal("added"), pending: PendingViewSchema }),
  Schema.Struct({ kind: Schema.Literal("ended"), pendingId: Schema.String, state: Schema.Literals(PENDING_END_STATES) }),
]);
export type PairPendingItem = typeof PairPendingItem.Type;

// ---- Devices (Decision 25) ----

/** `auth.device.list {all?}`: live devices, or every row kept (`all`). */
export const DeviceListPayload = Schema.Struct({ all: Schema.optional(Schema.Boolean) });
export const DeviceListSuccess = Schema.Struct({ devices: Schema.Array(DeviceSchema) });
export const DeviceRenamePayload = Schema.Struct({ id: Schema.String, name: Schema.String, challenge: ChallengeField });
export const DeviceRenameSuccess = Schema.Struct({ device: DeviceSchema });
export const DeviceRevokePayload = Schema.Struct({ id: Schema.String, challenge: ChallengeField });
export const DeviceRevokeSuccess = Schema.Struct({ revoked: Schema.Boolean });

/** `auth.devices.watch` (stream): the snapshot first; `seen` at most once a minute per device. */
export const DevicesWatchItem = Schema.Union([
  Schema.Struct({ kind: Schema.Literal("snapshot"), devices: Schema.Array(DeviceSchema) }),
  Schema.Struct({ kind: Schema.Literal("added"), device: DeviceSchema }),
  Schema.Struct({ kind: Schema.Literal("renamed"), device: DeviceSchema }),
  Schema.Struct({ kind: Schema.Literal("revoked"), id: Schema.String }),
  Schema.Struct({ kind: Schema.Literal("seen"), id: Schema.String, at: Schema.Number, route: RouteKindSchema }),
]);
export type DevicesWatchItem = typeof DevicesWatchItem.Type;

// ---- The environment (Decisions 30, 31) ----

export const EnvNamePayload = Schema.Struct({ name: Schema.String, challenge: ChallengeField });
export const EnvConfirmPayload = Schema.Struct({ challenge: ChallengeField });
export const EnvResetSuccess = Schema.Struct({ envId: Schema.String, devices: Schema.Number, credentials: Schema.Number });
export const EnvRotateIdentitySuccess = Schema.Struct({ spki: Schema.String, revoked: Schema.Number });

// ---- Settings (Decisions 26, 27) ----

/** The `settings.json` sections `settings.write` may replace. */
export const SETTINGS_PATHS = ["server.remote", "server.projectRoots", "server.origins", "server.loopback"] as const;
export const SettingsPathSchema = Schema.Literals(SETTINGS_PATHS);
export type SettingsPath = typeof SettingsPathSchema.Type;
export const SettingsWritePayload = Schema.Struct({ path: SettingsPathSchema, value: Schema.Unknown, challenge: ChallengeField });
export const SettingsWriteSuccess = Schema.Struct({ path: SettingsPathSchema });

// ---- The HTTP front's bodies (Decisions 11, 16, 18, 20) ----

export const PairRedeemBody = Schema.Struct({
  offerId: Schema.String,
  secret: Schema.String,
  device: Schema.Struct({ name: Schema.String, platform: PlatformSchema, jwk: DeviceJwkSchema }),
});
export const PairRedeemAnswer = Schema.Struct({ pendingId: Schema.String, safetyCode: Schema.String });
export const PairStatusBody = Schema.Struct({ pendingId: Schema.String });
export const PairStatusAnswer = Schema.Struct({
  state: Schema.Literals(["pending", "approved", "rejected", "expired"]),
  deviceId: Schema.optional(Schema.String),
  environment: Schema.optional(EnvironmentSchema),
});
/** The life of an access token, and of a device's WebSocket ticket, in seconds. */
export const ACCESS_TOKEN_TTL_S = 900;
export const DEVICE_TICKET_TTL_S = 30;
export const TokenAnswer = Schema.Struct({ accessToken: Schema.String, tokenType: Schema.Literal("DPoP"), expiresIn: Schema.Number });
export const WsTicketAnswer = Schema.Struct({ ticket: Schema.String, expiresIn: Schema.Number });

// ---- Codes (Decision 33) ----

/**
 * 5B's error codes: the HTTP status an endpoint answers them with (`status`, when one does)
 * and `fin`'s exit class (`crates/fin/src/output.rs`; the console's `exit-codes.ts`). `start`
 * codes stop the server from starting; `TLS_PIN_MISMATCH` is a client's own (sub-project 9).
 */
export const DEVICE_ERROR_CODES = {
  PAIRING_EXPIRED: { status: 410, exit: 3 },
  PAIRING_USED: { status: 409, exit: 4 },
  PAIRING_REJECTED: { status: 403, exit: 7 },
  PAIRING_RATE_LIMITED: { status: 429, exit: 5 },
  TOKEN_RATE_LIMITED: { status: 429, exit: 5 },
  DPOP_INVALID: { status: 401, exit: 7 },
  DPOP_NONCE: { status: 401, exit: 7 },
  TOKEN_EXPIRED: { status: 401, exit: 7 },
  TICKET_INVALID: { status: 401, exit: 7 },
  ORIGIN_REFUSED: { status: 403, exit: 7 },
  HOST_REFUSED: { status: 403, exit: 7 },
  DEVICE_REVOKED: { status: 401, exit: 7 },
  ENV_MOVED: { status: 403, exit: 7 },
  PATH_NOT_ALLOWED: { exit: 7 },
  DEVICE_LIMIT: { status: 409, exit: 4 },
  LISTEN_NEEDS_TLS: { exit: 3, start: true },
  TLS_FILES_INVALID: { exit: 3, start: true },
  IDENTITY_MISMATCH: { exit: 3, start: true },
  SECRETS_KEY_INVALID: { exit: 3, start: true },
  TLS_PIN_MISMATCH: { exit: 7 },
} as const satisfies Record<string, { readonly status?: number; readonly exit: 3 | 4 | 5 | 7; readonly start?: true }>;
export type DeviceErrorCode = keyof typeof DEVICE_ERROR_CODES;
