// `finisherd cache status [--json]` and `finisherd cache gc [--to <GiB>]` (6B-1 plan Task 13,
// Decision 12): the media cache's state, and an eviction of unpinned entries, least recently used
// first, down to the cap (`media.cacheMaxGiB`) or to `--to` when lower. `gc` is the owner's
// (`settings.admin`, which `fin cache gc` cannot hold): a terminal and a `y`.
import type { CacheGcResult, CacheStatus } from "@finisher/fin-node";
import { valueOf } from "../client.ts";
import { confirmed, proceed, requireTty } from "../confirm.ts";
import { has, one, onlyFlags, usage } from "../format.ts";
import { type CommandContext, withConnection } from "./context.ts";

const GIB = 2 ** 30;
const gib = (bytes: number): string => `${(bytes / GIB).toFixed(2)} GiB`;

export async function cache(ctx: CommandContext): Promise<number> {
  const [sub, ...more] = ctx.args.words;
  if (more.length > 0) usage(`cache ${sub ?? ""} takes no more arguments`);
  if (sub === "status") {
    onlyFlags(ctx.args, "cache status", ["json"]);
    const s = await withConnection(ctx, async (c) => valueOf<CacheStatus>(await c.call("media.cache.status", {})));
    if (has(ctx.args, "json")) {
      ctx.output.out(JSON.stringify(s));
      return 0;
    }
    ctx.output.out(`Media cache: ${gib(s.bytes)} of ${gib(s.capBytes)} in ${s.entries} entries, ${s.pinned} pinned`);
    for (const [recipe, u] of Object.entries(s.byRecipe)) ctx.output.out(`  ${recipe}: ${gib(u.bytes)} in ${u.entries} entries`);
    return 0;
  }
  if (sub !== "gc") return usage(sub === undefined ? "cache needs a command: status or gc" : `unknown cache command: ${sub}`);
  onlyFlags(ctx.args, "cache gc", ["to", "json"]);
  const to = one(ctx.args, "to");
  const toGiB = to === undefined ? undefined : Number(to);
  if (toGiB !== undefined && !(Number.isFinite(toGiB) && toGiB >= 0)) usage("--to needs a number of GiB, 0 or more");
  requireTty(ctx.prompt, "cache gc");
  const what = toGiB === undefined ? "down to its cap" : `down to ${toGiB} GiB (or its cap, when lower)`;
  const r = await withConnection(ctx, (c) =>
    confirmed<CacheGcResult>(c, ctx.prompt, `This evicts unpinned media cache entries ${what}; they are made again when needed.`, proceed, "media.cache.gc", toGiB === undefined ? {} : { toGiB }),
  );
  ctx.output.out(has(ctx.args, "json") ? JSON.stringify(r) : `Evicted ${r.evicted} entries (${gib(r.freedBytes)}); the cache holds ${gib(r.bytes)}.`);
  return 0;
}
