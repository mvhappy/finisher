// What every console command gets: its arguments, where to print, how to ask the person and how
// to reach the server (`connect`: proved as `console`, closed by the caller).
import type { ConsoleConnection } from "../client.ts";
import type { Args, Output } from "../format.ts";
import type { Prompt } from "../prompt.ts";

export interface CommandContext {
  readonly args: Args;
  readonly serverData: string;
  readonly prompt: Prompt;
  readonly output: Output;
  readonly connect: () => Promise<ConsoleConnection>;
}

/** Runs `f` on a fresh console connection, closed afterwards. */
export async function withConnection<A>(ctx: CommandContext, f: (c: ConsoleConnection) => Promise<A>): Promise<A> {
  const c = await ctx.connect();
  try {
    return await f(c);
  } finally {
    await c.close();
  }
}
