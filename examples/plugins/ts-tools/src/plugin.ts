// `@example/ts-tools`: an example Node plugin built with `@finisher/plugin-sdk` (plan 8B
// Task 13). `trim-silence` reads the snapshot and proposes a transaction (`writes: true`);
// `fetch-title` fetches a page through the host's HTTPS broker and returns data. `build.mjs`
// bundles this file and the SDK into `dist/plugin.mjs`, the manifest's `runtime.main`.
import type { DeleteMode, Op, Project } from "@finisher/core-types";
import { definePlugin, PluginError } from "@finisher/plugin-sdk";

interface TrimSilenceArgs {
  readonly thresholdDb?: number;
  readonly mode?: DeleteMode;
}

interface FetchTitleArgs {
  readonly url: string;
}

/** Items of the main timeline's audio tracks whose gain is at or below `thresholdDb`: as good as silent. */
export function silentItems(project: Project, thresholdDb: number): string[] {
  const main = project.timelines[0];
  if (main === undefined) return [];
  const ids: string[] = [];
  for (const track of main.tracks) {
    if (track.kind !== "audio" || track.lock === true) continue;
    for (const item of track.items) {
      const gain = item.audio?.gain;
      if (track.mute === true || (gain !== undefined && gain <= thresholdDb)) ids.push(item.id);
    }
  }
  return ids;
}

/** The text of the first `<title>` of an HTML page, whitespace collapsed. */
export function titleOf(html: string): string | null {
  const m = /<title[^>]*>([\s\S]*?)<\/title>/i.exec(html);
  return m?.[1] === undefined ? null : m[1].replace(/\s+/g, " ").trim();
}

definePlugin({
  commands: {
    "trim-silence": async (args: TrimSilenceArgs, host) => {
      const project = await host.snapshot();
      const mode = args.mode ?? "ripple";
      const ids = silentItems(project, args.thresholdDb ?? -60);
      host.log("info", `trim-silence: ${ids.length} silent items`);
      const tx: Op[] = ids.map((item) => ({ op: "item.delete", item, mode }));
      return { tx };
    },
    "fetch-title": async (args: FetchTitleArgs, host) => {
      const res = await host.fetch({ method: "GET", url: args.url, headers: [{ name: "accept", value: "text/html" }] }, {});
      if (res.status < 200 || res.status > 299) throw new PluginError("FETCH_TITLE_HTTP", `the page answered ${res.status}`, { status: res.status });
      return { data: { status: res.status, title: titleOf(new TextDecoder().decode(res.body)) } };
    },
  },
});
