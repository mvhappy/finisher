// Self-signed certificates and QR codes (5B Task 3) over crates/fin-node/src/tls.rs.
import { native, sync } from "./native.js";

/** A self-signed certificate (PEM) from the identity key's PEM, for `sans`, valid `notAfterDays` (≤ 397). */
export function selfSignedCert(identityPem, sans, notAfterDays) {
  return sync(() => native.selfSignedCert(identityPem, sans, notAfterDays));
}

/** The QR code of `text` as an SVG document. */
export function qrSvg(text) {
  return sync(() => native.qrSvg(text));
}

/** The QR code of `text` as Unicode half blocks (a terminal). */
export function qrText(text) {
  return sync(() => native.qrText(text));
}
