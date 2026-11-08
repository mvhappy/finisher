// A paired device for the tests (5B Tasks 8–9): a P-256 key, DPoP proofs with the server's
// nonce (retried once on `use_dpop_nonce`), the token and ticket flow over the loopback
// listener's HTTP front, and a WebSocket with the device's ticket.
import { request } from "node:http";
import { calculateJwkThumbprint, exportJWK, generateKeyPair, SignJWT } from "jose";
import { WebSocket } from "ws";
import { athOf } from "../../src/auth/dpop.ts";
import { confirmed, connectProved, ok, type ServerInfo } from "./ndjson-client.ts";
import { openWs, type WsClient } from "./ws-client.ts";

export interface HttpResult {
  readonly status: number;
  readonly body: Record<string, unknown>;
  readonly headers: Record<string, string | string[] | undefined>;
}

export function post(port: number, path: string, body: unknown, headers: Record<string, string> = {}): Promise<HttpResult> {
  return new Promise((resolve, reject) => {
    const req = request({ host: "127.0.0.1", port, method: "POST", path, headers: { host: `127.0.0.1:${port}`, "content-type": "application/json", ...headers } }, (res) => {
      let t = "";
      res.on("data", (d) => (t += d));
      res.on("end", () => resolve({ status: res.statusCode ?? 0, body: t === "" ? {} : JSON.parse(t), headers: res.headers }));
    });
    req.on("error", reject);
    req.end(JSON.stringify(body));
  });
}

export async function deviceKey() {
  const { privateKey, publicKey } = await generateKeyPair("ES256", { extractable: true });
  const jwk = await exportJWK(publicKey);
  const pub = { kty: "EC" as const, crv: "P-256" as const, x: jwk.x!, y: jwk.y! };
  return { privateKey, jwk: pub, jkt: await calculateJwkThumbprint(pub, "sha256") };
}

export type DeviceKey = Awaited<ReturnType<typeof deviceKey>>;

let jti = 0;

export class TestDevice {
  nonce: string | undefined;
  readonly port: number;
  readonly key: DeviceKey;
  constructor(port: number, key: DeviceKey) {
    this.port = port;
    this.key = key;
  }

  async proof(path: string, accessToken?: string): Promise<string> {
    return new SignJWT({
      jti: `test-${process.pid}-${jti++}`,
      htm: "POST",
      htu: `http://127.0.0.1:${this.port}${path}`,
      iat: Math.floor(Date.now() / 1000),
      ...(this.nonce === undefined ? {} : { nonce: this.nonce }),
      ...(accessToken === undefined ? {} : { ath: athOf(accessToken) }),
    })
      .setProtectedHeader({ typ: "dpop+jwt", alg: "ES256", jwk: this.key.jwk })
      .sign(this.key.privateKey);
  }

  /** A DPoP request; on `use_dpop_nonce` it retries once with the server's nonce. */
  async call(path: string, body: unknown, accessToken?: string): Promise<HttpResult> {
    for (let attempt = 0; ; attempt++) {
      const headers: Record<string, string> = { dpop: await this.proof(path, accessToken), ...(accessToken === undefined ? {} : { authorization: `DPoP ${accessToken}` }) };
      const got = await post(this.port, path, body, headers);
      const n = got.headers["dpop-nonce"];
      if (typeof n === "string") this.nonce = n;
      const retry = got.status === 401 && String(got.headers["www-authenticate"] ?? "").includes("use_dpop_nonce");
      if (!retry || attempt >= 1) return got;
    }
  }

  async token(): Promise<string> {
    const got = await this.call("/auth/token", {});
    if (got.status !== 200) throw Object.assign(new Error(`token: ${got.status} ${JSON.stringify(got.body)}`), { result: got });
    return got.body.accessToken as string;
  }

  async ticket(token?: string): Promise<string> {
    const got = await this.call("/auth/ws-ticket", {}, token ?? (await this.token()));
    if (got.status !== 200) throw Object.assign(new Error(`ticket: ${got.status} ${JSON.stringify(got.body)}`), { result: got });
    return got.body.ticket as string;
  }

  /** A WebSocket with a fresh device ticket; `raw` is the socket (its close is the revocation's). */
  async ws(): Promise<WsClient> {
    return openWs(this.port, ["finisher.v2", `finisher.ticket.${await this.ticket()}`]);
  }

  async rawWs(): Promise<WebSocket> {
    const ticket = await this.ticket();
    return new Promise((resolve, reject) => {
      const sock = new WebSocket(`ws://127.0.0.1:${this.port}/rpc`, ["finisher.v2", `finisher.ticket.${ticket}`]);
      sock.once("open", () => resolve(sock));
      sock.once("error", reject);
    });
  }
}

/** Pairs a new device through the console and the HTTP front: offer, redeem, approve. */
export async function pairDevice(server: { info: ServerInfo; serverData: string }, name = "Test tablet"): Promise<{ device: TestDevice; deviceId: string }> {
  const c = await connectProved(server, "console");
  try {
    const offer = ok<{ link: string }>(await confirmed(c, "auth.pair.offer", {}));
    const [offerId, secret] = offer.link.slice(offer.link.indexOf("#") + 1).split(".");
    const key = await deviceKey();
    const redeemed = await post(server.info.wsPort, "/pair/redeem", { offerId, secret, device: { name, platform: "android", jwk: key.jwk } });
    if (redeemed.status !== 200) throw new Error(`redeem: ${redeemed.status} ${JSON.stringify(redeemed.body)}`);
    const approved = ok<{ device: { id: string } }>(await confirmed(c, "auth.pair.approve", { pendingId: redeemed.body.pendingId }));
    return { device: new TestDevice(server.info.wsPort, key), deviceId: approved.device.id };
  } finally {
    await c.close();
  }
}
