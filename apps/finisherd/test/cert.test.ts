// 5B Task 3: certificates from the identity key accepted by Node's TLS, the peer's SPKI equal
// to the pin, and the files-mode checks (Decisions 6, 8, 9).
import { createSecureContext, connect, createServer, type TLSSocket } from "node:tls";
import { createHash, generateKeyPairSync, X509Certificate } from "node:crypto";
import type { AddressInfo } from "node:net";
import { generateIdentityKey, qrSvg, qrText, selfSignedCert, spkiPin } from "@finisher/fin-node";
import { describe, expect, test } from "vitest";
import { certSpki, checkTlsFiles } from "../src/net/tls-files.ts";

const DAY = 86_400_000;

describe("self-signed certificates", () => {
  test("cert_spki_is_identity_spki", () => {
    const id = generateIdentityKey();
    const pem = selfSignedCert(id.pem, ["127.0.0.2", "studio"], 397);
    const cert = new X509Certificate(pem);
    expect(certSpki(cert)).toBe(id.spki);
    expect(spkiPin(id.pem)).toBe(id.spki);
    expect(cert.subject).toBe("CN=finisher");
    expect(cert.subjectAltName).toContain("IP Address:127.0.0.2");
    expect(cert.subjectAltName).toContain("DNS:studio");
    expect(cert.subjectAltName).toContain("DNS:localhost");
    expect(cert.subjectAltName).toContain("IP Address:127.0.0.1");
    expect(cert.subjectAltName).toContain("IP Address:0:0:0:0:0:0:0:1");
    const v6 = new X509Certificate(selfSignedCert(id.pem, ["[fd7a:115c:a1e0::1]", "fe80::1%eth0", "bad host", "été.local", "studio.local."], 30));
    expect(v6.subjectAltName).toContain("IP Address:FD7A:115C:A1E0:0:0:0:0:1");
    expect(v6.subjectAltName).toContain("IP Address:FE80:0:0:0:0:0:0:1");
    expect(v6.subjectAltName).toContain("DNS:studio.local");
    // Names a certificate cannot carry are left out, never failing it.
    expect(v6.subjectAltName).not.toContain("bad host");
    expect(v6.subjectAltName).not.toContain("local, DNS:été");
    // Valid now and for 397 days, give or take the day of slack before.
    expect(cert.validToDate.getTime()).toBeGreaterThan(Date.now() + 396 * DAY);
    expect(cert.validToDate.getTime()).toBeLessThan(Date.now() + 399 * DAY);
    expect(cert.validFromDate.getTime()).toBeLessThan(Date.now());
    expect(checkTlsFiles(pem, id.pem)).toMatchObject({ ok: true, spki: id.spki });
  });

  test("remade_cert_keeps_pin", () => {
    const id = generateIdentityKey();
    const a = new X509Certificate(selfSignedCert(id.pem, ["127.0.0.2"], 397));
    const b = new X509Certificate(selfSignedCert(id.pem, ["127.0.0.3", "edit-bay"], 397));
    expect(a.serialNumber).not.toBe(b.serialNumber);
    expect(certSpki(a)).toBe(certSpki(b));
    expect(certSpki(b)).toBe(id.spki);
  });

  test("node_tls_accepts_the_pair: a loopback handshake whose peer SPKI is the pin", async () => {
    const id = generateIdentityKey();
    const cert = selfSignedCert(id.pem, [], 397);
    expect(() => createSecureContext({ key: id.pem, cert })).not.toThrow();
    const server = createServer({ key: id.pem, cert, minVersion: "TLSv1.2" }, (s) => s.end("hello"));
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
    try {
      const port = (server.address() as AddressInfo).port;
      // A pinning client: no CA, the certificate checked by its SPKI alone.
      const { pin, text } = await new Promise<{ pin: string; text: string }>((resolve, reject) => {
        const s: TLSSocket = connect({
          host: "127.0.0.1",
          port,
          servername: "localhost",
          rejectUnauthorized: false,
        });
        let text = "";
        s.on("secureConnect", () => {
          const peer = s.getPeerX509Certificate();
          const der = peer!.publicKey.export({ type: "spki", format: "der" });
          s.on("data", (d) => (text += String(d)));
          s.on("end", () => resolve({ pin: createHash("sha256").update(der).digest("base64url"), text }));
        });
        s.on("error", reject);
      });
      expect(pin).toBe(id.spki);
      expect(text).toBe("hello");
    } finally {
      await new Promise((r) => server.close(r));
    }
  });
});

describe("files mode (Decision 9)", () => {
  test("files_key_mismatch_is_invalid", () => {
    const a = generateIdentityKey();
    const b = generateIdentityKey();
    const cert = selfSignedCert(a.pem, [], 90);
    expect(checkTlsFiles(cert, b.pem)).toMatchObject({ ok: false, reason: "keyMismatch", spki: a.spki });
    // An owner's RSA key does not match either, and reads fine.
    const rsa = generateKeyPairSync("rsa", { modulusLength: 2048 }).privateKey.export({ type: "pkcs8", format: "pem" }) as string;
    expect(checkTlsFiles(cert, rsa).reason).toBe("keyMismatch");
  });

  test("files_expired_is_invalid", () => {
    const id = generateIdentityKey();
    const cert = selfSignedCert(id.pem, [], 90);
    const notAfter = new X509Certificate(cert).validToDate.getTime();
    expect(checkTlsFiles(cert, id.pem, notAfter).ok).toBe(true);
    expect(checkTlsFiles(cert, id.pem, notAfter + 1)).toMatchObject({ ok: false, reason: "expired", notAfter });
    expect(checkTlsFiles(cert, id.pem, Date.now() - 10 * DAY).reason).toBe("notYetValid");
  });

  test("unreadable files are reported without their contents", () => {
    const id = generateIdentityKey();
    const cert = selfSignedCert(id.pem, [], 90);
    expect(checkTlsFiles("not a certificate", id.pem)).toEqual({ ok: false, reason: "certUnreadable" });
    const r = checkTlsFiles(cert, "-----BEGIN PRIVATE KEY-----\nc2VjcmV0\n-----END PRIVATE KEY-----\n");
    expect(r.reason).toBe("keyUnreadable");
    expect(JSON.stringify(r)).not.toContain("c2VjcmV0");
  });
});

describe("QR codes", () => {
  test("svg and text renderings", () => {
    const link = "finisher://pair?v=1&env=01envenvenvenvenvenvenvenv#01offer.secret";
    expect(qrSvg(link)).toContain("<svg");
    expect(qrText(link).split("\n").length).toBeGreaterThan(10);
    expect(() => qrSvg("x".repeat(5000))).toThrow();
  });
});
