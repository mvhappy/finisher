// The broker's address check (sp8b plan Task 8; spec §9.3 "Addresses"): an address `net.fetch`
// may connect to is a public unicast one. Refused, as `EGRESS_PRIVATE_ADDRESS`: loopback,
// private (RFC 1918, RFC 4193), link-local, CGNAT and Tailscale (100.64.0.0/10), multicast,
// unspecified, the rest of "this network" and the reserved 240.0.0.0/4 (with broadcast), the
// IPv6 forms that embed an IPv4 address (mapped `::ffff:0:0/96`, compatible `::/96`, NAT64
// `64:ff9b::/96`, 6to4 `2002::/16`; each decoded and checked as IPv4 first), and the
// deprecated site-local `fec0::/10`. Pure; no resolution happens here.
import { isIPv4, isIPv6 } from "node:net";

/** Why an address is refused, or `undefined` for a public unicast address. */
export type PrivateReason =
  | "unspecified"
  | "loopback"
  | "private"
  | "linkLocal"
  | "cgnat"
  | "multicast"
  | "reserved"
  | "invalid";

type V4Rule = readonly [base: number, bits: number, reason: PrivateReason];

const v4 = (a: number, b: number, c: number, d: number): number => ((a << 24) | (b << 16) | (c << 8) | d) >>> 0;

const V4_RULES: ReadonlyArray<V4Rule> = [
  [v4(0, 0, 0, 0), 8, "unspecified"], // "this network", 0.0.0.0 included
  [v4(127, 0, 0, 0), 8, "loopback"],
  [v4(10, 0, 0, 0), 8, "private"],
  [v4(172, 16, 0, 0), 12, "private"],
  [v4(192, 168, 0, 0), 16, "private"],
  [v4(169, 254, 0, 0), 16, "linkLocal"],
  [v4(100, 64, 0, 0), 10, "cgnat"], // CGNAT and Tailscale
  [v4(224, 0, 0, 0), 4, "multicast"],
  [v4(240, 0, 0, 0), 4, "reserved"], // 255.255.255.255 included
];

function parseV4(s: string): number | undefined {
  if (!isIPv4(s)) return undefined;
  const p = s.split(".").map(Number);
  return v4(p[0]!, p[1]!, p[2]!, p[3]!);
}

function checkV4(n: number): PrivateReason | undefined {
  for (const [base, bits, reason] of V4_RULES) {
    const mask = bits === 0 ? 0 : (0xffffffff << (32 - bits)) >>> 0;
    if ((n & mask) >>> 0 === base) return reason;
  }
  return undefined;
}

/** The eight 16-bit groups of an IPv6 address (`::` expanded, a trailing dotted IPv4 split). */
export function ipv6Groups(s: string): number[] | undefined {
  const text = s.split("%")[0]!; // a zone id names an interface: the address is what counts
  if (!isIPv6(text)) return undefined;
  let head = text;
  const tail: number[] = [];
  const lastColon = text.lastIndexOf(":");
  const last = text.slice(lastColon + 1);
  if (last.includes(".")) {
    const n = parseV4(last);
    if (n === undefined) return undefined;
    tail.push(n >>> 16, n & 0xffff);
    head = text.slice(0, lastColon + 1);
    if (head.endsWith(":") && !head.endsWith("::")) head = head.slice(0, -1);
  }
  const groups = (part: string): number[] => (part === "" ? [] : part.split(":").map((g) => parseInt(g, 16)));
  const dbl = head.indexOf("::");
  let out: number[];
  if (dbl >= 0) {
    const left = groups(head.slice(0, dbl));
    const right = groups(head.slice(dbl + 2));
    const fill = 8 - tail.length - left.length - right.length;
    if (fill < 0) return undefined;
    out = [...left, ...Array<number>(fill).fill(0), ...right, ...tail];
  } else {
    out = [...groups(head), ...tail];
  }
  return out.length === 8 && out.every((g) => Number.isInteger(g) && g >= 0 && g <= 0xffff) ? out : undefined;
}

const embedded = (hi: number, lo: number): number => ((hi << 16) | lo) >>> 0;

function checkV6(g: ReadonlyArray<number>): PrivateReason | undefined {
  const [a, b, c, d, e, f, h, l] = g as [number, number, number, number, number, number, number, number];
  const zero5 = a === 0 && b === 0 && c === 0 && d === 0 && e === 0;
  if (zero5 && f === 0) {
    if (h === 0 && l === 0) return "unspecified";
    if (h === 0 && l === 1) return "loopback";
    // IPv4-compatible `::a.b.c.d` (deprecated): checked as the IPv4 address it names.
    return checkV4(embedded(h, l)) ?? "reserved";
  }
  // IPv4-mapped `::ffff:a.b.c.d`.
  if (zero5 && f === 0xffff) return checkV4(embedded(h, l));
  // NAT64 well-known prefix `64:ff9b::/96`.
  if (a === 0x64 && b === 0xff9b && c === 0 && d === 0 && e === 0 && f === 0) return checkV4(embedded(h, l));
  // 6to4 `2002:AABB:CCDD::/48` carries the IPv4 address in its second and third groups.
  if (a === 0x2002) return checkV4(embedded(b, c));
  if ((a & 0xfe00) === 0xfc00) return "private"; // fc00::/7 (Tailscale's fd7a:115c:a1e0::/48 included)
  if ((a & 0xffc0) === 0xfe80) return "linkLocal"; // fe80::/10
  if ((a & 0xffc0) === 0xfec0) return "private"; // fec0::/10, site-local
  if ((a & 0xff00) === 0xff00) return "multicast"; // ff00::/8
  return undefined;
}

/**
 * Why `address` (an IPv4 or IPv6 literal, as `dns.lookup` returns it) may not be connected to,
 * or `undefined` when it is a public unicast address. Anything that is not an address is
 * `invalid` (refused too).
 */
export function privateReason(address: string): PrivateReason | undefined {
  const n = parseV4(address);
  if (n !== undefined) return checkV4(n);
  const g = ipv6Groups(address);
  return g === undefined ? "invalid" : checkV6(g);
}

/** A URL host as an address literal (`[::1]` → `::1`), or `undefined` for a name. */
export function hostLiteral(hostname: string): string | undefined {
  const h = hostname.startsWith("[") && hostname.endsWith("]") ? hostname.slice(1, -1) : hostname;
  return isIPv4(h) || isIPv6(h) ? h : undefined;
}
