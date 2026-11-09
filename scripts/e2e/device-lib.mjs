// A paired device for the "remote" scenario (plan 5B Task 13), written apart from the server's
// own code: a WebCrypto P-256 key, DPoP proofs (RFC 9449) signed here as ES256 JWS, its RFC 7638
// thumbprint, the token and ticket flow over HTTPS, and a ticketed WebSocket over `wss`. Every
// TLS connection pins the identity: the peer certificate's SPKI hash must equal the pin the
// server published, so a self-signed certificate is checked, never just accepted.
import { createHash, randomBytes, webcrypto } from "node:crypto";
import { request as httpRequest } from "node:http";
import { request as httpsRequest } from "node:https";
import { createRequire } from "node:module";
import { join } from "node:path";
import { connect as tlsConnect } from "node:tls";
import { parseFrames, ROOT } from "./lib.mjs";

// `ws` is finisherd's dependency (its TLS options let the pin check run before any frame).
const WebSocket = createRequire(join(ROOT, "apps", "finisherd", "package.json"))("ws");
const subtle = webcrypto.subtle;
const b64 = (buf) => Buffer.from(buf).toString("base64url");
const sha256 = (data) => createHash("sha256").update(data).digest();

/** Every secret a device saw (the scenario's leak grep needles). */
export const deviceSecrets = new Set();

/** The pin of a TLS socket's peer: `base64url(SHA-256(SubjectPublicKeyInfo))`. */
export function pinOf(socket) {
  const cert = socket.getPeerX509Certificate?.();
  if (cert === undefined) throw new Error("the TLS peer showed no certificate");
  return b64(sha256(cert.publicKey.export({ type: "spki", format: "der" })));
}

/** A new device key: the private key, its public JWK and thumbprint. */
export async function newDeviceKey() {
  const pair = await subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
  const full = await subtle.exportKey("jwk", pair.privateKey);
  deviceSecrets.add(full.d);
  const jwk = { kty: "EC", crv: "P-256", x: full.x, y: full.y };
  // RFC 7638: the required members in lexicographic order, no spaces.
  const jkt = b64(sha256(JSON.stringify({ crv: jwk.crv, kty: jwk.kty, x: jwk.x, y: jwk.y })));
  return { privateKey: pair.privateKey, jwk, jkt };
}

/** A DPoP proof JWT for `htm htu`, with the server's nonce and the token's `ath` when given. */
export async function dpopProof(key, htm, htu, { nonce, accessToken } = {}) {
  const header = { typ: "dpop+jwt", alg: "ES256", jwk: key.jwk };
  const payload = {
    jti: b64(randomBytes(16)),
    htm,
    htu,
    iat: Math.floor(Date.now() / 1000),
    ...(nonce === undefined ? {} : { nonce }),
    ...(accessToken === undefined ? {} : { ath: b64(sha256(accessToken)) }),
  };
  const input = `${b64(JSON.stringify(header))}.${b64(JSON.stringify(payload))}`;
  // WebCrypto's ECDSA signature is already JWS's `r || s` (IEEE P1363).
  const sig = await subtle.sign({ name: "ECDSA", hash: "SHA-256" }, key.privateKey, Buffer.from(input));
  const jwt = `${input}.${b64(sig)}`;
  deviceSecrets.add(jwt);
  return jwt;
}

/**
 * A TLS connection to `port` on loopback whose peer's pin is checked on `secureConnect`, before
 * anything is written to it: a peer with another pin never sees a secret. `pin: undefined` only
 * reads the pin (the first contact, step 1); it resolves `{socket, pin}`.
 */
export function pinnedSocket(port, pin) {
  return new Promise((resolve, reject) => {
    const socket = tlsConnect({ host: "127.0.0.1", port, servername: "localhost", rejectUnauthorized: false });
    socket.once("error", reject);
    socket.once("secureConnect", () => {
      let seen;
      try {
        seen = pinOf(socket);
      } catch (e) {
        socket.destroy();
        return reject(e);
      }
      if (pin !== undefined && seen !== pin) {
        socket.destroy();
        return reject(new Error(`TLS pin mismatch: ${seen} is not ${pin}`));
      }
      socket.off("error", reject);
      resolve({ socket, pin: seen });
    });
  });
}

/**
 * An HTTPS request to `port` on loopback over a pinned socket (`pinnedSocket`); resolves
 * `{status, body, headers, pin}`.
 */
export async function httpsCall(port, method, path, { body, headers = {}, pin } = {}) {
  const { socket, pin: seen } = await pinnedSocket(port, pin);
  return new Promise((resolve, reject) => {
    const req = httpsRequest(
      {
        host: "127.0.0.1",
        port,
        method,
        path,
        createConnection: () => socket,
        headers: { host: `127.0.0.1:${port}`, connection: "close", ...(body === undefined ? {} : { "content-type": "application/json" }), ...headers },
      },
      (res) => {
        let text = "";
        res.setEncoding("utf8").on("data", (d) => (text += d));
        res.on("end", () => resolve({ status: res.statusCode ?? 0, body: text === "" ? {} : JSON.parse(text), headers: res.headers, pin: seen }));
      },
    );
    req.on("error", reject);
    req.end(body === undefined ? undefined : JSON.stringify(body));
  });
}

