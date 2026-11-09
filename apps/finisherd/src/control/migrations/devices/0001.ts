// devices 0001 (5B Task 1, Decision 2, spec §3.1, §5.1): paired devices, pairing offers and
// pending pairing requests. Frozen once merged: never edit, add the next step. No foreign key
// leaves the chain.
//
// - `devices.jwk`: the device's public JWK as JSON text; `jkt` its RFC 7638 thumbprint;
// - `approvedBy`: `<kind>:<id>` of the principal that approved it;
// - `lastRoute`: a route kind (`loopback|lan|tailscale|public|connect|other`), never an address
//   (owner decision 4: addresses live only in the audit log);
// - `revokedAt`/`revokedReason` (`owner | self | reset | rotation`) end it;
// - `pair_offers.secretHash`: SHA-256 of the offer's secret, base64url; the secret is never
//   stored; `self` 0 or 1; `state` `open | used | expired`;
// - `pair_pending.safetyCode` and `.remote` are nullable and written NULL: the safety code is
//   a secret and the address belongs to the audit log alone (owner decision 4), so both stay in
//   memory, and a request still pending at a restart loads as expired (the device redeems a new
//   offer); `state` `pending | approved | rejected | expired`;
// - times in ms since the epoch.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const devices0001: Migration = {
  n: 1,
  up: (db: DatabaseSync) => {
    db.exec(`
      CREATE TABLE devices (
        id TEXT PRIMARY KEY, name TEXT NOT NULL, platform TEXT NOT NULL,
        jwk TEXT NOT NULL, jkt TEXT NOT NULL UNIQUE,
        createdAt INTEGER NOT NULL, approvedBy TEXT NOT NULL,
        lastSeenAt INTEGER, lastRoute TEXT,
        revokedAt INTEGER, revokedReason TEXT) STRICT;
      CREATE TABLE pair_offers (
        id TEXT PRIMARY KEY, secretHash TEXT NOT NULL, label TEXT,
        self INTEGER NOT NULL, createdBy TEXT NOT NULL,
        createdAt INTEGER NOT NULL, expiresAt INTEGER NOT NULL, state TEXT NOT NULL) STRICT;
      CREATE TABLE pair_pending (
        id TEXT PRIMARY KEY, offerId TEXT NOT NULL, deviceName TEXT NOT NULL, platform TEXT NOT NULL,
        jwk TEXT NOT NULL, jkt TEXT NOT NULL, safetyCode TEXT,
        redeemedAt INTEGER NOT NULL, remote TEXT, expiresAt INTEGER NOT NULL, state TEXT NOT NULL) STRICT;
    `);
  },
};
