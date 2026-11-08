// 5B Task 5: route discovery (Decision 10) over fake interfaces.
// Addresses are documentation ranges (RFC 5737, RFC 3849), link-local and made-up shared-space
// (100.64.0.0/10) ones: no real host.
import type { NetworkInterfaceInfo } from "node:os";
import { describe, expect, test } from "vitest";
import { discover, isLinkLocal6, isTailscale, type RouteSource, Routes } from "../src/net/routes.ts";

const v4 = (address: string, internal = false): NetworkInterfaceInfo =>
  ({ address, family: "IPv4", internal, netmask: "255.255.255.0", mac: "00:00:00:00:00:00", cidr: null }) as NetworkInterfaceInfo;
const v6 = (address: string, internal = false): NetworkInterfaceInfo =>
  ({ address, family: "IPv6", internal, netmask: "ffff:ffff:ffff:ffff::", mac: "00:00:00:00:00:00", scopeid: 0, cidr: null }) as NetworkInterfaceInfo;

function source(ifaces: Record<string, NetworkInterfaceInfo[]>): RouteSource & { set(i: Record<string, NetworkInterfaceInfo[]>): void } {
  let now = ifaces;
  return { interfaces: () => now, hostname: () => "studio-box", set: (i) => (now = i) };
}

const FAKE = {
  lo: [v4("127.0.0.1", true), v6("::1", true)],
  eth0: [v4("192.0.2.40"), v6("fe80::1234"), v6("2001:db8::5")],
  tailscale0: [v4("100.64.0.7"), v6("fd7a:115c:a1e0::42")],
  outside: [v4("198.51.100.9"), v4("169.254.10.20")],
};

describe("routes", () => {
  test("routes_classify_interfaces", () => {
    const routes = discover(source(FAKE), { loopbackPort: () => 4100, remote: { port: 7311, pin: "identity" } });
    expect(routes).toEqual([
      { kind: "loopback", url: "http://127.0.0.1:4100", pin: "identity" },
      { kind: "tailscale", url: "https://100.64.0.7:7311", pin: "identity" },
      { kind: "tailscale", url: "https://[fd7a:115c:a1e0::42]:7311", pin: "identity" },
      { kind: "lan", url: "https://192.0.2.40:7311", pin: "identity" },
      { kind: "lan", url: "https://198.51.100.9:7311", pin: "identity" },
      { kind: "lan", url: "https://[2001:db8::5]:7311", pin: "identity" },
    ]);
    // Internal and link-local addresses never become routes.
    expect(JSON.stringify(routes)).not.toContain("fe80");
    expect(JSON.stringify(routes)).not.toContain("169.254");
    expect(isTailscale("100.64.0.0", "IPv4")).toBe(true);
    expect(isTailscale("100.127.255.255", "IPv4")).toBe(true);
    expect(isTailscale("198.51.100.9", "IPv4")).toBe(false);
    expect(isTailscale("FD7A:115C:A1E0:ab::1", "IPv6")).toBe(true);
    expect(isLinkLocal6("febf::1")).toBe(true);
    expect(isLinkLocal6("fec0::1")).toBe(false);
  });

  test("only loopback while the remote listener is off", () => {
    expect(discover(source(FAKE), { loopbackPort: () => 4100 })).toEqual([{ kind: "loopback", url: "http://127.0.0.1:4100", pin: "identity" }]);
  });

  test("public_route_from_public_url", () => {
    const routes = discover(source({ lo: FAKE.lo }), {
      loopbackPort: () => 4100,
      remote: { port: 7311, pin: "webpki" },
      publicUrl: "https://studio.example.test/",
    });
    expect(routes).toEqual([
      { kind: "loopback", url: "http://127.0.0.1:4100", pin: "identity" },
      { kind: "public", url: "https://studio.example.test", pin: "webpki" },
    ]);
  });

  test("route_change_fires_once", () => {
    const src = source({ lo: FAKE.lo, eth0: [v4("192.0.2.40")] });
    const routes = new Routes(src, { loopbackPort: () => 4100, remote: { port: 7311, pin: "identity" } });
    const seen: number[] = [];
    routes.onChange((r) => seen.push(r.length));
    routes.poll();
    expect(seen).toEqual([]);
    src.set({ lo: FAKE.lo, eth0: [v4("192.0.2.41")] });
    routes.poll();
    routes.poll();
    expect(seen).toEqual([2]);
    // The listener stopping drops every route but loopback, once.
    routes.reconfigure({ remote: undefined });
    routes.poll();
    expect(seen).toEqual([2, 1]);
    // The certificate's names: route hosts and the host name.
    routes.reconfigure({ remote: { port: 7311, pin: "identity" } });
    expect(routes.sans()).toEqual(["192.0.2.41", "studio-box"]);
  });
});