/** A plain HTTP request to the loopback listener (a device there: no TLS, no pin). */
export function httpCall(port, method, path, { body, headers = {} } = {}) {
  return new Promise((resolve, reject) => {
    const req = httpRequest({ host: "127.0.0.1", port, method, path, headers: { host: `127.0.0.1:${port}`, ...(body === undefined ? {} : { "content-type": "application/json" }), ...headers } }, (res) => {
      let text = "";
      res.setEncoding("utf8").on("data", (d) => (text += d));
      res.on("end", () => resolve({ status: res.statusCode ?? 0, body: text === "" ? {} : JSON.parse(text), headers: res.headers }));
    });
    req.on("error", reject);
    req.end(body === undefined ? undefined : JSON.stringify(body));
  });
}

/**
 * A device paired with the server on `port`: its `wss` listener pinned to `pin`, or with `pin:
 * null` the loopback listener over plain HTTP.
 */
export class RemoteDevice {
  constructor(port, pin, key) {
    this.port = port;
    this.pin = pin;
    this.key = key;
    this.nonce = undefined;
  }

  url(path) {
    return `${this.pin === null ? "http" : "https"}://127.0.0.1:${this.port}${path}`;
  }

  /** A DPoP request; with `retry`, a `use_dpop_nonce` answer is retried once with the nonce. */
  async call(path, body, { accessToken, retry = true } = {}) {
    for (let attempt = 0; ; attempt++) {
      const dpop = await dpopProof(this.key, "POST", this.url(path), { nonce: this.nonce, accessToken });
      const headers = { dpop, ...(accessToken === undefined ? {} : { authorization: `DPoP ${accessToken}` }) };
      const got = this.pin === null ? await httpCall(this.port, "POST", path, { body, headers }) : await httpsCall(this.port, "POST", path, { body, headers, pin: this.pin });
      const n = got.headers["dpop-nonce"];
      if (typeof n === "string") {
        this.nonce = n;
        deviceSecrets.add(n);
      }
      const again = got.status === 401 && String(got.headers["www-authenticate"] ?? "").includes("use_dpop_nonce");
      if (!retry || !again || attempt >= 1) return got;
    }
  }

  async token() {
    const got = await this.call("/auth/token", {});
    if (got.status !== 200) throw Object.assign(new Error(`token: ${got.status} ${JSON.stringify(got.body)}`), { result: got });
    deviceSecrets.add(got.body.accessToken);
    return got.body.accessToken;
  }

  async ticket(token) {
    const got = await this.call("/auth/ws-ticket", {}, { accessToken: token ?? (await this.token()) });
    if (got.status !== 200) throw Object.assign(new Error(`ticket: ${got.status} ${JSON.stringify(got.body)}`), { result: got });
    deviceSecrets.add(got.body.ticket);
    return got.body.ticket;
  }

  /** A ticketed WebSocket over `wss`, pinned; a minimal Effect-RPC client over it. */
  async ws() {
    const ticket = await this.ticket();
    // The pin is checked on the socket before the upgrade (and its ticket) is written.
    const { socket } = await pinnedSocket(this.port, this.pin);
    const sock = new WebSocket(`wss://127.0.0.1:${this.port}/rpc`, ["finisher.v2", `finisher.ticket.${ticket}`], {
      createConnection: () => socket,
      headers: { host: `127.0.0.1:${this.port}` },
    });
    await new Promise((resolve, reject) => {
      sock.once("open", resolve);
      sock.once("error", reject);
    });
    // A later socket error ends the connection (`closed`), never the run.
    sock.on("error", () => undefined);
    let nextId = 1;
    const exits = new Map();
    const chunks = new Map();
    const closed = new Promise((resolve) =>
      sock.once("close", () => {
        for (const done of exits.values()) done({ _tag: "Closed" });
        exits.clear();
        resolve(performance.now());
      }),
    );
    sock.on("message", (data) => {
      for (const frame of parseFrames(String(data))) {
        if (frame._tag === "Chunk") {
          chunks.get(frame.requestId)?.(frame.values);
          sock.send(`${JSON.stringify({ _tag: "Ack", requestId: frame.requestId })}\n`);
        } else if (frame._tag === "Exit") {
          chunks.delete(frame.requestId);
          exits.get(frame.requestId)?.(frame.exit);
          exits.delete(frame.requestId);
        }
      }
    });
    const send = (tag, payload, onValues) => {
      const id = String(nextId++);
      if (onValues) chunks.set(id, onValues);
      const done = new Promise((resolve) => exits.set(id, resolve));
      sock.send(`${JSON.stringify({ _tag: "Request", id, tag, payload, headers: [] })}\n`);
      return { id, done };
    };
    return {
      /** The call's exit (`{_tag: "Success", value}` or a failure). */
      exit: (tag, payload) => send(tag, payload).done,
      subscribe: (payload, onValue) => send("ops.subscribe", payload, (values) => values.forEach(onValue)),
      closed,
      close: () => sock.close(),
    };
  }
}

/** Redeems `link` (`finisher://pair?…#<offerId>.<secret>`) over `port` as a new device: the HTTP answer and the key. */
export async function redeem(port, pin, link, name, key) {
  const [offerId, secret] = link.slice(link.indexOf("#") + 1).split(".");
  deviceSecrets.add(secret);
  deviceSecrets.add(link.slice(link.indexOf("#") + 1));
  const got = await httpsCall(port, "POST", "/pair/redeem", { body: { offerId, secret, device: { name, platform: "android", jwk: key.jwk } }, pin });
  return got;
}
