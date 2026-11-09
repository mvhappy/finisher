// `finisherd pair [--label <text>] [--ttl <duration>]` (plan 5B Task 12, spec §5.6): a one-time
// pairing link for a device, after a `y`. It prints the link, a browser link when a trusted
// certificate route has one, and the QR code when it fits the terminal (else the link alone:
// the link carries at most three routes, so the code stays small). Then it waits on
// `auth.pair.pending`: for each request it shows the device's name, platform and safety code
// and asks to approve (`y`), else to reject (`y`), else leaves it waiting. Requests already
// waiting are asked about too. It ends once a request that came after its offer is decided and
// none is left to ask about, or when the offer expires; Ctrl-C leaves the offer to expire. The link is printed here only (stdout of this command), never logged.
import { PAIR_OFFER_MAX_TTL_S, type PendingView } from "@finisher/contracts";
import { qrText } from "@finisher/fin-node";
import { type ConsoleConnection, ConsoleError } from "../client.ts";
import { confirmed, proceed, requireTty, withChallenge } from "../confirm.ts";
import { one, onlyFlags, qrThatFits, usage } from "../format.ts";
import { safe } from "../safe.ts";
import { parseDuration } from "./agent.ts";
import { type CommandContext, withConnection } from "./context.ts";

type PendingItem =
  | { kind: "snapshot"; pending: PendingView[] }
  | { kind: "added"; pending: PendingView }
  | { kind: "ended"; pendingId: string; state: "approved" | "rejected" | "expired" };

interface Offer {
  offerId: string;
  link: string;
  browserLink?: string;
  expiresAt: number;
}

const yes = (a: string) => ["y", "yes"].includes(a.trim().toLowerCase());

export async function pair(ctx: CommandContext): Promise<number> {
  onlyFlags(ctx.args, "pair", ["label", "ttl"]);
  if (ctx.args.words.length > 0) usage("pair takes no arguments");
  const label = one(ctx.args, "label");
  const ttlText = one(ctx.args, "ttl");
  const ttlS = ttlText === undefined ? undefined : parseDuration("ttl", ttlText, false)!;
  if (ttlS !== undefined && (ttlS < 30 || ttlS > PAIR_OFFER_MAX_TTL_S)) usage(`--ttl is 30s to ${PAIR_OFFER_MAX_TTL_S / 60}m`);
  requireTty(ctx.prompt, "pair");
  return withConnection(ctx, async (c) => {
    // Watching first: the request for this offer then always comes as `added`, however soon the
    // device redeems (review of Task 12); requests already waiting are in the snapshot.
    const requests = new Requests();
    const watch = c.stream("auth.pair.pending", {}, (v) => requests.take(v as PendingItem));
    const dropped = watch.done.then((exit) => {
      throw new ConsoleError(exit.ok ? { code: "SERVER_UNAVAILABLE", message: "the server ended the pairing requests stream" } : exit.error);
    });
    dropped.catch(() => undefined);
    try {
      await Promise.race([requests.snapshot, dropped]);
      const offer = await confirmed<Offer>(
        c,
        ctx.prompt,
        "This makes a one-time pairing link. The device that opens it asks to pair, and you approve it here after comparing its safety code.",
        proceed,
        "auth.pair.offer",
        { ...(label === undefined ? {} : { label }), ...(ttlS === undefined ? {} : { ttlS }) },
      );
      showOffer(ctx, offer);
      await Promise.race([decide(ctx, c, requests, offer.expiresAt), dropped]);
      return 0;
    } finally {
      requests.close();
      watch.stop();
    }
  });
}

function showOffer(ctx: CommandContext, offer: Offer): void {
  const o = ctx.output;
  o.out(`Pairing link (one use, until ${new Date(offer.expiresAt).toISOString()}):`);
  o.out(safe(offer.link));
  if (offer.browserLink !== undefined) o.out(`In a browser: ${safe(offer.browserLink)}`);
  const qr = qrThatFits(qrText(offer.link), ctx.columns?.());
  if (qr === undefined) o.out("The QR code is wider than this terminal: open the link on the device, or widen the terminal and run pair again.");
  else o.out(qr);
  o.out("Waiting for the device. Press Ctrl-C to stop; the link then expires on its own.");
}

