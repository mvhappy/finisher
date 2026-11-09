// 5B Task 12: the console's pairing, device, remote, tls and env commands through an injected
// person (`support/person.ts`) against a server in this process: their text and `--json`, the
// confirmations (y/N; a device's or the environment's name typed back for `revoke`, `tls rotate`
// and `env reset`), `CONSOLE_NEEDS_TTY` for every mutation, `pair` approving and rejecting, the
// QR code only when it fits, and `remote enable` refusing a listener without TLS before asking.
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { runConsole } from "../src/console/main.ts";
import type { Prompt } from "../src/console/prompt.ts";
import { serve, StderrLogger } from "../src/main.ts";
import { pairDevice, redeemLink } from "./support/device.ts";
import { confirmed, connectProved, ok, type ServerInfo } from "./support/ndjson-client.ts";
import { type Person, person } from "./support/person.ts";

let tmp: string;
let fiber: Fiber.Fiber<unknown, unknown> | undefined;
let server: { info: ServerInfo; serverData: string };

beforeEach(async () => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-console-remote-"));
  const serverData = join(tmp, "sd");
  mkdirSync(serverData, { mode: 0o700 });
  let ready!: (i: ServerInfo) => void;
  const info = new Promise<ServerInfo>((r) => (ready = r));
  fiber = Effect.runFork(serve({ serverData, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)));
  server = { info: await info, serverData };
});

