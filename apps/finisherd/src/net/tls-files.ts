// The owner's certificate files (5B Decision 9, files mode: `tailscale cert`, a reverse proxy's
// pair): read with Node's `X509Certificate`, which reads every key type such files hold. A pair
// is usable when the certificate parses, the key is its key, and now lies within its validity;
// `spki` is the certificate's pin (`base64url(SHA-256(SubjectPublicKeyInfo))`), shown in
// `server.status`, never used to pin a files route (those are WebPKI routes). Neither PEM is
// ever put in a report or an error. Only the first certificate of a chain file is read: the
// intermediates are the client's to verify (a WebPKI route), and a chain written leaf-last reads
// as `keyMismatch`.
import { createHash, createPrivateKey, X509Certificate } from "node:crypto";

export type TlsFilesProblem = "certUnreadable" | "keyUnreadable" | "keyMismatch" | "expired" | "notYetValid";

export interface TlsFilesReport {
  readonly ok: boolean;
  readonly reason?: TlsFilesProblem;
  readonly spki?: string;
  /** ms since the epoch. */
  readonly notAfter?: number;
}

/** The SPKI pin of a certificate (PEM): the same string `spkiPin` gives its key. */
export function certSpki(cert: X509Certificate): string {
  const der = cert.publicKey.export({ type: "spki", format: "der" });
  return createHash("sha256").update(der).digest("base64url");
}

/** Checks the pair `certPem`/`keyPem` at `nowMs`. */
export function checkTlsFiles(certPem: string, keyPem: string, nowMs: number = Date.now()): TlsFilesReport {
  let cert: X509Certificate;
  try {
    cert = new X509Certificate(certPem);
  } catch {
    return { ok: false, reason: "certUnreadable" };
  }
  const spki = certSpki(cert);
  const notAfter = cert.validToDate.getTime();
  const facts = { spki, notAfter };
  let key;
  try {
    key = createPrivateKey(keyPem);
  } catch {
    return { ok: false, reason: "keyUnreadable", ...facts };
  }
  if (!cert.checkPrivateKey(key)) return { ok: false, reason: "keyMismatch", ...facts };
  // `notAfter` is inclusive (RFC 5280).
  if (nowMs > notAfter) return { ok: false, reason: "expired", ...facts };
  if (nowMs < cert.validFromDate.getTime()) return { ok: false, reason: "notYetValid", ...facts };
  return { ok: true, ...facts };
}