/** The requests to ask about, in order: the snapshot's (old), then each one added (fresh). */
class Requests {
  readonly queue: Array<{ view: PendingView; fresh: boolean }> = [];
  readonly #seen = new Set<string>();
  readonly #ended = new Set<string>();
  #wake: (() => void) | undefined;
  #snapshot!: () => void;
  readonly snapshot = new Promise<void>((r) => (this.#snapshot = r));

  take(item: PendingItem): void {
    if (item.kind === "snapshot") {
      for (const p of item.pending) this.#push(p, false);
      this.#snapshot();
    } else if (item.kind === "added") this.#push(item.pending, true);
    else this.#ended.add(item.pendingId);
    this.#wake?.();
  }

  #push(p: PendingView, fresh: boolean): void {
    if (this.#seen.has(p.pendingId)) return;
    this.#seen.add(p.pendingId);
    this.queue.push({ view: p, fresh });
  }

  /** The next request still waiting (ones ended elsewhere are dropped). */
  next(): { view: PendingView; fresh: boolean } | undefined {
    for (;;) {
      const n = this.queue.shift();
      if (n === undefined || !this.#ended.has(n.view.pendingId)) return n;
    }
  }

  /** Resolves on the next change, or after `ms`. */
  wait(ms: number): Promise<void> {
    return new Promise<void>((resolve) => {
      const timer = setTimeout(done, ms);
      const self = this;
      function done() {
        clearTimeout(timer);
        self.#wake = undefined;
        resolve();
      }
      this.#wake = done;
    });
  }

  close(): void {
    this.#wake?.();
  }
}

/** Asks about each request in turn until the module docs' end. */
async function decide(ctx: CommandContext, c: ConsoleConnection, requests: Requests, expiresAt: number): Promise<void> {
  let waiting = 0;
  for (;;) {
    const next = requests.next();
    if (next !== undefined) {
      const answer = await ask(ctx, c, next.view);
      if (answer === "stop") {
        ctx.output.out("Stopped: the link expires on its own.");
        return;
      }
      if (answer === "decided" && next.fresh) return;
      if (answer === "left") waiting++;
      continue;
    }
    const left = expiresAt - Date.now();
    if (left <= 0) {
      ctx.output.out(waiting > 0 ? "The link expired; the requests left waiting expire on their own." : "The link expired: no device asked to pair.");
      return;
    }
    await requests.wait(left);
  }
}

/** Codes that end the command: the connection is gone, not the request. */
const FATAL = new Set(["SERVER_UNAVAILABLE", "UNAUTHENTICATED", "SERVER_TIMEOUT"]);

/** One request: approve, reject or leave it; `stop` on Ctrl-C. */
async function ask(ctx: CommandContext, c: ConsoleConnection, p: PendingView): Promise<"decided" | "left" | "stop"> {
  const { prompt, output } = ctx;
  const name = safe(p.deviceName);
  prompt.show(`A device asks to pair: "${name}" (${safe(p.platform)}).`);
  prompt.show(`Its safety code: ${safe(p.safetyCode)}. Approve it only if the device shows the same code.`);
  const question = async (q: string) => {
    const a = await prompt.ask(q);
    return prompt.interrupted?.() === true ? undefined : yes(a);
  };
  try {
    const approve = await question("Approve this device? [y/N] ");
    if (approve === undefined) return "stop";
    if (approve) {
      const { device } = await withChallenge<{ device: { id: string; name: string } }>(c, "auth.pair.approve", { pendingId: p.pendingId });
      output.out(`Paired "${safe(device.name)}" as device ${safe(device.id)}.`);
      return "decided";
    }
    const reject = await question("Reject it? [y/N] ");
    if (reject === undefined) return "stop";
    if (reject) {
      await withChallenge(c, "auth.pair.reject", { pendingId: p.pendingId });
      output.out(`Rejected "${name}".`);
      return "decided";
    }
  } catch (e) {
    // An expired or already decided request: say so and go on with the others.
    const code = (e as { error?: { code?: string } }).error?.code;
    if (code === undefined || FATAL.has(code)) throw e;
    output.out(`Could not decide on "${name}": ${safe(code)}.`);
    return "decided";
  }
  output.out(`Left "${name}" waiting; it expires on its own.`);
  return "left";
}