afterEach(async () => {
  if (fiber !== undefined) await Effect.runPromise(Fiber.interrupt(fiber));
  fiber = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

const NOT_A_TERMINAL: Prompt = { isInteractive: () => false, show: () => undefined, ask: async () => "" };

async function run(argv: string[], prompt: Prompt = person(), opts: { columns?: number; onOut?: (line: string) => void } = {}) {
  let out = "";
  let err = "";
  const code = await runConsole([...argv, "--server-data", server.serverData], {
    prompt,
    output: {
      out: (t) => {
        out += `${t}\n`;
        opts.onOut?.(t);
      },
      err: (t) => void (err += `${t}\n`),
    },
    columns: () => opts.columns ?? 200,
  });
  return { code, out, err };
}

const envName = async () => {
  const c = await connectProved(server, "console");
  try {
    return ok<{ name: string; spki: string; envId: string }>(await c.call("env.describe", {}));
  } finally {
    await c.close();
  }
};

describe("read-only commands", () => {
  test("devices: text and --json", async () => {
    expect((await run(["devices"], NOT_A_TERMINAL)).out).toBe("No paired devices.\n");
    const { deviceId } = await pairDevice(server, "Edit tablet");
    const text = await run(["devices"], NOT_A_TERMINAL);
    expect(text.code).toBe(0);
    expect(text.out).toContain(deviceId);
    expect(text.out).toContain('"Edit tablet"  android');
    const json = JSON.parse((await run(["devices", "--json"], NOT_A_TERMINAL)).out) as { devices: Array<{ id: string }> };
    expect(json.devices.map((d) => d.id)).toEqual([deviceId]);
  });

  test("tls show, env show and remote status: text and --json", async () => {
    const e = await envName();
    const tls = await run(["tls", "show"], NOT_A_TERMINAL);
    expect(tls.out).toContain(`Identity pin: ${e.spki.slice(0, 4)} ${e.spki.slice(4, 8)}`);
    expect(JSON.parse((await run(["tls", "show", "--json"], NOT_A_TERMINAL)).out)).toMatchObject({ spki: e.spki });
    const shown = await run(["env", "show"], NOT_A_TERMINAL);
    expect(shown.out).toContain(`Environment "${e.name}": ${e.envId}`);
    expect(shown.out).toContain("State: ok");
    expect(JSON.parse((await run(["env", "show", "--json"], NOT_A_TERMINAL)).out)).toMatchObject({ envId: e.envId, state: "ok" });
    expect((await run(["remote", "status"], NOT_A_TERMINAL)).out).toBe("Remote access is off.\n");
    expect(JSON.parse((await run(["remote", "status", "--json"], NOT_A_TERMINAL)).out)).toEqual({ enabled: false });
  });
});

describe("mutations", () => {
  test("every mutation needs a terminal (CONSOLE_NEEDS_TTY, exit 7), before anything changes", async () => {
    for (const argv of [
      ["pair"],
      ["revoke", "01device"],
      ["remote", "enable", "--listen", "127.0.0.1:0", "--tls", "self-signed"],
      ["remote", "disable"],
      ["tls", "rotate"],
      ["env", "name", "Studio"],
      ["env", "adopt"],
      ["env", "reset"],
    ]) {
      const r = await run(argv, NOT_A_TERMINAL);
      expect([r.code, r.err.split("\n")[0]], argv.join(" ")).toEqual([7, expect.stringContaining("CONSOLE_NEEDS_TTY")]);
    }
    expect((await envName()).name).toBe("Finisher");
  });

  test("revoke types the device's name back; a y is not it", async () => {
    const { deviceId } = await pairDevice(server, "Edit tablet");
    const refused = await run(["revoke", deviceId], person("y", "", ""));
    expect(refused.code).not.toBe(0);
    expect(refused.err).toContain("CONFIRMATION_REQUIRED");
    expect((await run(["devices", "--json"], NOT_A_TERMINAL)).out).toContain(deviceId);
    const p = person();
    const done = await run(["revoke", deviceId], p);
    expect(done).toMatchObject({ code: 0, out: `Revoked "Edit tablet" (${deviceId}).\n` });
    expect(p.shown).toContain("To confirm, type the device's name: Edit tablet");
    expect((await run(["devices"], NOT_A_TERMINAL)).out).toBe("No paired devices.\n");
    expect((await run(["revoke", deviceId], person())).out).toBe(`No live device ${deviceId}: nothing changed.\n`);
  });

  test("a name with surrounding spaces is typed back trimmed", async () => {
    const { deviceId } = await pairDevice(server, " Spaced ");
    const p = person();
    expect((await run(["revoke", deviceId], p)).code).toBe(0);
    expect(p.shown).toContain("To confirm, type the device's name: Spaced");
  });

  test("env name and adopt are y/N; env reset and tls rotate take the environment's name, not a y", async () => {
    expect((await run(["env", "name", "Studio"], person("n"))).code).not.toBe(0);
    const named = await run(["env", "name", "Studio"], person("y"));
    expect(named).toMatchObject({ code: 0, out: 'The environment is now "Studio".\n' });
    expect((await run(["env", "adopt"], person("y"))).code).toBe(0);
    const before = await envName();
    await pairDevice(server);
    for (const argv of [["tls", "rotate"], ["env", "reset"]]) {
      const r = await run(argv, person("y", "", ""));
      expect(r.err, argv.join(" ")).toContain("CONFIRMATION_REQUIRED");
    }
    expect((await envName()).spki).toBe(before.spki);
    const p = person();
    const rotated = await run(["tls", "rotate"], p);
    expect(rotated.code).toBe(0);
    expect(p.shown).toContain("To confirm, type the environment's name: Studio");
    expect(rotated.out).toMatch(/Revoked 1 device\(s\)/);
    expect((await envName()).spki).not.toBe(before.spki);
    const reset = await run(["env", "reset"], person());
    expect(reset.code, reset.err).toBe(0);
    expect(reset.out).toMatch(/^New environment \S+: deleted 0 device\(s\)/);
    expect(reset.out).not.toContain(before.envId);
  });

  test("remote enable refuses a listener off this computer without TLS before asking; enable, status, disable", async () => {
    const p = person();
    const refused = await run(["remote", "enable", "--listen", "0.0.0.0:7311"], p);
    expect(refused.code).not.toBe(0);
    expect(refused.err).toContain("LISTEN_NEEDS_TLS");
    expect(p.asked).toEqual([]);
    const enabled = await run(["remote", "enable", "--listen", "127.0.0.1:0", "--tls", "self-signed"], p);
    expect(enabled.code, enabled.err).toBe(0);
    expect(p.shown.join("\n")).toContain("an SSH tunnel on the same port");
    expect(enabled.out).toMatch(/^Remote access is on: wss:\/\/127\.0\.0\.1:\d+ \(self-signed, from settings\)\.\nPin: /);
    expect(JSON.parse((await run(["remote", "status", "--json"], NOT_A_TERMINAL)).out)).toMatchObject({ enabled: true, listener: { transport: "wss", mode: "selfSigned" } });
    expect((await run(["remote", "disable"], person())).out).toBe("Remote access is off.\n");
    // Certificate files that are not there: refused by the server, nothing written.
    const files = await run(["remote", "enable", "--listen", "127.0.0.1:0", "--cert", join(tmp, "c.pem"), "--key", join(tmp, "k.pem")], person());
    expect(files.err).toContain("TLS_FILES_INVALID");
    expect(JSON.parse((await run(["remote", "status", "--json"], NOT_A_TERMINAL)).out)).toMatchObject({ enabled: false, settings: { enabled: false } });
    for (const argv of [["--tls", "self-signed", "--cert", "/c"], ["--cert", "/c"], ["--tls", "other"]]) {
      expect((await run(["remote", "enable", ...argv], person())).err, argv.join(" ")).toContain("USAGE");
    }
    expect((await run(["remote", "status"], NOT_A_TERMINAL)).out).toBe("Remote access is off.\n");
  });
});

describe("pair", () => {
  /** A person who confirms the offer, then answers each request with the next pair of answers. */
  const answering = (...answers: string[]): Person => person("y", ...answers);

  test("approves one request and rejects another, comparing safety codes", async () => {
    // A request already waiting from another offer: in `auth.pair.pending`'s snapshot.
    const c = await connectProved(server, "console");
    const other = ok<{ link: string }>(await confirmed(c, "auth.pair.offer", {}));
    await c.close();
    await redeemLink(server.info.wsPort, other.link, "Old phone");
    const p = answering("n", "y", "y");
    let redeeming: Promise<string> | undefined;
    const r = await run(["pair", "--label", "Studio tablet", "--ttl", "2m"], p, {
      onOut: (line) => {
        if (line.startsWith("finisher://")) redeeming = redeemLink(server.info.wsPort, line, "New tablet");
      },
    });
    await redeeming;
    expect(r.code, r.err).toBe(0);
    expect(r.out).toMatch(/^Pairing link \(one use, until .+\):\nfinisher:\/\/pair\?/);
    expect(p.shown.filter((s) => s.startsWith("A device asks to pair"))).toEqual(['A device asks to pair: "Old phone" (android).', 'A device asks to pair: "New tablet" (android).']);
    expect(p.shown.filter((s) => s.startsWith("Its safety code: ")).every((s) => /^Its safety code: \S+\. /.test(s))).toBe(true);
    expect(r.out).toContain('Rejected "Old phone".');
    expect(r.out).toMatch(/Paired "New tablet" as device \w+\./);
    const list = JSON.parse((await run(["devices", "--json"], NOT_A_TERMINAL)).out) as { devices: Array<{ name: string }> };
    expect(list.devices.map((d) => d.name)).toEqual(["New tablet"]);
  });

  test("the QR code is shown only when it fits the terminal; the link alone otherwise", async () => {
    // Each run's device redeems the link and is rejected, which ends the command.
    const pairWith = (columns: number) => {
      let redeeming: Promise<string> | undefined;
      const onOut = (line: string) => {
        if (line.startsWith("finisher://")) redeeming = redeemLink(server.info.wsPort, line, "Phone");
      };
      return run(["pair", "--ttl", "30s"], person("y", "n", "y"), { columns, onOut }).then(async (r) => (await redeeming, r));
    };
    const wide = await pairWith(200);
    expect(wide.code, wide.err).toBe(0);
    expect(wide.out).toMatch(/[▀▄█]/);
    const narrow = await pairWith(20);
    expect(narrow.code, narrow.err).toBe(0);
    expect(narrow.out).not.toMatch(/[▀▄█]/);
    expect(narrow.out).toContain("The QR code is wider than this terminal");
    expect(narrow.out).toContain("finisher://pair?");
  });

  test("Ctrl-C at a request stops pair; --ttl is 30s to 10m", async () => {
    let redeeming: Promise<string> | undefined;
    const p = person("y");
    let asks = 0;
    const ctrlC = { ...p, ask: async (q: string) => (asks++, p.ask(q)), interrupted: () => asks > 1 };
    const r = await run(["pair", "--ttl", "30s"], ctrlC, {
      onOut: (line) => {
        if (line.startsWith("finisher://")) redeeming = redeemLink(server.info.wsPort, line, "Phone");
      },
    });
    await redeeming;
    expect(r.code, r.err).toBe(0);
    expect(r.out).toContain("Stopped: the link expires on its own.");
    expect(asks).toBe(2);
    for (const ttl of ["29s", "11m", "0"]) expect((await run(["pair", "--ttl", ttl], person())).err, ttl).toContain("USAGE");
  });

  test("a cancelled offer sends nothing", async () => {
    const r = await run(["pair"], person("n"));
    expect(r.err).toContain("CONFIRMATION_REQUIRED");
    expect(r.out).not.toContain("finisher://");
  });
});
